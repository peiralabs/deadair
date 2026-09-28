use std::env;

/// Runtime configuration read from environment variables.
#[derive(Clone)]
pub struct Config {
    /// Gluetun control-server base URL.
    pub gluetun_url: String,
    /// Optional gluetun API key.
    pub gluetun_apikey: Option<String>,
    /// Optional gluetun basic-auth username.
    pub gluetun_user: Option<String>,
    /// Optional gluetun basic-auth password.
    pub gluetun_pass: Option<String>,
    /// qBittorrent WebUI base URL.
    pub qbt_url: String,
    /// Optional qBittorrent username.
    pub qbt_user: Option<String>,
    /// Optional qBittorrent password.
    pub qbt_pass: Option<String>,
    /// Seconds between watch samples.
    pub interval: u64,
    /// Thresholds used to evaluate observations.
    pub thresholds: Thresholds,
    /// Metrics server listen address.
    pub listen: String,
    /// Per-request HTTP timeout in seconds.
    pub timeout: u64,
}

/// Thresholds consumed by the pure verdict evaluator.
#[derive(Clone, Copy)]
pub struct Thresholds {
    /// Seconds without progress before traffic is stalled.
    pub stall_window: u64,
    /// Consecutive failures required before reporting failure.
    pub confirmations: usize,
}

impl Config {
    /// Reads configuration from the environment, using defaults for absent or invalid values.
    pub fn from_env() -> Self {
        fn text(name: &str, default: &str) -> String {
            env::var(name).unwrap_or_else(|_| default.into())
        }
        fn optional(name: &str) -> Option<String> {
            env::var(name).ok().filter(|value| !value.is_empty())
        }
        fn number<T: std::str::FromStr>(name: &str, default: T) -> T {
            env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        }
        Self {
            gluetun_url: text("DEADAIR_GLUETUN_URL", "http://localhost:8000"),
            gluetun_apikey: optional("DEADAIR_GLUETUN_APIKEY"),
            gluetun_user: optional("DEADAIR_GLUETUN_USER"),
            gluetun_pass: optional("DEADAIR_GLUETUN_PASS"),
            qbt_url: text("DEADAIR_QBT_URL", "http://localhost:8080"),
            qbt_user: optional("DEADAIR_QBT_USER"),
            qbt_pass: optional("DEADAIR_QBT_PASS"),
            interval: number("DEADAIR_INTERVAL", 30),
            thresholds: Thresholds {
                stall_window: number("DEADAIR_STALL_WINDOW", 900),
                confirmations: number("DEADAIR_CONFIRMATIONS", 3),
            },
            listen: text("DEADAIR_LISTEN", "0.0.0.0:9113"),
            timeout: number("DEADAIR_TIMEOUT", 10),
        }
    }
}
