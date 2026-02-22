use anyhow::{Context as AnyhowContext, Result};
use boa_engine::string::JsString;
use boa_engine::{Context, JsValue, NativeFunction, Source};
use log::error;
use std::fs;
use std::thread;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct PacEngine {
    sender: mpsc::Sender<PacRequest>,
}

struct PacRequest {
    url: String,
    host: String,
    respond_to: oneshot::Sender<Result<String>>,
}

impl PacEngine {
    pub fn new(pac_url_or_path: &str) -> Result<Self> {
        let script = if pac_url_or_path.starts_with("http") {
            // For HTTP URLs, we need to handle async in a blocking context
            // Using a runtime inside the new() function
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let client = reqwest::Client::new();
                client.get(pac_url_or_path).send().await?.text().await
            })
            .context("Failed to fetch PAC file")?
        } else {
            fs::read_to_string(pac_url_or_path).context("Failed to read PAC file")?
        };

        let (tx, mut rx) = mpsc::channel::<PacRequest>(32);

        thread::spawn(move || {
            let mut context = Context::default();

            let _ = context.register_global_callable(
                JsString::from("dnsResolve"),
                1,
                NativeFunction::from_fn_ptr(|_, args, _| {
                    let host = args
                        .get(0)
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
                    let str = args
                        .get(0)
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    let pattern = args
                        .get(1)
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();

                    // Convert glob pattern to regex
                    let regex_pattern = pattern
                        .replace(".", "\\.")
                        .replace("*", ".*")
                        .replace("?", ".");

                    // Try to compile regex
                    match regex::Regex::new(&regex_pattern) {
                        Ok(re) => Ok(JsValue::from(re.is_match(&str))),
                        Err(e) => Err(boa_engine::JsError::from_opaque(JsValue::from(
                            JsString::from(e.to_string()),
                        ))),
                    }
                }),
            );

            if let Err(e) = context.eval(Source::from_bytes(&script)) {
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

        Ok(PacEngine { sender: tx })
    }

    pub async fn find_proxy(&self, url: &str, host: &str) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        let req = PacRequest {
            url: url.to_string(),
            host: host.to_string(),
            respond_to: tx,
        };

        self.sender
            .send(req)
            .await
            .map_err(|_| anyhow::anyhow!("PAC thread dead"))?;
        match rx.await {
            Ok(res) => res,
            Err(_) => Err(anyhow::anyhow!("PAC thread dropped channel")),
        }
    }
}
