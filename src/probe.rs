use crate::config::Config;
use crate::verdict::Observation;
use serde::Deserialize;
use std::time::Duration;
use ureq::http::Response;
use ureq::{Agent, Body};

#[derive(Deserialize)]
struct PortResponse {
    port: u16,
}
#[derive(Deserialize)]
struct IpResponse {
    public_ip: String,
}
#[derive(Deserialize)]
struct Preferences {
    listen_port: u16,
}
#[derive(Deserialize)]
struct Transfer {
    dl_info_data: u64,
    connection_status: String,
}
#[derive(Deserialize)]
struct Torrent {
    state: String,
    num_complete: i64,
}

/// Ceiling for a single JSON response body, well above a large qBittorrent torrent list.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024 * 1024;

/// Stateful HTTP probe for gluetun and qBittorrent.
pub struct Probe {
    agent: Agent,
    config: Config,
    sid: Option<String>,
}

impl Probe {
    /// Creates a probe with the configured global request timeout.
    pub fn new(config: Config) -> Self {
        let settings = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(config.timeout)))
            .http_status_as_error(false)
            .build();
        Self {
            agent: settings.into(),
            config,
            sid: None,
        }
    }

    /// Samples every external endpoint, retaining failures inside the observation.
    pub fn observe(&mut self, now: u64) -> Observation {
        let mut observation = Observation {
            at: now,
            forwarded_port: None,
            vpn_public_ip: None,
            listen_port: None,
            connection_status: None,
            downloaded_bytes: None,
            wanting: 0,
            seedless: 0,
            errors: vec![],
        };
        match self.gluetun_port() {
            Ok(value) => observation.forwarded_port = Some(value),
            Err(error) => observation.errors.push(format!("gluetun port: {error}")),
        }
        match self.gluetun_ip() {
            Ok(value) => observation.vpn_public_ip = Some(value),
            Err(error) => observation.errors.push(format!("gluetun IP: {error}")),
        }
        if self.config.qbt_user.is_some()
            && self.sid.is_none()
            && let Err(error) = self.login()
        {
            observation
                .errors
                .push(format!("qBittorrent login: {error}"));
        }
        match self.qbt_json::<Preferences>("/api/v2/app/preferences") {
            Ok(value) => observation.listen_port = Some(value.listen_port),
            Err(error) => observation
                .errors
                .push(format!("qBittorrent preferences: {error}")),
        }
        match self.qbt_json::<Transfer>("/api/v2/transfer/info") {
            Ok(value) => {
                observation.downloaded_bytes = Some(value.dl_info_data);
                observation.connection_status = Some(value.connection_status);
            }
            Err(error) => observation
                .errors
                .push(format!("qBittorrent transfer: {error}")),
        }
        match self.qbt_json::<Vec<Torrent>>("/api/v2/torrents/info") {
            Ok(torrents) => {
                for torrent in torrents.iter().filter(|torrent| {
                    matches!(
                        torrent.state.as_str(),
                        "downloading" | "metaDL" | "forcedDL" | "stalledDL"
                    )
                }) {
                    observation.wanting += 1;
                    // qBittorrent's seed fields are ambiguous, so this is advisory only.
                    observation.seedless += usize::from(torrent.num_complete == 0);
                }
            }
            Err(error) => observation
                .errors
                .push(format!("qBittorrent torrents: {error}")),
        }
        observation
    }

    fn gluetun_request(&self, path: &str) -> Result<Response<Body>, String> {
        let url = format!("{}{path}", self.config.gluetun_url.trim_end_matches('/'));
        let mut request = self.agent.get(&url);
        if let Some(key) = &self.config.gluetun_apikey {
            request = request.header("X-API-Key", key);
        } else if let Some(user) = &self.config.gluetun_user {
            let credentials = format!(
                "{user}:{}",
                self.config.gluetun_pass.as_deref().unwrap_or("")
            );
            request = request.header(
                "Authorization",
                &format!("Basic {}", base64(credentials.as_bytes())),
            );
        }
        request.call().map_err(|error| error.to_string())
    }

    fn gluetun_port(&self) -> Result<u16, String> {
        let mut response = self.gluetun_request("/v1/portforward")?;
        if response.status().as_u16() == 404 {
            response = self.gluetun_request("/v1/openvpn/portforwarded")?;
        }
        require_success(&response)?;
        response
            .body_mut()
            .read_json::<PortResponse>()
            .map(|value| value.port)
            .map_err(|error| error.to_string())
    }

    fn gluetun_ip(&self) -> Result<String, String> {
        let mut response = self.gluetun_request("/v1/publicip/ip")?;
        require_success(&response)?;
        response
            .body_mut()
            .read_json::<IpResponse>()
            .map(|value| value.public_ip)
            .map_err(|error| error.to_string())
    }

    fn login(&mut self) -> Result<(), String> {
        let url = format!(
            "{}/api/v2/auth/login",
            self.config.qbt_url.trim_end_matches('/')
        );
        let username = self.config.qbt_user.as_deref().unwrap_or("");
        let password = self.config.qbt_pass.as_deref().unwrap_or("");
        let response = self
            .agent
            .post(&url)
            .send_form([("username", username), ("password", password)])
            .map_err(|error| error.to_string())?;
        require_success(&response)?;
        let cookie = response
            .headers()
            .get("set-cookie")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                value
                    .split(';')
                    .find(|part| part.trim().starts_with("SID="))
            })
            .map(str::trim)
            .and_then(|value| value.strip_prefix("SID="))
            .filter(|value| !value.is_empty())
            .ok_or("successful login returned no SID cookie")?;
        self.sid = Some(cookie.to_owned());
        Ok(())
    }

    fn qbt_json<T: serde::de::DeserializeOwned>(&mut self, path: &str) -> Result<T, String> {
        let mut response = self.qbt_request(path)?;
        if response.status().as_u16() == 403 && self.config.qbt_user.is_some() {
            self.sid = None;
            self.login()?;
            response = self.qbt_request(path)?;
        }
        require_success(&response)?;
        // ureq caps read_json at 10MB by default. A large seedbox exceeds that on
        // /api/v2/torrents/info, and a failed read would leave `wanting` at zero — the
        // stall check would then report "idle" and miss every stall. Raise the ceiling.
        response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_json::<T>()
            .map_err(|error| error.to_string())
    }

    fn qbt_request(&self, path: &str) -> Result<Response<Body>, String> {
        let url = format!("{}{path}", self.config.qbt_url.trim_end_matches('/'));
        let mut request = self.agent.get(&url);
        if let Some(sid) = &self.sid {
            request = request.header("Cookie", &format!("SID={sid}"));
        }
        request.call().map_err(|error| error.to_string())
    }
}

fn require_success(response: &Response<Body>) -> Result<(), String> {
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", response.status().as_u16()))
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_padding() {
        assert_eq!(base64(b"u:p"), "dTpw");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
    }
}
