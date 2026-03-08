use anyhow::{Context as AnyhowContext, Result};
use boa_engine::string::JsString;
use boa_engine::{Context, JsValue, NativeFunction, Source};
use glob::Pattern;
use log::error;
use std::fs;
use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct PacEngine {
    senders: Vec<mpsc::Sender<PacRequest>>,
    next_worker: Arc<AtomicUsize>,
}

struct PacRequest {
    url: String,
    host: String,
    respond_to: oneshot::Sender<Result<String>>,
}

fn glob_match(pattern: &str, text: &str) -> bool {
    // The glob crate fails to parse patterns with more than two consecutive asterisks
    // (e.g. `***`) returning a PatternError ("wildcards are either regular `*` or recursive `**`").
    // We collapse consecutive asterisks to `*` before parsing.
    let mut collapsed_pattern = String::with_capacity(pattern.len());
    let mut last_was_star = false;
    for c in pattern.chars() {
        if c == '*' {
            if !last_was_star {
                collapsed_pattern.push(c);
                last_was_star = true;
            }
        } else {
            collapsed_pattern.push(c);
            last_was_star = false;
        }
    }

    Pattern::new(&collapsed_pattern)
        .map(|p| p.matches(text))
        .unwrap_or(false)
}

fn register_pac_functions(context: &mut Context) {
    let _ = context.register_global_callable(
        JsString::from("dnsResolve"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            Ok(JsValue::from(JsString::from(host)))
        }),
    );

    let _ = context.register_global_callable(
        JsString::from("myIpAddress"),
        0,
        NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::from(JsString::from("127.0.0.1")))),
    );

    let _ = context.register_global_callable(
        JsString::from("shExpMatch"),
        2,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let str_val = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let pattern = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let matched = glob_match(&pattern, &str_val);
            Ok(JsValue::from(matched))
        }),
    );

    // isPlainHostName(host) - true if no dots in hostname
    let _ = context.register_global_callable(
        JsString::from("isPlainHostName"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            Ok(JsValue::from(!host.contains('.')))
        }),
    );

    // dnsDomainIs(host, domain) - true if host's domain matches
    let _ = context.register_global_callable(
        JsString::from("dnsDomainIs"),
        2,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let domain = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            Ok(JsValue::from(host.ends_with(&domain)))
        }),
    );

    // localHostOrDomainIs(host, hostdom) - true if exact match
    // or host (without domain) matches hostdom's host part
    let _ = context.register_global_callable(
        JsString::from("localHostOrDomainIs"),
        2,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let hostdom = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let result = host == hostdom || hostdom.starts_with(&format!("{}.", host));
            Ok(JsValue::from(result))
        }),
    );

    // isResolvable(host) - true if DNS can resolve the host
    let _ = context.register_global_callable(
        JsString::from("isResolvable"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let resolvable = format!("{}:0", host)
                .to_socket_addrs()
                .map(|mut addrs| addrs.next().is_some())
                .unwrap_or(false);
            Ok(JsValue::from(resolvable))
        }),
    );

    // isInNet(host, pattern, mask) - true if IP of host matches pattern/mask
    let _ = context.register_global_callable(
        JsString::from("isInNet"),
        3,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let pattern = args
                .get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let mask = args
                .get(2)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();

            let resolve_ip = |h: &str| -> Option<u32> {
                // Try parsing as IP first, then DNS resolve
                if let Ok(ip) = h.parse::<std::net::Ipv4Addr>() {
                    return Some(u32::from(ip));
                }
                format!("{}:0", h)
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut addrs| {
                        addrs.find_map(|a| match a {
                            std::net::SocketAddr::V4(v4) => Some(u32::from(*v4.ip())),
                            _ => None,
                        })
                    })
            };

            let result = (|| -> Option<bool> {
                let host_ip = resolve_ip(&host)?;
                let pattern_ip = pattern.parse::<std::net::Ipv4Addr>().ok()?;
                let mask_ip = mask.parse::<std::net::Ipv4Addr>().ok()?;
                let pattern_int = u32::from(pattern_ip);
                let mask_int = u32::from(mask_ip);
                Some((host_ip & mask_int) == (pattern_int & mask_int))
            })()
            .unwrap_or(false);
            Ok(JsValue::from(result))
        }),
    );

    // dnsDomainLevels(host) - returns number of dots in hostname
    let _ = context.register_global_callable(
        JsString::from("dnsDomainLevels"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let host = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let levels = host.matches('.').count() as i32;
            Ok(JsValue::from(levels))
        }),
    );

    // convert_addr(ipaddr) - converts dotted IP string to integer
    let _ = context.register_global_callable(
        JsString::from("convert_addr"),
        1,
        NativeFunction::from_fn_ptr(|_, args, _| {
            let addr = args
                .first()
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let result = addr
                .parse::<std::net::Ipv4Addr>()
                .map(|ip| u32::from(ip) as f64)
                .unwrap_or(0.0);
            Ok(JsValue::from(result))
        }),
    );

    // weekdayRange, dateRange, timeRange - stub implementations
    // that return true (permissive) to avoid blocking traffic
    let _ = context.register_global_callable(
        JsString::from("weekdayRange"),
        3,
        NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::from(true))),
    );

    let _ = context.register_global_callable(
        JsString::from("dateRange"),
        7,
        NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::from(true))),
    );

    let _ = context.register_global_callable(
        JsString::from("timeRange"),
        7,
        NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::from(true))),
    );
}

impl PacEngine {
    pub async fn new(pac_url_or_path: &str) -> Result<Self> {
        let script = if pac_url_or_path.starts_with("http") {
            // PAC files must always be fetched with a DIRECT connection (no proxy),
            // otherwise we'd have a circular dependency: needing the proxy config
            // to fetch the file that provides proxy config.
            // We also disable TLS to ensure plain HTTP works for WPAD/PAC URLs.
            let client = reqwest::Client::builder()
                .no_proxy()
                .build()
                .context("Failed to build HTTP client for PAC fetch")?;
            client
                .get(pac_url_or_path)
                .send()
                .await?
                .text()
                .await
                .context("Failed to fetch PAC file")?
        } else {
            fs::read_to_string(pac_url_or_path).context("Failed to read PAC file")?
        };

        // Determine number of workers based on available cores (fallback to 4)
        let num_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let mut senders = Vec::with_capacity(num_workers);

        for _ in 0..num_workers {
            let (tx, mut rx) = mpsc::channel::<PacRequest>(32);
            senders.push(tx);
            let script_clone = script.clone();

            // Boa JS engine uses deep recursion for parsing/evaluation;
            // the default thread stack size can cause stack overflows on
            // complex PAC scripts, so we allocate 8 MB per worker thread.
            thread::Builder::new()
                .name("pac-worker".into())
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    let mut context = Context::default();

                    register_pac_functions(&mut context);

                    if let Err(e) = context.eval(Source::from_bytes(&script_clone)) {
                        error!("Failed to evaluate PAC script: {}", e);
                    }

                    let func_name = JsString::from("FindProxyForURL");

                    while let Some(req) = rx.blocking_recv() {
                        let global_obj = context.global_object();

                        let result = (|| -> Result<String> {
                            let func = global_obj
                                .get(func_name.clone(), &mut context)
                                .map_err(|e| anyhow::anyhow!("JS Error: {}", e))?;

                            if !func.is_callable() {
                                return Err(anyhow::anyhow!("FindProxyForURL is not defined"));
                            }
                            let args = [
                                JsValue::from(JsString::from(req.url)),
                                JsValue::from(JsString::from(req.host)),
                            ];
                            let res = func
                                .as_callable()
                                .unwrap()
                                .call(&JsValue::undefined(), &args, &mut context)
                                .map_err(|e| anyhow::anyhow!("JS Error: {}", e))?;

                            res.as_string()
                                .map(|s| s.to_std_string_escaped())
                                .ok_or_else(|| {
                                    anyhow::anyhow!("FindProxyForURL returned non-string")
                                })
                        })();

                        let _ = req.respond_to.send(result);
                    }
                })
                .context("Failed to spawn PAC worker thread")?;
        }

        Ok(PacEngine {
            senders,
            next_worker: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub async fn find_proxy(&self, url: &str, host: &str) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        let req = PacRequest {
            url: url.to_string(),
            host: host.to_string(),
            respond_to: tx,
        };

        let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.senders.len();

        self.senders[worker_idx]
            .send(req)
            .await
            .map_err(|_| anyhow::anyhow!("PAC thread dead"))?;

        match rx.await {
            Ok(res) => res,
            Err(_) => Err(anyhow::anyhow!("PAC thread dropped channel")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{glob_match, register_pac_functions};
    use boa_engine::{Context, Source};

    #[test]
    fn test_is_plain_host_name() {
        let mut context = Context::default();
        register_pac_functions(&mut context);

        let res1 = context
            .eval(Source::from_bytes("isPlainHostName('example.com')"))
            .unwrap();
        assert_eq!(res1.as_boolean(), Some(false));

        let res2 = context
            .eval(Source::from_bytes("isPlainHostName('localhost')"))
            .unwrap();
        assert_eq!(res2.as_boolean(), Some(true));
    }

    #[test]
    fn test_glob_match_cases() {
        let cases = vec![
            // Exact match
            ("abc", "abc", true),
            ("abc", "def", false),
            // Wildcard (*)
            ("*", "anything", true),
            ("*", "", true),
            ("abc*", "abcdef", true),
            ("*def", "abcdef", true),
            ("ab*ef", "abcdef", true),
            ("a*c", "abc", true),
            ("a*c", "abbc", true),
            ("*bc*", "abcdef", true),
            // Question mark (?)
            ("?", "a", true),
            ("?", "", false),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("a?c", "abbc", false),
            // Mixed
            ("?b*", "abc", true),
            ("*b?", "abc", true),
            // Edge cases
            ("", "", true),
            ("", "a", false),
            ("a", "", false),
            ("*******", "a", true),
        ];

        for (pattern, text, expected) in cases {
            assert_eq!(
                glob_match(pattern, text),
                expected,
                "Pattern: '{}', Text: '{}'",
                pattern,
                text
            );
        }
    }
}
