// rt-test-utils: shared test utilities for the P2P remote forwarding suite.

pub mod p2p;

/// Poll an async condition until it returns `true`, or panic after `timeout`.
pub async fn poll_until<F, Fut>(mut f: F, timeout: std::time::Duration)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if f().await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("poll_until timed out after {:?}", timeout);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
