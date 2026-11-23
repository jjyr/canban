//! Pure asynchronous Tokio push subscription stream (`app.subscribe`) over `canban-transport`.
use anyhow::{Context, Result, bail};
use canban_transport::{
    ClientTransport, MAX_FRAME_BYTES, TransportStream, read_bounded_line, write_json_line_async,
};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum PushFrame {
    Heartbeat,
    StateChanged {
        topic: &'static str,
        card_id: Option<String>,
        marker: Value,
    },
    UpdateChanged(Value),
    UtilitiesChanged(Value),
    UsageChanged(Value),
    HerdrInstall(Value),
}

#[derive(Clone, Debug)]
pub struct SubscribeHandshake {
    pub changed: bool,
    pub marker: Value,
    pub result: Value,
}

pub fn state_marker(state: &Value) -> Value {
    json!({"revision": state["revision"], "github_revision": state["github_revision"]})
}

pub fn change_topic(connected: bool, previous: &Value, next: &Value) -> &'static str {
    if !connected {
        "resync"
    } else if previous["github_revision"] != next["github_revision"] {
        "github"
    } else {
        "application"
    }
}

fn static_event_topic(raw: Option<&str>, previous: &Value, params: &Value) -> &'static str {
    match raw {
        Some("resync") => "resync",
        Some("github") => "github",
        Some("review") => "review",
        Some("configuration") => "configuration",
        Some("desktop") => "desktop",
        Some("schedules") => "schedules",
        Some("terminals") => "terminals",
        Some("reclaim") => "reclaim",
        Some("runtime") => "runtime",
        Some("worktrees") => "worktrees",
        Some("execution") => "execution",
        Some("host") => "host",
        Some("application") => "application",
        _ => change_topic(true, previous, params),
    }
}

pub struct AsyncSubscribeStream {
    reader: tokio::io::BufReader<tokio::io::ReadHalf<TransportStream>>,
    _writer: tokio::io::WriteHalf<TransportStream>,
    heartbeat_ms: u64,
    last_marker: Value,
}

impl AsyncSubscribeStream {
    pub fn read_timeout(&self) -> Duration {
        Duration::from_millis(self.heartbeat_ms.saturating_mul(3).max(500))
    }

    pub async fn next_frame(&mut self) -> Result<Option<PushFrame>> {
        loop {
            let line_opt = match tokio::time::timeout(
                self.read_timeout(),
                read_bounded_line(&mut self.reader, MAX_FRAME_BYTES),
            )
            .await
            {
                Ok(Ok(opt)) => opt,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "app.subscribe heartbeat timed out",
                    )
                    .into());
                }
            };
            let Some(line) = line_opt else {
                return Ok(None);
            };
            let mut msg: Value = serde_json::from_str(&line)?;
            match msg["method"].as_str() {
                Some("heartbeat") => return Ok(Some(PushFrame::Heartbeat)),
                Some("event") => {
                    let params = &msg["params"];
                    let topic =
                        static_event_topic(params["topic"].as_str(), &self.last_marker, params);
                    let card_id = params["card_id"].as_str().map(str::to_owned);
                    let marker = state_marker(params);
                    self.last_marker = marker.clone();
                    return Ok(Some(PushFrame::StateChanged {
                        topic,
                        card_id,
                        marker,
                    }));
                }
                Some("update.changed") => {
                    return Ok(Some(PushFrame::UpdateChanged(
                        msg["params"]["state"].take(),
                    )));
                }
                Some("utilities.changed") => {
                    return Ok(Some(PushFrame::UtilitiesChanged(
                        msg["params"]["state"].take(),
                    )));
                }
                Some("usage.changed") => {
                    return Ok(Some(PushFrame::UsageChanged(msg["params"]["state"].take())));
                }
                Some("herdr.install") => {
                    return Ok(Some(PushFrame::HerdrInstall(msg["params"]["state"].take())));
                }
                _ => continue,
            }
        }
    }
}

/// Opens an asynchronous persistent `app.subscribe` stream on `transport`.
pub async fn open_async_subscribe(
    transport: &ClientTransport,
    after: &Value,
    topics: &[&str],
    heartbeat_ms: Option<u64>,
) -> Result<(AsyncSubscribeStream, SubscribeHandshake)> {
    let (stream, auth_token) = transport.open_async_stream().await?;
    let (read_half, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut params = json!({
        "after": after,
        "topics": topics,
    });
    if let Some(ms) = heartbeat_ms {
        params["heartbeat_ms"] = json!(ms);
    }
    let mut req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "app.subscribe",
        "params": params,
    });
    if let Some(token) = auth_token {
        req["auth"] = json!(token);
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        write_json_line_async(&mut writer, &req),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Timed out sending app.subscribe request",
        )
    })??;

    let line_opt = tokio::time::timeout(
        Duration::from_secs(15),
        read_bounded_line(&mut reader, MAX_FRAME_BYTES),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Timed out waiting for app.subscribe handshake",
        )
    })??;
    let Some(line) = line_opt else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "Server closed connection during app.subscribe handshake",
        )
        .into());
    };
    let reply: Value = serde_json::from_str(&line)?;
    if let Some(err) = reply.get("error") {
        let code = err["code"].as_i64().unwrap_or(-32000);
        let msg = err["message"].as_str().unwrap_or("app.subscribe failed");
        if code == -32001 {
            bail!("auth_failed: {msg}");
        }
        bail!("{msg}");
    }
    let result = reply
        .get("result")
        .cloned()
        .context("Invalid app.subscribe response")?;
    let marker = state_marker(&result);
    let changed = result["changed"] == true;
    let hb_ms = result["heartbeat_ms"].as_u64().unwrap_or(15_000);
    Ok((
        AsyncSubscribeStream {
            reader,
            _writer: writer,
            heartbeat_ms: hb_ms,
            last_marker: marker.clone(),
        },
        SubscribeHandshake {
            changed,
            marker,
            result,
        },
    ))
}
