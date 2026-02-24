use anyhow::{Context as AnyhowContext, Result};
use boa_engine::string::JsString;
use boa_engine::{Context, JsValue, NativeFunction, Source};
use log::error;
use tokio::fs;
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
    let p_chars: Vec<char> = pattern.chars().collect();
    let t_chars: Vec<char> = text.chars().collect();

    let mut p_idx = 0;
    let mut t_idx = 0;
    let mut star_idx = None;
    let mut match_idx = 0;

    while t_idx < t_chars.len() {
        if p_idx < p_chars.len() && (p_chars[p_idx] == '?' || p_chars[p_idx] == t_chars[t_idx]) {
            p_idx += 1;
            t_idx += 1;
        } else if p_idx < p_chars.len() && p_chars[p_idx] == '*' {
            star_idx = Some(p_idx);
            match_idx = t_idx;
            p_idx += 1;
        } else if let Some(star) = star_idx {
            p_idx = star + 1;
            match_idx += 1;
            t_idx = match_idx;
        } else {
            return false;
        }
    }

    while p_idx < p_chars.len() && p_chars[p_idx] == '*' {
        p_idx += 1;
    }

    p_idx == p_chars.len()
}

impl PacEngine {
    pub async fn new(pac_url_or_path: &str) -> Result<Self> {
        let script = Arc::new(if pac_url_or_path.starts_with("http") {
            let client = reqwest::Client::new();
            client
                .get(pac_url_or_path)
                .send()
                .await?
                .text()
                .await
                .context("Failed to fetch PAC file")?
        } else {
            fs::read_to_string(pac_url_or_path)
                .await
                .context("Failed to read PAC file")?
        });

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

                if let Err(e) = context.eval(Source::from_bytes(script_clone.as_bytes())) {
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
