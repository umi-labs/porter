use super::pull::{Pulled, pull};
use super::{Ctx, Hub, clean, hub_error};
use crate::output::{print_json, render};
use anyhow::{Result, anyhow, bail};
use porter_engine::events::Event;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

/// Reconnects after the event stream drops, in a row.
const MAX_RECONNECTS: u32 = 5;

pub async fn run(
    ctx: &Ctx,
    hub: &Hub,
    stages: Vec<String>,
    apply: bool,
    confirm_host: Option<String>,
) -> Result<i32> {
    let confirm_host = match (apply, confirm_host) {
        (true, None) if ctx.interactive && std::io::stdin().is_terminal() => {
            eprint!("--apply writes to the destination; type its host to confirm: ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            Some(line.trim().to_string())
        }
        (true, None) => {
            eprintln!("error: --apply needs --confirm-host <destination host>");
            return Ok(2);
        }
        (_, h) => h,
    };
    let stages = if stages.iter().any(|s| s == "all") {
        json!("all")
    } else {
        json!(stages)
    };
    let mut body = json!({ "stages": stages, "apply": apply });
    if let Some(h) = confirm_host {
        body["confirm_host"] = json!(h);
    }
    let (st, v) = hub.project(Method::POST, &["runs"], Some(&body)).await?;
    if st != StatusCode::CREATED {
        match v.get("run_id").and_then(Value::as_str) {
            Some(id) => bail!("{} (run {id})", hub_error(st, &v)),
            None => bail!("{}", hub_error(st, &v)),
        }
    }
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("the hub's run response has no id"))?
        .to_string();
    if ctx.json {
        print_json(&json!({"v": 1, "event": "hub-run-queued", "run": v}));
    } else {
        eprintln!(
            "queued run {id} on v{}",
            v.get("base_version").and_then(Value::as_u64).unwrap_or(0)
        );
    }
    let end = stream(ctx, hub, &id).await?;
    let Some(end) = end else {
        eprintln!("stopped watching run {id}; it is still being cancelled on the hub");
        return Ok(3);
    };
    let state = end.get("state").and_then(Value::as_str).unwrap_or("");
    let exit = end.get("exit_code").and_then(Value::as_i64);
    let code = match state {
        "finished" if exit == Some(2) => 2,
        "finished" => 0,
        "cancelled" => 3,
        _ => 1,
    };
    if ctx.json {
        print_json(&json!({"v": 1, "event": "hub-run-finished", "run": end}));
    } else {
        match end.get("error").and_then(Value::as_str) {
            Some(e) => eprintln!("run {state}: {e}"),
            None => eprintln!("run {state}"),
        }
    }
    if let Some(n) = end.get("result_version").and_then(Value::as_u64) {
        // Ctrl-C still works here (our SIGINT handler replaced the default). The pull's
        // swap has no await inside, so it can't be cut off halfway.
        let pulled = tokio::select! {
            r = pull(ctx, hub, Some(n), false) => r,
            _ = tokio::signal::ctrl_c() => {
                eprintln!("interrupted; the result v{n} was not pulled: run `porter hub pull --version {n}`");
                return Ok(3);
            }
        };
        match pulled {
            Ok(Pulled::Done) => {}
            Ok(Pulled::Interrupted) => {
                eprintln!(
                    "interrupted; the result v{n} was not pulled: run `porter hub pull --version {n}`"
                );
                return Ok(3);
            }
            Ok(Pulled::Refused(why)) => eprintln!(
                "not pulling the result v{n}: {why}. Pull it with `porter hub pull --version {n}` (add --force to overwrite local changes)"
            ),
            Err(e) => eprintln!(
                "error: pulling the result v{n} failed: {e:#}; retry with `porter hub pull --version {n}`"
            ),
        }
    }
    Ok(code)
}

/// Streams `/runs/<id>/events` until the `end` event and returns its Run JSON, or `None`
/// on a second Ctrl-C. The first Ctrl-C asks the hub to cancel and keeps streaming.
async fn stream(ctx: &Ctx, hub: &Hub, id: &str) -> Result<Option<Value>> {
    let mut last_id: Option<String> = None;
    let mut retry = Duration::from_millis(2000);
    let mut drops = 0u32;
    let mut cancelled = false;
    let mut sigint = Box::pin(tokio::signal::ctrl_c());
    loop {
        let mut rb = hub
            .request(
                Method::GET,
                &["api", "projects", &hub.slug, "runs", id, "events"],
                None,
            )
            .header("accept", "text/event-stream");
        if let Some(l) = &last_id {
            rb = rb.header("last-event-id", l.as_str());
        }
        let sent = tokio::select! {
            r = rb.send() => r,
            _ = &mut sigint => {
                if cancelled { return Ok(None); }
                cancelled = true;
                cancel(hub, id).await;
                sigint = Box::pin(tokio::signal::ctrl_c());
                continue;
            }
        };
        match sent {
            Ok(mut resp) if resp.status() == StatusCode::OK => {
                let mut sse = Sse::default();
                loop {
                    tokio::select! {
                        chunk = resp.chunk() => match chunk {
                            Ok(Some(b)) => {
                                for m in sse.feed(&b) {
                                    drops = 0;
                                    if let Some(r) = m.retry { retry = Duration::from_millis(r); }
                                    if m.event.as_deref() == Some("end") {
                                        return Ok(Some(serde_json::from_str(&m.data)
                                            .map_err(|e| anyhow!("the hub's end event is not JSON: {e}"))?));
                                    }
                                    if m.id.is_some() { last_id = m.id.clone(); }
                                    if !m.data.is_empty() { show(ctx.json, &m.data); }
                                }
                            }
                            Ok(None) | Err(_) => break,
                        },
                        _ = &mut sigint => {
                            if cancelled { return Ok(None); }
                            cancelled = true;
                            cancel(hub, id).await;
                            sigint = Box::pin(tokio::signal::ctrl_c());
                        }
                    }
                }
            }
            // A proxy's 5xx or 429 during a hub deploy counts as a drop.
            Ok(resp)
                if resp.status().is_server_error()
                    || resp.status() == StatusCode::TOO_MANY_REQUESTS => {}
            Ok(resp) => {
                let st = resp.status();
                let v: Value = resp.json().await.unwrap_or(Value::Null);
                bail!("{}", hub_error(st, &v));
            }
            Err(_) => {}
        }
        drops += 1;
        if drops > MAX_RECONNECTS {
            bail!("lost the event stream for run {id}; it carries on on the hub");
        }
        tracing::info!(drops, "event stream dropped; reconnecting");
        tokio::time::sleep(retry).await;
    }
}

async fn cancel(hub: &Hub, id: &str) {
    eprintln!("cancelling run {id} (Ctrl-C again to stop watching)");
    match hub
        .project(Method::POST, &["runs", id, "cancel"], None)
        .await
    {
        Ok((StatusCode::OK, _)) => {}
        Ok((st, v)) => eprintln!("warning: cancel: {}", hub_error(st, &v)),
        Err(e) => eprintln!("warning: cancel: {e:#}"),
    }
}

/// `--json`: the line verbatim. Otherwise porter events as a local run renders them,
/// hub events as short notes.
fn show(json: bool, data: &str) {
    if json {
        print_json(&as_event(data));
        return;
    }
    if let Some(e) = Event::from_json(data) {
        eprintln!("{}", clean(&render(&e)));
        return;
    }
    let v = as_event(data);
    let s = |k: &str| clean(v.get(k).and_then(Value::as_str).unwrap_or_default());
    match s("event").as_str() {
        "hub-note" => eprintln!("hub: {}", s("message")),
        "hub-stderr" => eprintln!("porter stderr:\n{}", s("text")),
        "hub-malformed" => eprintln!("hub: malformed line: {}", s("line")),
        "hub-result" => eprintln!(
            "hub: result saved as v{}",
            v.get("version").and_then(Value::as_u64).unwrap_or(0)
        ),
        _ => eprintln!("{}", clean(&v.to_string())),
    }
}

/// One stream message as a v1 event: a JSON object gets `"v": 1` if it lacks it
/// (the hub's own events); anything else becomes `hub-malformed`.
fn as_event(data: &str) -> Value {
    match serde_json::from_str::<Value>(data) {
        Ok(Value::Object(m)) if m.get("event").is_some_and(Value::is_string) => {
            if m.contains_key("v") {
                return Value::Object(m);
            }
            let mut out = serde_json::Map::new();
            out.insert("v".into(), json!(1));
            out.extend(m);
            Value::Object(out)
        }
        _ => json!({"v": 1, "event": "hub-malformed", "line": data}),
    }
}

#[derive(Default, Debug, PartialEq)]
struct Msg {
    id: Option<String>,
    event: Option<String>,
    data: String,
    retry: Option<u64>,
}

/// A minimal server-sent-events parser (enough for the hub's stream).
#[derive(Default)]
struct Sse {
    buf: Vec<u8>,
}

impl Sse {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Msg> {
        // Split on bytes so a UTF-8 character cut across chunks stays whole. JSON data
        // lines never carry a raw CR, so dropping CRs turns CRLF into LF.
        self.buf.extend(bytes.iter().filter(|&&b| b != b'\r'));
        let mut out = Vec::new();
        while let Some(end) = self.buf.windows(2).position(|w| w == b"\n\n") {
            let block = String::from_utf8_lossy(&self.buf[..end]).into_owned();
            self.buf.drain(..end + 2);
            let mut m = Msg::default();
            let mut data: Vec<&str> = Vec::new();
            for line in block.lines() {
                if line.starts_with(':') {
                    continue;
                }
                let (k, val) = line.split_once(':').unwrap_or((line, ""));
                let val = val.strip_prefix(' ').unwrap_or(val);
                match k {
                    "id" => m.id = Some(val.to_string()),
                    "event" => m.event = Some(val.to_string()),
                    "data" => data.push(val),
                    "retry" => m.retry = val.parse().ok(),
                    _ => {}
                }
            }
            m.data = data.join("\n");
            if m.id.is_some() || m.event.is_some() || !m.data.is_empty() || m.retry.is_some() {
                out.push(m);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parses_split_chunks() {
        let mut p = Sse::default();
        assert!(p.feed(b"retry: 2000\n\nid: 1\nda").len() == 1);
        let m = p.feed(b"ta: {\"a\":1}\n\n: ping\n\nevent: end\ndata: {}\n\n");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].id.as_deref(), Some("1"));
        assert_eq!(m[0].data, "{\"a\":1}");
        assert_eq!(m[1].event.as_deref(), Some("end"));
    }
}
