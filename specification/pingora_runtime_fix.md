# Pingora Runtime Fix Specification

## Issue
When starting the proxy service via the GUI, a panic occurs with the message:
`Cannot start a runtime from within a runtime. This happens because a function (like block_on) attempted to block the current thread while the thread is being used to drive asynchronous tasks.`

This happens because the UI component `ConfigEditor::handle_toggle_service` spawns a `tokio::spawn` task to handle proxy initialization, and within that task, it was calling `Proxy::run().await`, which internally called `my_server.run_forever()`. Pingora's `run_forever()` method spins up its own internal Tokio runtime and invokes `block_on`. Because `tokio::spawn` executes within the existing application Tokio runtime, creating a new one via `block_on` triggered the panic. Furthermore, `run_forever()` terminates the entire process (`std::process::exit(0)`) after execution, which is undesired for a GUI application.

## Solution

1. **Avoid Tokio Runtime Conflict**
   Instead of running the Pingora server setup inside the `tokio::spawn` future, we moved the actual `proxy.run()` execution into a dedicated OS thread using `std::thread::spawn`. The async PAC resolution is still handled asynchronously by `tokio::spawn` to avoid blocking the UI thread, and once initialized, the proxy server itself is launched in the new thread.

2. **Refactor `Proxy::run` Signature**
   `Proxy::run` was changed from `pub async fn run(&self)` to `pub fn run(&self)` since the internal `my_server.run(...)` handles its own blocking execution.

3. **Graceful Shutdown Mechanism**
   Pingora's `run_forever()` was replaced with `my_server.run(run_args)`. This allows the application to stay alive after the server stops. To allow the UI to stop the proxy service on demand, we introduced a `tokio::sync::watch::channel(false)`. The proxy struct now takes an optional `shutdown_rx: Option<tokio::sync::watch::Receiver<bool>>`.

   A new module `src/proxy/shutdown.rs` was created to implement Pingora's `ShutdownSignalWatch` trait. It listens for changes on the watch receiver. When the UI toggles the service off, it sends `true` over the channel, which triggers a `ShutdownSignal::FastShutdown` for the Pingora server, cleanly terminating the proxy without bringing down the UI.
