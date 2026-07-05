// rt-test-utils: shared test utilities for the P2P remote forwarding suite.

pub mod p2p;

/// Poll an async condition until it returns `true`, or panic after `timeout`.
///
/// The panic message includes the caller's file/line/column, so a test with
/// several polls identifies exactly which one timed out from CI logs alone.
/// The caller location is captured in the synchronous part of the call (before
/// the returned future is awaited), which is why this is a plain `fn` returning
/// a future rather than an `async fn`.
#[track_caller]
pub fn poll_until<F, Fut>(
    mut f: F,
    timeout: std::time::Duration,
) -> impl std::future::Future<Output = ()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let caller = std::panic::Location::caller();
    async move {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if f().await {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("poll_until at {caller} timed out after {timeout:?}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}
