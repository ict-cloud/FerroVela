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
    use super::{glob_match, register_pac_functions, PacEngine};
    use rquickjs::{Context, Runtime};

    /// Helper: create a JS runtime with all PAC globals registered.
    fn pac_ctx() -> (Runtime, Context) {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        ctx.with(|ctx| register_pac_functions(&ctx).unwrap());
        (rt, ctx)
    }

    #[test]
    fn test_is_plain_host_name() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
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

    // ── dnsDomainIs ───────────────────────────────────────────────────────────

    #[test]
    fn test_dns_domain_is() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            let t: bool = ctx
                .eval("dnsDomainIs('www.netscape.com', '.netscape.com')")
                .unwrap();
            assert!(t);

            let f: bool = ctx
                .eval("dnsDomainIs('www.netscape.com', '.mcom.com')")
                .unwrap();
            assert!(!f);

            let f2: bool = ctx
                .eval("dnsDomainIs('localhost', '.netscape.com')")
                .unwrap();
            assert!(!f2);
        });
    }

    // ── localHostOrDomainIs ───────────────────────────────────────────────────

    #[test]
    fn test_local_host_or_domain_is() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            // Exact FQDN match
            let t1: bool = ctx
                .eval("localHostOrDomainIs('www.netscape.com', 'www.netscape.com')")
                .unwrap();
            assert!(t1);

            // Short name that is the hostname part of the domain
            let t2: bool = ctx
                .eval("localHostOrDomainIs('www', 'www.netscape.com')")
                .unwrap();
            assert!(t2);

            // Different hostname
            let f: bool = ctx
                .eval("localHostOrDomainIs('home.netscape.com', 'www.netscape.com')")
                .unwrap();
            assert!(!f);
        });
    }

    // ── dnsDomainLevels ───────────────────────────────────────────────────────

    #[test]
    fn test_dns_domain_levels() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            let lvl0: i32 = ctx.eval("dnsDomainLevels('localhost')").unwrap();
            assert_eq!(lvl0, 0);

            let lvl1: i32 = ctx.eval("dnsDomainLevels('example.com')").unwrap();
            assert_eq!(lvl1, 1);

            let lvl2: i32 = ctx.eval("dnsDomainLevels('www.example.com')").unwrap();
            assert_eq!(lvl2, 2);

            let lvl3: i32 = ctx.eval("dnsDomainLevels('a.b.example.com')").unwrap();
            assert_eq!(lvl3, 3);
        });
    }

    // ── shExpMatch ────────────────────────────────────────────────────────────

    #[test]
    fn test_sh_exp_match() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            let t: bool = ctx
                .eval("shExpMatch('http://home.netscape.com/people/ari/index.html', '*/ari/*')")
                .unwrap();
            assert!(t);

            let f: bool = ctx
                .eval("shExpMatch('http://home.netscape.com/people/other/index.html', '*/ari/*')")
                .unwrap();
            assert!(!f);

            // Wildcard-only pattern matches anything
            let t2: bool = ctx.eval("shExpMatch('anything', '*')").unwrap();
            assert!(t2);

            // Multi-asterisk collapse
            let t3: bool = ctx.eval("shExpMatch('hello', '***')").unwrap();
            assert!(t3);
        });
    }

    // ── myIpAddress ───────────────────────────────────────────────────────────

    #[test]
    fn test_my_ip_address() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            let ip: String = ctx.eval("myIpAddress()").unwrap();
            assert_eq!(ip, "127.0.0.1");
        });
    }

    // ── dnsResolve ────────────────────────────────────────────────────────────

    #[test]
    fn test_dns_resolve_stub() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            // The stub returns the hostname unchanged (no actual DNS lookup)
            let result: String = ctx.eval("dnsResolve('example.com')").unwrap();
            assert_eq!(result, "example.com");
        });
    }

    // ── convert_addr ─────────────────────────────────────────────────────────

    #[test]
    fn test_convert_addr() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            // 127.0.0.1 → 127*2^24 + 1 = 2130706433
            let loopback: f64 = ctx.eval("convert_addr('127.0.0.1')").unwrap();
            assert_eq!(loopback as u32, 2130706433u32);

            // 0.0.0.0 → 0
            let zero: f64 = ctx.eval("convert_addr('0.0.0.0')").unwrap();
            assert_eq!(zero as u32, 0);

            // Invalid address → 0
            let invalid: f64 = ctx.eval("convert_addr('not-an-ip')").unwrap();
            assert_eq!(invalid as u32, 0);
        });
    }

    // ── isInNet ───────────────────────────────────────────────────────────────

    #[test]
    fn test_is_in_net() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            // Same /16 subnet
            let t: bool = ctx
                .eval("isInNet('198.95.249.79', '198.95.0.0', '255.255.0.0')")
                .unwrap();
            assert!(t);

            // Different subnet
            let f: bool = ctx
                .eval("isInNet('198.95.249.79', '192.168.0.0', '255.255.0.0')")
                .unwrap();
            assert!(!f);

            // Loopback in /8
            let t2: bool = ctx
                .eval("isInNet('127.0.0.1', '127.0.0.0', '255.0.0.0')")
                .unwrap();
            assert!(t2);

            // Host address with /32
            let t3: bool = ctx
                .eval("isInNet('10.0.0.1', '10.0.0.1', '255.255.255.255')")
                .unwrap();
            assert!(t3);
        });
    }

    // ── time stubs (weekdayRange / dateRange / timeRange) ─────────────────────

    #[test]
    fn test_time_stubs_always_true() {
        let (_rt, ctx) = pac_ctx();
        ctx.with(|ctx| {
            // weekdayRange is always called with strings in PAC scripts.
            let wd: bool = ctx.eval("weekdayRange('MON', 'FRI')").unwrap();
            assert!(wd);

            // dateRange and timeRange are specified to accept numbers in PAC,
            // but the stubs use Rest<String> which requires string args from JS.
            // We test with strings here; the stubs ignore all args and return true.
            let dr: bool = ctx.eval("dateRange('1', '31')").unwrap();
            assert!(dr);

            let tr: bool = ctx.eval("timeRange('0', '23')").unwrap();
            assert!(tr);
        });
    }

    // ── PacEngine::find_proxy (end-to-end) ────────────────────────────────────

    #[tokio::test]
    async fn test_find_proxy_direct() {
        let pac_script = r#"
            function FindProxyForURL(url, host) {
                return "DIRECT";
            }
        "#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), pac_script).unwrap();

        let engine = PacEngine::new(tmp.path().to_str().unwrap()).await.unwrap();
        let result = engine
            .find_proxy("http://example.com/", "example.com")
            .await
            .unwrap();
        assert_eq!(result, "DIRECT");
    }

    #[tokio::test]
    async fn test_find_proxy_conditional_routing() {
        let pac_script = r#"
            function FindProxyForURL(url, host) {
                if (dnsDomainIs(host, '.internal.example.com')) {
                    return "DIRECT";
                }
                return "PROXY upstream:8080";
            }
        "#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), pac_script).unwrap();

        let engine = PacEngine::new(tmp.path().to_str().unwrap()).await.unwrap();

        let direct = engine
            .find_proxy(
                "http://host.internal.example.com/",
                "host.internal.example.com",
            )
            .await
            .unwrap();
        assert_eq!(direct, "DIRECT");

        let proxied = engine
            .find_proxy("http://example.com/", "example.com")
            .await
            .unwrap();
        assert_eq!(proxied, "PROXY upstream:8080");
    }

    #[tokio::test]
    async fn test_find_proxy_uses_sh_exp_match() {
        let pac_script = r#"
            function FindProxyForURL(url, host) {
                if (shExpMatch(url, "http://special.*")) {
                    return "PROXY special-proxy:3128";
                }
                return "DIRECT";
            }
        "#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), pac_script).unwrap();

        let engine = PacEngine::new(tmp.path().to_str().unwrap()).await.unwrap();

        let matched = engine
            .find_proxy("http://special.example.com/path", "special.example.com")
            .await
            .unwrap();
        assert_eq!(matched, "PROXY special-proxy:3128");

        let unmatched = engine
            .find_proxy("https://other.com/", "other.com")
            .await
            .unwrap();
        assert_eq!(unmatched, "DIRECT");
    }

    #[tokio::test]
    async fn test_find_proxy_invalid_script() {
        let pac_script = "this is not valid javascript {{{";
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), pac_script).unwrap();

        // PacEngine::new succeeds (errors are logged, not propagated on init)
        // but find_proxy should fail because FindProxyForURL is not defined.
        let engine = PacEngine::new(tmp.path().to_str().unwrap()).await.unwrap();
        let result = engine
            .find_proxy("http://example.com/", "example.com")
            .await;
        assert!(result.is_err());
    }
}
