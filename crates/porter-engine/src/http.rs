use anyhow::{Result, anyhow};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use std::time::Duration;

const BASE: Duration = Duration::from_millis(500);
const CAP: Duration = Duration::from_secs(30);

pub fn client(user_agent: &str) -> Client {
    Client::builder()
        .user_agent(user_agent)
        .build()
        // Only fails if the TLS backend cannot initialise; nothing sensible to do then.
        .unwrap_or_default()
}

pub fn retryable(s: StatusCode) -> bool {
    s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error()
}

/// How long to wait after failed attempt `attempt` (1-based): the response's
/// `Retry-After` seconds if any, else 500 ms exponential backoff; capped at 30 s.
pub fn retry_wait(attempt: u32, res: Option<&Response>) -> Duration {
    let after = res
        .and_then(|r| r.headers().get(reqwest::header::RETRY_AFTER))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let backoff = BASE * 2u32.saturating_pow(attempt.saturating_sub(1));
    after.unwrap_or(backoff).min(CAP)
}

/// Sends `req`, retrying 429/5xx and transport errors up to `max` attempts in all
/// (500 ms exponential backoff, capped at 30 s, `Retry-After` seconds honoured).
/// The last response is returned as-is, so callers still check its status.
/// Streaming bodies can't be cloned and so get a single attempt.
pub async fn send_with_retry(req: RequestBuilder, max: u32) -> Result<Response> {
    let max = max.max(1);
    let mut req = Some(req);
    for attempt in 1..=max {
        let this = req.take().ok_or_else(|| anyhow!("request consumed"))?;
        let next = if attempt < max {
            this.try_clone()
        } else {
            None
        };
        let result = this.send().await;
        let Some(next) = next else {
            return Ok(result?);
        };
        let wait = match &result {
            Ok(r) if retryable(r.status()) => retry_wait(attempt, Some(r)),
            Ok(_) => return result.map_err(Into::into),
            Err(_) => retry_wait(attempt, None),
        };
        tokio::time::sleep(wait).await;
        req = Some(next);
    }
    Err(anyhow!("send_with_retry: no attempts made"))
}
