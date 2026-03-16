use anyhow::{Context as AnyhowContext, Result};
use glob::Pattern;
use log::error;
use rquickjs::function::Rest;
use rquickjs::{Context, Function, Runtime};
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

fn register_pac_functions(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    globals.set(
        "dnsResolve",
        Function::new(ctx.clone(), |host: String| -> String { host })?,
    )?;

    globals.set(
        "myIpAddress",
        Function::new(ctx.clone(), || -> String { "127.0.0.1".to_string() })?,
    )?;

    globals.set(
        "shExpMatch",
        Function::new(ctx.clone(), |str_val: String, pattern: String| -> bool {
            glob_match(&pattern, &str_val)
        })?,
    )?;

    globals.set(
        "isPlainHostName",
        Function::new(ctx.clone(), |host: String| -> bool { !host.contains('.') })?,
    )?;

    globals.set(
        "dnsDomainIs",
        Function::new(ctx.clone(), |host: String, domain: String| -> bool {
            host.ends_with(&domain)
        })?,
    )?;

    globals.set(
        "localHostOrDomainIs",
        Function::new(ctx.clone(), |host: String, hostdom: String| -> bool {
            host == hostdom || hostdom.starts_with(&format!("{}.", host))
        })?,
    )?;

    globals.set(
        "isResolvable",
        Function::new(ctx.clone(), |host: String| -> bool {
            format!("{}:0", host)
                .to_socket_addrs()
                .map(|mut addrs| addrs.next().is_some())
                .unwrap_or(false)
        })?,
    )?;

    globals.set(
        "isInNet",
        Function::new(
            ctx.clone(),
            |host: String, pattern: String, mask: String| -> bool {
                let resolve_ip = |h: &str| -> Option<u32> {
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
                (|| -> Option<bool> {
                    let host_ip = resolve_ip(&host)?;
                    let pattern_ip = pattern.parse::<std::net::Ipv4Addr>().ok()?;
                    let mask_ip = mask.parse::<std::net::Ipv4Addr>().ok()?;
                    let pattern_int = u32::from(pattern_ip);
                    let mask_int = u32::from(mask_ip);
                    Some((host_ip & mask_int) == (pattern_int & mask_int))
                })()
                .unwrap_or(false)
            },
        )?,
    )?;

    globals.set(
        "dnsDomainLevels",
        Function::new(ctx.clone(), |host: String| -> i32 {
            host.matches('.').count() as i32
        })?,
    )?;

    globals.set(
        "convert_addr",
        Function::new(ctx.clone(), |addr: String| -> f64 {
            addr.parse::<std::net::Ipv4Addr>()
                .map(|ip| u32::from(ip) as f64)
                .unwrap_or(0.0)
        })?,
    )?;

    // weekdayRange, dateRange, timeRange - stub implementations
    // that return true (permissive) to avoid blocking traffic
    globals.set(
        "weekdayRange",
        Function::new(ctx.clone(), |_args: Rest<String>| -> bool { true })?,
    )?;

    globals.set(
        "dateRange",
        Function::new(ctx.clone(), |_args: Rest<String>| -> bool { true })?,
    )?;

    globals.set(
        "timeRange",
        Function::new(ctx.clone(), |_args: Rest<String>| -> bool { true })?,
    )?;

    Ok(())
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

        // PAC evaluation is quick (sub-millisecond) so a small pool suffices.
        // Cap at 4 workers to avoid wasting memory on 8 MB stacks (one per core
        // would be 128 MB on a 16-core machine sitting idle).
        let num_workers = std::thread::available_parallelism()
            .map(|n| n.get().min(4))
            .unwrap_or(2);

        let mut senders = Vec::with_capacity(num_workers);
        let script = Arc::new(script);

        for _ in 0..num_workers {
            let (tx, mut rx) = mpsc::channel::<PacRequest>(32);
            senders.push(tx);
            let script_clone = Arc::clone(&script);

            // QuickJS uses deep recursion for parsing/evaluation;
            // the default thread stack size can cause stack overflows on
            // complex PAC scripts, so we allocate 8 MB per worker thread.
            thread::Builder::new()
                .name("pac-worker".into())
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    let rt = match Runtime::new() {
                        Ok(r) => r,
                        Err(e) => {
                            error!("Failed to create JS runtime: {}", e);
                            return;
                        }
                    };
                    let ctx = match Context::full(&rt) {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to create JS context: {}", e);
                            return;
                        }
                    };

                    ctx.with(|ctx| {
                        if let Err(e) = register_pac_functions(&ctx) {
                            error!("Failed to register PAC functions: {}", e);
                        }
                        if let Err(e) = ctx.eval::<(), _>(script_clone.as_str()) {
                            error!("Failed to evaluate PAC script: {}", e);
                        }
                    });

                    while let Some(req) = rx.blocking_recv() {
                        let url = req.url;
                        let host = req.host;
                        let respond_to = req.respond_to;

                        let result = ctx.with(|ctx| -> Result<String> {
                            let func = ctx
                                .globals()
                                .get::<_, Function>("FindProxyForURL")
                                .map_err(|e| anyhow::anyhow!("JS Error: {}", e))?;

                            func.call::<_, String>((url.as_str(), host.as_str()))
                                .map_err(|e| anyhow::anyhow!("JS Error: {}", e))
                        });

                        let _ = respond_to.send(result);
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
    use rquickjs::{Context, Runtime};

    #[test]
    fn test_is_plain_host_name() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            register_pac_functions(&ctx).unwrap();

            let res1: bool = ctx.eval("isPlainHostName('example.com')").unwrap();
            assert_eq!(res1, false);

            let res2: bool = ctx.eval("isPlainHostName('localhost')").unwrap();
            assert_eq!(res2, true);
        });
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
