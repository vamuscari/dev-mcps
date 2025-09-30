use anyhow::{anyhow, Result};
use tokio::time::{timeout, Duration};

pub async fn with_timeout<F, T>(fut: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    let dur = Duration::from_secs(60);
    match timeout(dur, fut).await {
        Ok(v) => v,
        Err(_) => Err(anyhow!("test timeout after 60s")),
    }
}

