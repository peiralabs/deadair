use crate::verdict::{Level, Observation, Verdict};
use std::fmt::Display;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Longest request line accepted, so a malformed client cannot exhaust memory.
const MAX_REQUEST_LINE: u64 = 8 * 1024;

/// Latest evaluation plus the observation behind it, shared with the metrics server.
pub struct Snapshot {
    /// Most recent verdict.
    pub verdict: Verdict,
    /// Observation the verdict was computed from, absent before the first sample.
    pub observation: Option<Observation>,
    /// Process start time as a Unix timestamp.
    pub started: u64,
}

/// Serves Prometheus metrics and health status one connection at a time.
pub fn serve(addr: &str, state: Arc<Mutex<Snapshot>>) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    for stream in listener.incoming().flatten() {
        // A single misbehaving scraper must never take the metrics endpoint down with it.
        if let Err(error) = handle(stream, &state) {
            eprintln!("metrics connection: {error}");
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, state: &Mutex<Snapshot>) -> std::io::Result<()> {
    let mut request_line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST_LINE)).read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let route = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("GET"), Some(path), Some(_), None) => path,
        _ => "",
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Render while holding the lock, then release it before writing: a slow client must
    // not block the sampling loop from publishing its next verdict.
    let (status, content_type, body) = {
        let snapshot = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match route {
            "/metrics" => (
                "200 OK",
                "text/plain; version=0.0.4",
                metrics(&snapshot, now),
            ),
            "/healthz" if snapshot.verdict.level == Level::Fail => {
                ("503 Service Unavailable", "text/plain", "fail\n".into())
            }
            "/healthz" => ("200 OK", "text/plain", "ok\n".into()),
            _ => ("404 Not Found", "text/plain", "not found\n".into()),
        }
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn number(level: Level) -> u8 {
    match level {
        Level::Ok => 0,
        Level::Warn => 1,
        Level::Fail => 2,
    }
}

/// Writes one metric family: help, type, then its single sample.
fn family(output: &mut String, name: &str, help: &str, kind: &str, value: impl Display) {
    output.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}\n"
    ));
}

fn metrics(snapshot: &Snapshot, now: u64) -> String {
    let Snapshot {
        verdict,
        observation,
        started,
    } = snapshot;
    let mut output = String::new();
    // Check names are a fixed set of bare identifiers, so no label escaping is required.
    output.push_str("# HELP deadair_check Health check severity.\n# TYPE deadair_check gauge\n");
    for item in &verdict.checks {
        output.push_str(&format!(
            "deadair_check{{check=\"{}\"}} {}\n",
            item.name,
            number(item.level)
        ));
    }
    family(
        &mut output,
        "deadair_level",
        "Overall health severity.",
        "gauge",
        number(verdict.level),
    );
    // Ports and the byte counter are omitted entirely when unknown: emitting a zero would
    // look like a real reading, and a fake counter reset distorts rate().
    if let Some(port) = observation.as_ref().and_then(|item| item.forwarded_port) {
        family(
            &mut output,
            "deadair_forwarded_port",
            "VPN forwarded port.",
            "gauge",
            port,
        );
    }
    if let Some(port) = observation.as_ref().and_then(|item| item.listen_port) {
        family(
            &mut output,
            "deadair_listen_port",
            "qBittorrent listening port.",
            "gauge",
            port,
        );
    }
    if let Some(bytes) = observation.as_ref().and_then(|item| item.downloaded_bytes) {
        family(
            &mut output,
            "deadair_downloaded_bytes_total",
            "Bytes downloaded in the current qBittorrent session.",
            "counter",
            bytes,
        );
    }
    family(
        &mut output,
        "deadair_wanting_torrents",
        "Torrents wanting data.",
        "gauge",
        observation.as_ref().map_or(0, |item| item.wanting),
    );
    family(
        &mut output,
        "deadair_seedless_torrents",
        "Wanting torrents with no seeds.",
        "gauge",
        observation.as_ref().map_or(0, |item| item.seedless),
    );
    family(
        &mut output,
        "deadair_last_evaluation_timestamp_seconds",
        "Last evaluation Unix timestamp.",
        "gauge",
        observation.as_ref().map_or(0, |item| item.at),
    );
    family(
        &mut output,
        "deadair_uptime_seconds",
        "Process uptime in seconds.",
        "gauge",
        now.saturating_sub(*started),
    );
    output
}
