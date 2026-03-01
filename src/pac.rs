use anyhow::{Context as AnyhowContext, Result};
use boa_engine::string::JsString;
use boa_engine::{Context, JsValue, NativeFunction, Source};
use glob::Pattern;
use log::error;
use std::fs;
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

impl PacEngine {
    pub async fn new(pac_url_or_path: &str) -> Result<Self> {
        let script = if pac_url_or_path.starts_with("http") {
            let client = reqwest::Client::new();
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

            thread::spawn(move || {
                let mut context = Context::default();

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
                    NativeFunction::from_fn_ptr(|_, _, _| {
                        Ok(JsValue::from(JsString::from("127.0.0.1")))
                    }),
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

                if let Err(e) = context.eval(Source::from_bytes(&script_clone)) {
                    error!("Failed to evaluate PAC script: {}", e);
                }

                while let Some(req) = rx.blocking_recv() {
                    let global_obj = context.global_object();

                    let result = (|| -> Result<String> {
                        let func_name = JsString::from("FindProxyForURL");
                        let func = global_obj
                            .get(func_name, &mut context)
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
                            .ok_or_else(|| anyhow::anyhow!("FindProxyForURL returned non-string"))
                    })();

                    let _ = req.respond_to.send(result);
                }
            });
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
    use super::glob_match;

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
