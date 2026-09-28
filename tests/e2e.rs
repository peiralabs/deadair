use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Output};
use std::thread::{self, JoinHandle};

const PORT: &str = r#"{"port":5914}"#;
const PUBLIC_IP: &str = r#"{"public_ip":"58.98.64.104","country":"Sweden","city":"Stockholm"}"#;
const PREFERENCES: &str = r#"{"listen_port":5914,"dht":true,"locale":"en"}"#;
const TRANSFER: &str = r#"{"dl_info_speed":0,"dl_info_data":1000,"up_info_speed":0,"up_info_data":5,"connection_status":"connected"}"#;
const TORRENTS: &str = r#"[{"state":"downloading","num_complete":12}]"#;

struct MockStack {
    gluetun: SocketAddr,
    qbt: SocketAddr,
    threads: Vec<JoinHandle<()>>,
}

impl MockStack {
    fn start(listen_port: u16, connection_status: &str, torrents: &str, legacy: bool) -> Self {
        let gluetun_listener = TcpListener::bind("127.0.0.1:0").expect("bind gluetun mock");
        let qbt_listener = TcpListener::bind("127.0.0.1:0").expect("bind qBittorrent mock");
        let gluetun = gluetun_listener.local_addr().expect("gluetun address");
        let qbt = qbt_listener.local_addr().expect("qBittorrent address");

        let gluetun_thread = thread::spawn(move || {
            let request_count = if legacy { 3 } else { 2 };
            for stream in gluetun_listener.incoming().take(request_count) {
                let mut stream = stream.expect("accept gluetun request");
                let (method, path) = read_request(&mut stream);
                let response = match (method.as_str(), path.as_str()) {
                    ("GET", "/v1/portforward") if legacy => (404, "Not Found", "{}"),
                    ("GET", "/v1/portforward") => (200, "OK", PORT),
                    ("GET", "/v1/openvpn/portforwarded") => (200, "OK", PORT),
                    ("GET", "/v1/publicip/ip") => (200, "OK", PUBLIC_IP),
                    _ => (404, "Not Found", "{}"),
                };
                write_response(&mut stream, response.0, response.1, response.2, false);
            }
        });

        let preferences = if listen_port == 5914 {
            PREFERENCES.to_owned()
        } else {
            format!(r#"{{"listen_port":{listen_port},"dht":true,"locale":"en"}}"#)
        };
        let transfer = if connection_status == "connected" {
            TRANSFER.to_owned()
        } else {
            format!(
                r#"{{"dl_info_speed":0,"dl_info_data":1000,"up_info_speed":0,"up_info_data":5,"connection_status":"{connection_status}"}}"#
            )
        };
        let torrents = torrents.to_owned();
        let qbt_thread = thread::spawn(move || {
            for stream in qbt_listener.incoming().take(4) {
                let mut stream = stream.expect("accept qBittorrent request");
                let (method, path) = read_request(&mut stream);
                let (code, reason, body, cookie) = match (method.as_str(), path.as_str()) {
                    ("POST", "/api/v2/auth/login") => (200, "OK", "Ok.", true),
                    ("GET", "/api/v2/app/preferences") => (200, "OK", preferences.as_str(), false),
                    ("GET", "/api/v2/transfer/info") => (200, "OK", transfer.as_str(), false),
                    ("GET", "/api/v2/torrents/info") => (200, "OK", torrents.as_str(), false),
                    _ => (404, "Not Found", "{}", false),
                };
                write_response(&mut stream, code, reason, body, cookie);
            }
        });

        Self {
            gluetun,
            qbt,
            threads: vec![gluetun_thread, qbt_thread],
        }
    }

    fn run(self) -> Output {
        // `run_check` blocks until the binary exits, so every request has been served by
        // the time it returns. The server threads are deliberately NOT joined: they park
        // on `incoming()` for a fixed number of connections, so joining would hang the
        // whole suite if the binary ever issued fewer requests than expected.
        let output = run_check(self.gluetun, self.qbt);
        drop(self.threads);
        output
    }
}

fn read_request(stream: &mut TcpStream) -> (String, String) {
    let mut request = Vec::new();
    let mut byte = [0; 1];
    while !request.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).expect("read request headers");
        request.push(byte[0]);
    }

    let headers = String::from_utf8(request).expect("UTF-8 request headers");
    let mut lines = headers.split("\r\n");
    let mut request_line = lines.next().expect("request line").split_whitespace();
    let method = request_line.next().expect("request method").to_owned();
    let path = request_line.next().expect("request path").to_owned();
    let content_length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("Content-Length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or(0);
    let mut body = vec![0; content_length];
    stream.read_exact(&mut body).expect("read request body");
    (method, path)
}

fn write_response(stream: &mut TcpStream, code: u16, reason: &str, body: &str, set_cookie: bool) {
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .expect("write response headers");
    if set_cookie {
        write!(stream, "Set-Cookie: SID=tok3n; path=/; HttpOnly\r\n").expect("write cookie header");
    }
    write!(stream, "\r\n{body}").expect("write response body");
}

fn run_check(gluetun: SocketAddr, qbt: SocketAddr) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deadair"))
        .args(["check", "--json"])
        .env("DEADAIR_GLUETUN_URL", format!("http://{gluetun}"))
        .env("DEADAIR_QBT_URL", format!("http://{qbt}"))
        .env("DEADAIR_QBT_USER", "admin")
        .env("DEADAIR_QBT_PASS", "password")
        .env("DEADAIR_TIMEOUT", "2")
        .output()
        .expect("run deadair")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 output")
}

#[test]
fn healthy_stack() {
    let output = MockStack::start(5914, "connected", TORRENTS, false).run();
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(
        stdout(&output).contains(r#""name":"port_agreement","level":"ok""#),
        "{}",
        stdout(&output)
    );
}

#[test]
fn firewalled_and_mismatched() {
    let torrents = r#"[{"state":"stalledDL","num_complete":0}]"#;
    let output = MockStack::start(6881, "firewalled", torrents, false).run();
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
}

#[test]
fn legacy_gluetun_port_endpoint() {
    let output = MockStack::start(5914, "connected", TORRENTS, true).run();
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(
        stdout(&output).contains(r#""name":"port_agreement","level":"ok""#),
        "{}",
        stdout(&output)
    );
}

#[test]
fn unreachable_upstreams() {
    let gluetun = unused_address();
    let qbt = unused_address();
    let output = run_check(gluetun, qbt);
    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    assert!(stdout(&output).contains("probe"), "{}", stdout(&output));
}

fn unused_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve unused port");
    listener.local_addr().expect("unused address")
}
