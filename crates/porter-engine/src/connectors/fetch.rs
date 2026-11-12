//! Shared capped file download, used for media by the crawl and WXR connectors.
use crate::http;
use anyhow::Result;
use reqwest::{Client, Response};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Default per-file size cap for media downloads.
pub const DEFAULT_MEDIA_CAP: u64 = 200 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;

pub enum Download {
    Ok(Vec<u8>),
    /// Over the cap; carries the declared or seen byte count.
    OverCap(u64),
    /// Non-success HTTP status.
    Status(u16),
    Cancelled,
}

/// Client for large files: follows redirects, 30 s read timeout, no total timeout.
pub fn media_client(user_agent: &str) -> Result<Client> {
    Ok(Client::builder()
        .user_agent(user_agent)
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .read_timeout(Duration::from_secs(30))
        .build()?)
}

/// Reads the body, giving up (`Err(bytes seen or declared)`) once it passes `cap`.
pub async fn read_capped(mut r: Response, cap: u64) -> Result<std::result::Result<Vec<u8>, u64>> {
    if let Some(n) = r.content_length().filter(|n| *n > cap) {
        return Ok(Err(n));
    }
    let mut out = Vec::new();
    while let Some(chunk) = r.chunk().await? {
        out.extend_from_slice(&chunk);
        if out.len() as u64 > cap {
            return Ok(Err(out.len() as u64));
        }
    }
    Ok(Ok(out))
}

/// GET `url` (3 attempts on 429/5xx), capped at `cap` bytes. `Err` is a transport failure.
pub async fn download_capped(
    client: &Client,
    url: &str,
    cap: u64,
    cancel: &CancellationToken,
) -> Result<Download> {
    tokio::select! {
        _ = cancel.cancelled() => Ok(Download::Cancelled),
        r = async {
            let r = http::send_with_retry(client.get(url), 3).await?;
            if !r.status().is_success() {
                return Ok(Download::Status(r.status().as_u16()));
            }
            Ok(match read_capped(r, cap).await? {
                Ok(b) => Download::Ok(b),
                Err(n) => Download::OverCap(n),
            })
        } => r,
    }
}
