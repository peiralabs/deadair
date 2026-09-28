use crate::config::Thresholds;
use serde::Serialize;
use std::net::IpAddr;

/// One point-in-time sample of the monitored stack.
#[derive(Clone)]
pub struct Observation {
    /// Unix timestamp in seconds.
    pub at: u64,
    /// Port forwarded by the VPN provider.
    pub forwarded_port: Option<u16>,
    /// Public IP reported by gluetun.
    pub vpn_public_ip: Option<String>,
    /// Port on which qBittorrent is listening.
    pub listen_port: Option<u16>,
    /// qBittorrent connection status.
    pub connection_status: Option<String>,
    /// Bytes downloaded in the current qBittorrent session.
    pub downloaded_bytes: Option<u64>,
    /// Number of torrents wanting to download.
    pub wanting: usize,
    /// Advisory number of wanting torrents with no complete peers.
    pub seedless: usize,
    /// Human-readable probe failures.
    pub errors: Vec<String>,
}

/// Severity of a check or complete verdict.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// The check passes.
    Ok,
    /// The check is uncertain or awaiting confirmation.
    Warn,
    /// The check fails.
    Fail,
}

/// One independently actionable health check.
#[derive(Clone, Serialize)]
pub struct Check {
    /// Stable check name.
    pub name: &'static str,
    /// Check severity.
    pub level: Level,
    /// Human-readable evidence.
    pub detail: String,
    /// Suggested recovery action, empty for passing checks.
    pub remedy: &'static str,
}

/// Aggregate result of evaluating observation history.
#[derive(Clone, Serialize)]
pub struct Verdict {
    /// Highest severity among all checks.
    pub level: Level,
    /// Checks in stable display order.
    pub checks: Vec<Check>,
}

const PROBE_REMEDY: &str = "deadair could not reach gluetun or qBittorrent. Check the URLs and credentials in DEADAIR_GLUETUN_URL / DEADAIR_QBT_URL.";
const TUNNEL_REMEDY: &str = "gluetun reports no public VPN IP: the tunnel is down and traffic may be leaving over your WAN. Restart gluetun and confirm /tmp/gluetun/ip is fresh.";
const PORT_REMEDY: &str = "qBittorrent is not listening on the port gluetun forwarded, so no incoming peers can reach it. Re-run your port sync, or set listen_port to the forwarded port.";
const FIREWALLED_REMEDY: &str = "The forwarded port is not actually reachable from outside. Restart gluetun, then restart every container that shares its network namespace (qBittorrent, Prowlarr) — they keep stale routes otherwise — then re-sync the port.";
const DISCONNECTED_REMEDY: &str = "qBittorrent is not connected to the network at all. Check that its container is running and still attached to gluetun's namespace.";
const TRAFFIC_REMEDY: &str = "Torrents are waiting on data and nothing has moved. Check the forwarded port first — a silently dead port forward is the most common cause of this.";

fn check(name: &'static str, level: Level, detail: String, remedy: &'static str) -> Check {
    Check {
        name,
        level,
        detail,
        remedy: if level == Level::Ok { "" } else { remedy },
    }
}

fn confirmed<F>(history: &[Observation], count: usize, evaluate: F) -> Check
where
    F: Fn(&Observation) -> Check,
{
    let mut newest = evaluate(history.last().expect("nonempty history"));
    let needed = count.max(1);
    if newest.level == Level::Fail
        && (history.len() < needed
            || !history[history.len() - needed..]
                .iter()
                .all(|observation| evaluate(observation).level == Level::Fail))
    {
        newest.level = Level::Warn;
    }
    newest
}

fn probe(observation: &Observation) -> Check {
    if observation.errors.is_empty() {
        check("probe", Level::Ok, "all probes succeeded".into(), "")
    } else {
        check(
            "probe",
            Level::Fail,
            observation.errors.join("; "),
            PROBE_REMEDY,
        )
    }
}

fn tunnel(observation: &Observation) -> Check {
    match observation.vpn_public_ip.as_deref() {
        Some(ip) if is_public_ip(ip) => check("tunnel", Level::Ok, ip.into(), ""),
        Some(ip) => check(
            "tunnel",
            Level::Fail,
            format!("non-public VPN IP: {ip}"),
            TUNNEL_REMEDY,
        ),
        None => check(
            "tunnel",
            Level::Fail,
            "VPN public IP unavailable".into(),
            TUNNEL_REMEDY,
        ),
    }
}

fn port_agreement(observation: &Observation) -> Check {
    match (observation.forwarded_port, observation.listen_port) {
        (Some(forwarded), Some(listen)) if forwarded == listen => check(
            "port_agreement",
            Level::Ok,
            format!("both use port {forwarded}"),
            "",
        ),
        (Some(forwarded), Some(listen)) => check(
            "port_agreement",
            Level::Fail,
            format!("forwarded {forwarded}, listening {listen}"),
            PORT_REMEDY,
        ),
        _ => check(
            "port_agreement",
            Level::Warn,
            "forwarded or listening port unavailable".into(),
            PORT_REMEDY,
        ),
    }
}

fn reachability(observation: &Observation) -> Check {
    match observation.connection_status.as_deref() {
        Some("connected") => check("reachability", Level::Ok, "connected".into(), ""),
        Some("firewalled") => check(
            "reachability",
            Level::Fail,
            "firewalled".into(),
            FIREWALLED_REMEDY,
        ),
        Some("disconnected") => check(
            "reachability",
            Level::Fail,
            "disconnected".into(),
            DISCONNECTED_REMEDY,
        ),
        Some(status) => check(
            "reachability",
            Level::Warn,
            format!("unknown status: {status}"),
            DISCONNECTED_REMEDY,
        ),
        None => check(
            "reachability",
            Level::Warn,
            "connection status unavailable".into(),
            DISCONNECTED_REMEDY,
        ),
    }
}

fn traffic(history: &[Observation], thresholds: &Thresholds) -> Check {
    let newest = history.last().expect("nonempty history");
    let advisory = if newest.seedless > 0 {
        format!("; {} torrents have no seeds", newest.seedless)
    } else {
        String::new()
    };
    if newest.wanting == 0 {
        return check(
            "traffic",
            Level::Ok,
            format!("idle: no torrent is waiting on data{advisory}"),
            "",
        );
    }
    // The baseline is the *newest* sample at least `stall_window` old, so the comparison
    // spans exactly the window. Taking the oldest sample in history instead would compare
    // across a longer span and silently miss a stall that began inside the window.
    let Some(oldest) = history
        .iter()
        .rev()
        .find(|sample| newest.at.saturating_sub(sample.at) >= thresholds.stall_window)
    else {
        return check(
            "traffic",
            Level::Ok,
            format!("watching: not enough history yet{advisory}"),
            "",
        );
    };
    match (newest.downloaded_bytes, oldest.downloaded_bytes) {
        (Some(now), Some(before)) if now > before => check(
            "traffic",
            Level::Ok,
            format!("{} bytes gained{advisory}", now - before),
            "",
        ),
        (Some(now), Some(before)) if now < before => check(
            "traffic",
            Level::Ok,
            format!("download counter reset{advisory}"),
            "",
        ),
        (Some(_), Some(_)) => check(
            "traffic",
            Level::Fail,
            format!(
                "{} torrents want data; 0 bytes in {}s{advisory}",
                newest.wanting, thresholds.stall_window
            ),
            TRAFFIC_REMEDY,
        ),
        _ => check(
            "traffic",
            Level::Warn,
            format!("download counter unavailable{advisory}"),
            TRAFFIC_REMEDY,
        ),
    }
}

fn is_public_ip(value: &str) -> bool {
    // Canonicalise first so an IPv4-mapped address such as ::ffff:192.168.1.1 is judged
    // by its IPv4 rules rather than slipping through the IPv6 arm as public.
    match value.parse::<IpAddr>().map(|ip| match ip {
        IpAddr::V6(v6) => v6.to_canonical(),
        v4 => v4,
    }) {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, _, _] = ip.octets();
            !(a == 10
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || a == 127
                || (a == 169 && b == 254)
                || (a == 100 && (64..=127).contains(&b))
                || ip.is_unspecified())
        }
        Ok(IpAddr::V6(ip)) => {
            let [a, b, ..] = ip.octets();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || (a & 0xfe) == 0xfc
                || (a == 0xfe && (b & 0xc0) == 0x80))
        }
        Err(_) => false,
    }
}

/// Evaluates oldest-first observations without performing I/O or reading a clock.
pub fn evaluate(history: &[Observation], thresholds: &Thresholds) -> Verdict {
    if history.is_empty() {
        return Verdict {
            level: Level::Fail,
            checks: vec![check(
                "probe",
                Level::Fail,
                "no observations yet".into(),
                PROBE_REMEDY,
            )],
        };
    }
    let confirmations = thresholds.confirmations;
    let checks = vec![
        confirmed(history, confirmations, probe),
        confirmed(history, confirmations, tunnel),
        confirmed(history, confirmations, port_agreement),
        confirmed(history, confirmations, reachability),
        traffic(history, thresholds),
    ];
    Verdict {
        level: checks
            .iter()
            .map(|item| item.level)
            .max()
            .unwrap_or(Level::Fail),
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thresholds(confirmations: usize) -> Thresholds {
        Thresholds {
            stall_window: 900,
            confirmations,
        }
    }
    fn sample(at: u64) -> Observation {
        Observation {
            at,
            forwarded_port: Some(5914),
            vpn_public_ip: Some("203.0.113.42".into()),
            listen_port: Some(5914),
            connection_status: Some("connected".into()),
            downloaded_bytes: Some(100),
            wanting: 0,
            seedless: 0,
            errors: vec![],
        }
    }
    fn level(history: &[Observation], name: &str, confirmations: usize) -> Level {
        evaluate(history, &thresholds(confirmations))
            .checks
            .into_iter()
            .find(|item| item.name == name)
            .unwrap()
            .level
    }

    #[test]
    fn empty_history_fails_probe() {
        let verdict = evaluate(&[], &thresholds(3));
        assert_eq!(verdict.level, Level::Fail);
        assert_eq!(verdict.checks[0].detail, "no observations yet");
    }

    #[test]
    fn traffic_gating_and_progress_cases() {
        let mut old = sample(0);
        let mut new = sample(900);
        assert_eq!(level(&[old.clone(), new.clone()], "traffic", 1), Level::Ok);
        new.wanting = 3;
        assert_eq!(
            level(&[old.clone(), new.clone()], "traffic", 1),
            Level::Fail
        );
        new.at = 899;
        assert_eq!(level(&[old.clone(), new.clone()], "traffic", 1), Level::Ok);
        new.at = 900;
        new.downloaded_bytes = Some(99);
        assert_eq!(level(&[old.clone(), new], "traffic", 1), Level::Ok);
        old.downloaded_bytes = None;
        let mut unavailable = sample(900);
        unavailable.wanting = 1;
        assert_eq!(level(&[old, unavailable], "traffic", 1), Level::Warn);
    }

    #[test]
    fn port_cases() {
        let mut observation = sample(0);
        observation.listen_port = Some(6000);
        assert_eq!(
            level(&[observation.clone()], "port_agreement", 1),
            Level::Fail
        );
        observation.listen_port = None;
        assert_eq!(level(&[observation], "port_agreement", 1), Level::Warn);
    }

    #[test]
    fn reachability_cases() {
        for (status, expected) in [
            ("firewalled", Level::Fail),
            ("disconnected", Level::Fail),
            ("mystery", Level::Warn),
        ] {
            let mut observation = sample(0);
            observation.connection_status = Some(status.into());
            assert_eq!(level(&[observation], "reachability", 1), expected);
        }
    }

    #[test]
    fn tunnel_missing_and_private_fail() {
        for ip in [None, Some("192.168.1.2")] {
            let mut observation = sample(0);
            observation.vpn_public_ip = ip.map(str::to_owned);
            assert_eq!(level(&[observation], "tunnel", 1), Level::Fail);
        }
    }

    #[test]
    fn public_ip_branches() {
        for ip in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "127.0.0.1",
            "169.254.2.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "::",
            "fc00::1",
            "fd00::1",
            "fe80::1",
            "not-an-ip",
        ] {
            assert!(!is_public_ip(ip), "{ip}");
        }
        assert!(is_public_ip("203.0.113.42"));
        assert!(is_public_ip("2001:4860:4860::8888"));
    }

    /// Regression: the baseline must be the window boundary, not the oldest sample held.
    /// Progress that happened before the window opened must not mask a stall inside it.
    #[test]
    fn stall_detected_when_history_outlives_window() {
        let mut history = vec![sample(0)];
        // Bytes climb early, then flatline for longer than the 900s window.
        for (index, at) in [300u64, 600, 900, 1200, 1500, 1800].iter().enumerate() {
            let mut observation = sample(*at);
            observation.wanting = 2;
            observation.downloaded_bytes = Some(if index < 2 { 100 + index as u64 } else { 101 });
            history.push(observation);
        }
        assert_eq!(level(&history, "traffic", 1), Level::Fail);
    }

    /// Regression: an IPv4-mapped IPv6 private address must not read as a public IP.
    #[test]
    fn ipv4_mapped_private_is_not_public() {
        assert!(!is_public_ip("::ffff:192.168.1.1"));
        assert!(!is_public_ip("::ffff:10.0.0.1"));
        assert!(is_public_ip("::ffff:203.0.113.42"));
    }

    #[test]
    fn probe_errors_and_absent_status() {
        let mut observation = sample(0);
        observation.errors.push("gluetun port: HTTP 401".into());
        assert_eq!(level(&[observation], "probe", 1), Level::Fail);
        let mut absent = sample(0);
        absent.connection_status = None;
        assert_eq!(level(&[absent], "reachability", 1), Level::Warn);
    }

    #[test]
    fn confirmation_hysteresis() {
        let mut bad = sample(0);
        bad.listen_port = Some(6000);
        assert_eq!(level(&[bad.clone()], "port_agreement", 3), Level::Warn);
        assert_eq!(
            level(&[bad.clone(), bad.clone(), bad], "port_agreement", 3),
            Level::Fail
        );
    }
}
