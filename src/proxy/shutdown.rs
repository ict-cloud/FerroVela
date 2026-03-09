use async_trait::async_trait;
use pingora::server::{ShutdownSignal, ShutdownSignalWatch};
use tokio::sync::watch;

pub struct WatchShutdownSignal {
    pub receiver: watch::Receiver<bool>,
}

#[async_trait]
impl ShutdownSignalWatch for WatchShutdownSignal {
    async fn recv(&self) -> ShutdownSignal {
        let mut rx = self.receiver.clone();
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return ShutdownSignal::FastShutdown;
            }
        }
        ShutdownSignal::FastShutdown
    }
}
