//! ntfy relay: JSON stream subscriber with reconnect backoff, and publishing.
//! Request URLs contain the topic, so they never appear in errors or statuses.

use std::convert::Infallible;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use url::Url;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Until the subscribe response headers arrive.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(15);
/// Wake-up interval while waiting for stream data, to run the dead-stream check.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(15);
/// ntfy sends keepalives every ~45 s. Measured in wall-clock time so a suspend counts.
const DEAD_AFTER: Duration = Duration::from_secs(70);
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(60);
/// A connection that lasted this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// Re-read this many seconds before the last seen server time; dedupe absorbs the overlap.
const SINCE_OVERLAP_SECS: i64 = 5;
const MAX_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RelayStatus {
    Connecting,
    Connected,
    Reconnecting { in_secs: u64, reason: String },
}

pub enum RelayEvent {
    Status(RelayStatus),
    /// Encrypted body of an ntfy `message` event, with the relay's unix time in seconds.
    Message {
        body: String,
        time: i64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("{0}")]
    Http(String),
    #[error("relay answered HTTP {0}")]
    Status(StatusCode),
    #[error("relay closed the connection")]
    Closed,
    #[error("relay did not answer within {} s", REQUEST_TIMEOUT.as_secs())]
    RequestTimeout,
    #[error("no data from relay for {} s", DEAD_AFTER.as_secs())]
    Dead,
    #[error("relay URL cannot take a path")]
    InvalidRelay,
}

impl From<reqwest::Error> for RelayError {
    fn from(e: reqwest::Error) -> Self {
        let e = e.without_url();
        let mut text = e.to_string();
        let mut source = std::error::Error::source(&e);
        while let Some(cause) = source {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        Self::Http(text)
    }
}

#[derive(Deserialize)]
struct NtfyEvent {
    event: String,
    time: i64,
    message: Option<String>,
}

/// Shared client for [`subscribe`] and [`publish`]; no overall timeout so streams can stay open.
pub fn http_client() -> Result<Client, RelayError> {
    Ok(Client::builder().connect_timeout(CONNECT_TIMEOUT).build()?)
}

/// Publishes an encrypted body to `{relay}/{topic}`.
pub async fn publish(
    http: &Client,
    relay: &Url,
    topic: &str,
    body: &str,
) -> Result<(), RelayError> {
    let response = http
        .post(topic_url(relay, topic)?)
        .header("X-Firebase", "no")
        .timeout(PUBLISH_TIMEOUT)
        .body(body.to_owned())
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(RelayError::Status(response.status()));
    }
    Ok(())
}

/// Streams `topic` forever, reconnecting with backoff. Stops when the receiver of `tx` is
/// dropped or the task is aborted (e.g. on re-pair).
pub async fn subscribe(http: Client, relay: Url, topic: String, tx: mpsc::Sender<RelayEvent>) {
    tokio::select! {
        () = tx.closed() => {}
        () = run(&http, &relay, &topic, &tx) => {}
    }
}

async fn run(http: &Client, relay: &Url, topic: &str, tx: &mpsc::Sender<RelayEvent>) {
    let start_secs = unix_secs();
    let mut max_time = None;
    let mut backoff = Duration::ZERO;
    loop {
        // Send errors mean the receiver is gone; `subscribe` then stops this future.
        let _ = tx.send(RelayEvent::Status(RelayStatus::Connecting)).await;
        let mut connected_at = None;
        let Err(error) = stream(
            http,
            relay,
            topic,
            since(max_time, start_secs),
            tx,
            &mut max_time,
            &mut connected_at,
        )
        .await;
        let stable_for = connected_at.map_or(Duration::ZERO, |at: Instant| at.elapsed());
        let rate_limited = matches!(error, RelayError::Status(StatusCode::TOO_MANY_REQUESTS));
        backoff = next_backoff(backoff, stable_for, rate_limited);
        let status = RelayStatus::Reconnecting {
            in_secs: backoff.as_secs(),
            reason: error.to_string(),
        };
        let _ = tx.send(RelayEvent::Status(status)).await;
        sleep(backoff).await;
    }
}

/// One connection; only returns when it ends.
async fn stream(
    http: &Client,
    relay: &Url,
    topic: &str,
    since: i64,
    tx: &mpsc::Sender<RelayEvent>,
    max_time: &mut Option<i64>,
    connected_at: &mut Option<Instant>,
) -> Result<Infallible, RelayError> {
    let request = http.get(subscribe_url(relay, topic, since)?).send();
    // connect_timeout doesn't cover a relay that accepts the connection but never answers.
    let mut response = timeout(REQUEST_TIMEOUT, request)
        .await
        .map_err(|_| RelayError::RequestTimeout)??;
    if !response.status().is_success() {
        return Err(RelayError::Status(response.status()));
    }
    *connected_at = Some(Instant::now());
    let mut lines = LineBuffer::default();
    let mut last_line = SystemTime::now();
    loop {
        if let Ok(chunk) = timeout(CHUNK_TIMEOUT, response.chunk()).await {
            let chunk = chunk?.ok_or(RelayError::Closed)?;
            for line in lines.push(&chunk) {
                last_line = SystemTime::now();
                if let Some(event) = handle_line(&line, max_time) {
                    // Send errors mean the receiver is gone; `subscribe` then stops.
                    let _ = tx.send(event).await;
                }
            }
        }
        let now = SystemTime::now();
        match now.duration_since(last_line) {
            Ok(gap) if gap > DEAD_AFTER => return Err(RelayError::Dead),
            Ok(_) => {}
            // The clock moved backwards; restart the measurement.
            Err(_) => last_line = now,
        }
    }
}

/// Only `message`/`keepalive` times advance `max_time`: `open` carries the current server time,
/// which would skip messages missed while disconnected.
fn handle_line(line: &[u8], max_time: &mut Option<i64>) -> Option<RelayEvent> {
    let event = parse_line(line)?;
    if matches!(event.event.as_str(), "message" | "keepalive") {
        *max_time = Some(max_time.map_or(event.time, |t| t.max(event.time)));
    }
    match (event.event.as_str(), event.message) {
        ("open", _) => Some(RelayEvent::Status(RelayStatus::Connected)),
        ("message", Some(body)) => Some(RelayEvent::Message {
            body,
            time: event.time,
        }),
        _ => None,
    }
}

fn parse_line(line: &[u8]) -> Option<NtfyEvent> {
    serde_json::from_slice(line).ok()
}

/// Splits a byte stream into lines; lines over [`MAX_LINE_BYTES`] are dropped whole.
#[derive(Default)]
struct LineBuffer {
    buf: Vec<u8>,
    skipping: bool,
}

impl LineBuffer {
    fn push(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        let mut lines = Vec::new();
        for part in chunk.split_inclusive(|&b| b == b'\n') {
            let (content, complete) = match part.strip_suffix(b"\n") {
                Some(content) => (content, true),
                None => (part, false),
            };
            if !self.skipping {
                if self.buf.len() + content.len() > MAX_LINE_BYTES {
                    self.skipping = true;
                    self.buf.clear();
                } else {
                    self.buf.extend_from_slice(content);
                }
            }
            if complete {
                if !self.skipping {
                    lines.push(std::mem::take(&mut self.buf));
                }
                self.skipping = false;
            }
        }
        lines
    }
}

fn topic_url(relay: &Url, topic: &str) -> Result<Url, RelayError> {
    let mut url = relay.clone();
    url.path_segments_mut()
        .map_err(|()| RelayError::InvalidRelay)?
        .pop_if_empty()
        .push(topic);
    Ok(url)
}

fn subscribe_url(relay: &Url, topic: &str, since: i64) -> Result<Url, RelayError> {
    let mut url = topic_url(relay, topic)?;
    url.path_segments_mut()
        .map_err(|()| RelayError::InvalidRelay)?
        .push("json");
    url.query_pairs_mut()
        .append_pair("since", &since.to_string());
    Ok(url)
}

/// Last seen server time, or our start time before any event, minus the overlap.
fn since(max_time: Option<i64>, start_secs: i64) -> i64 {
    max_time
        .unwrap_or(start_secs)
        .saturating_sub(SINCE_OVERLAP_SECS)
}

/// Doubles from 1 s up to 60 s; resets after a stable connection; at least 60 s after a 429.
fn next_backoff(prev: Duration, stable_for: Duration, rate_limited: bool) -> Duration {
    let next = if prev.is_zero() || stable_for >= STABLE_AFTER {
        MIN_BACKOFF
    } else {
        (prev * 2).min(MAX_BACKOFF)
    };
    if rate_limited {
        next.max(RATE_LIMIT_BACKOFF)
    } else {
        next
    }
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPIC: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn relay() -> Url {
        Url::parse("https://ntfy.sh").unwrap()
    }

    #[test]
    fn urls_are_built_from_relay_and_topic() {
        assert_eq!(
            topic_url(&relay(), TOPIC).unwrap().as_str(),
            "https://ntfy.sh/AAAAAAAAAAAAAAAAAAAAAA"
        );
        assert_eq!(
            subscribe_url(&relay(), TOPIC, 1_700_000_000)
                .unwrap()
                .as_str(),
            "https://ntfy.sh/AAAAAAAAAAAAAAAAAAAAAA/json?since=1700000000"
        );
        // A topic can never escape its path segment.
        assert_eq!(
            topic_url(&relay(), "a/../b?c").unwrap().as_str(),
            "https://ntfy.sh/a%2F..%2Fb%3Fc"
        );
    }

    #[test]
    fn since_uses_last_server_time_or_start() {
        assert_eq!(since(None, 1000), 995);
        assert_eq!(since(Some(2000), 1000), 1995);
        assert_eq!(since(Some(i64::MIN), 1000), i64::MIN);
    }

    #[test]
    fn backoff_doubles_caps_and_resets() {
        let mut backoff = Duration::ZERO;
        let mut seen = Vec::new();
        for _ in 0..8 {
            backoff = next_backoff(backoff, Duration::ZERO, false);
            seen.push(backoff.as_secs());
        }
        assert_eq!(seen, [1, 2, 4, 8, 16, 32, 60, 60]);
        // Up for less than a minute keeps growing; a stable minute resets.
        assert_eq!(
            next_backoff(Duration::from_secs(4), Duration::from_secs(59), false),
            Duration::from_secs(8)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(60), STABLE_AFTER, false),
            MIN_BACKOFF
        );
    }

    #[test]
    fn rate_limit_waits_at_least_a_minute() {
        assert_eq!(
            next_backoff(Duration::ZERO, Duration::ZERO, true),
            RATE_LIMIT_BACKOFF
        );
        assert_eq!(
            next_backoff(Duration::from_secs(60), STABLE_AFTER, true),
            RATE_LIMIT_BACKOFF
        );
    }

    #[test]
    fn lines_split_across_and_within_chunks() {
        let mut lines = LineBuffer::default();
        assert!(lines.push(b"{\"a\":").is_empty());
        assert_eq!(
            lines.push(b"1}\n{\"b\":2}\n{\"c\""),
            [b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec()]
        );
        assert_eq!(lines.push(b":3}\n"), [b"{\"c\":3}".to_vec()]);
        assert_eq!(lines.push(b"\n"), [Vec::<u8>::new()]);
    }

    #[test]
    fn oversized_lines_are_dropped_and_the_next_line_survives() {
        let mut lines = LineBuffer::default();
        let max = vec![b'x'; MAX_LINE_BYTES];
        assert_eq!(
            lines.push(&[max.as_slice(), b"\n"].concat()),
            [max.as_slice()]
        );

        assert!(lines.push(&max).is_empty());
        assert!(lines.push(b"y").is_empty());
        assert!(lines.push(&max).is_empty());
        assert_eq!(lines.push(b"tail\nok\n"), [b"ok".to_vec()]);
    }

    #[test]
    fn parses_ntfy_events() {
        let event = parse_line(
            br#"{"id":"x","time":1700000000,"event":"message","topic":"t","message":"body","extra":1}"#,
        )
        .unwrap();
        assert_eq!(event.event, "message");
        assert_eq!(event.time, 1_700_000_000);
        assert_eq!(event.message.as_deref(), Some("body"));

        let keepalive = parse_line(br#"{"id":"y","time":5,"event":"keepalive"}"#).unwrap();
        assert_eq!(keepalive.event, "keepalive");
        assert!(keepalive.message.is_none());

        assert!(parse_line(b"").is_none());
        assert!(parse_line(b"not json").is_none());
        assert!(parse_line(br#"{"event":"message","message":"no time"}"#).is_none());
    }

    #[test]
    fn only_message_and_keepalive_advance_the_since_time() {
        let mut max_time = None;
        let open = handle_line(br#"{"id":"o","time":100,"event":"open"}"#, &mut max_time);
        assert!(matches!(
            open,
            Some(RelayEvent::Status(RelayStatus::Connected))
        ));
        assert_eq!(max_time, None);

        let message = handle_line(
            br#"{"id":"m","time":50,"event":"message","message":"body"}"#,
            &mut max_time,
        );
        assert!(
            matches!(message, Some(RelayEvent::Message { ref body, time: 50 }) if body == "body")
        );
        assert_eq!(max_time, Some(50));

        let keepalive = handle_line(
            br#"{"id":"k","time":60,"event":"keepalive"}"#,
            &mut max_time,
        );
        assert!(keepalive.is_none());
        assert_eq!(max_time, Some(60));

        // Older times never move it back; garbage changes nothing.
        handle_line(
            br#"{"id":"k","time":10,"event":"keepalive"}"#,
            &mut max_time,
        );
        assert!(handle_line(b"not json", &mut max_time).is_none());
        assert_eq!(max_time, Some(60));
    }

    #[test]
    fn errors_never_contain_the_url() {
        let url = format!("ftp://relay.example/{TOPIC}");
        let error = tauri::async_runtime::block_on(Client::new().get(url).send()).unwrap_err();
        let text = RelayError::from(error).to_string();
        assert!(!text.contains(TOPIC), "{text}");
    }

    /// Run with `cargo test -- --ignored relay`; uses the public ntfy.sh relay.
    #[test]
    #[ignore = "uses the network"]
    fn relay_round_trip_over_ntfy() {
        tauri::async_runtime::block_on(async {
            let topic = crate::crypto::generate_topic().unwrap();
            let http = http_client().unwrap();
            let (tx, mut rx) = mpsc::channel(16);
            let task =
                tauri::async_runtime::spawn(subscribe(http.clone(), relay(), topic.clone(), tx));
            let next = async |rx: &mut mpsc::Receiver<RelayEvent>| {
                timeout(Duration::from_secs(20), rx.recv())
                    .await
                    .unwrap()
                    .unwrap()
            };
            loop {
                match next(&mut rx).await {
                    RelayEvent::Status(RelayStatus::Connected) => break,
                    RelayEvent::Status(RelayStatus::Reconnecting { reason, .. }) => {
                        panic!("{reason}")
                    }
                    _ => {}
                }
            }
            publish(&http, &relay(), &topic, "ciphertext-body")
                .await
                .unwrap();
            loop {
                if let RelayEvent::Message { body, time } = next(&mut rx).await {
                    assert_eq!(body, "ciphertext-body");
                    assert!(time > 0);
                    break;
                }
            }
            drop(rx);
            timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
        });
    }
}
