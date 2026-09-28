# deadair

`deadair` checks whether a gluetun + qBittorrent stack is actually working, not merely configured.

Many existing tools sync gluetun's forwarded port into qBittorrent, but a synced port is not the same as traffic flowing: the tunnel can be up, the port synced, and downloads dead for days. `deadair` verifies the things that actually break and prints the specific next step when a check fails. An idle queue is not a failure; it only reports a stall when torrents actually want data and none has arrived within the window, which makes it safe to alert on.

```text
[ok]   probe           all probes succeeded
[ok]   tunnel          203.0.113.42
[fail] port_agreement  forwarded 51413, listening 6881
[fail] reachability    firewalled
[fail] traffic         3 torrents want data; 0 bytes in 900s
[fail] overall
```

## Checks

| Check | Fails when |
|---|---|
| `probe` | gluetun or qBittorrent could not be reached at all |
| `tunnel` | gluetun reports no public IP, or a private/CGNAT one — the tunnel is down and traffic may be leaving over the WAN |
| `port_agreement` | gluetun's forwarded port and qBittorrent's `listen_port` disagree |
| `reachability` | qBittorrent reports `firewalled` (nothing can reach the forwarded port) or `disconnected` |
| `traffic` | torrents want data and zero bytes arrived within `DEADAIR_STALL_WINDOW` |

A check that fails on one sample is reported as a warning until it has failed `DEADAIR_CONFIRMATIONS` samples in a row, so a single blip does not page anyone. One-shot `check` runs have no history, so they report a failure immediately.

## Commands

- `deadair check [--json]` runs one check and exits `0` for ok, `1` for warn, or `2` for fail. Use it with Uptime Kuma, healthchecks.io, or cron.
- `deadair watch` samples on an interval and serves Prometheus metrics.
- `deadair explain` prints the remedy for each failing check.

## Install

Pull the container image (linux/amd64 and linux/arm64):

```sh
docker pull ghcr.io/peiralabs/deadair:latest
```

Or download a static binary from the [latest release](https://github.com/peiralabs/deadair/releases/latest).
It links statically against musl, so it has no runtime dependencies and runs on a
busybox NAS as happily as on a full distro:

```sh
curl -LO https://github.com/peiralabs/deadair/releases/latest/download/deadair-x86_64-unknown-linux-musl
chmod +x deadair-x86_64-unknown-linux-musl
```

Each release also ships `SHA256SUMS`. The released binaries are extracted from the same
build that produces the container image, so they are byte-identical to what runs inside it.

Or build from source:

```sh
cargo install --git https://github.com/peiralabs/deadair --locked
```

Run it beside the stack it watches. Sharing gluetun's network namespace is what lets
`deadair` reach both control servers on `localhost`, exactly as qBittorrent does:

```yaml
services:
  gluetun:
    image: qmcgaw/gluetun
    cap_add: [NET_ADMIN]
    ports:
      - "9113:9113"   # deadair's metrics, published through gluetun's namespace

  qbittorrent:
    image: lscr.io/linuxserver/qbittorrent
    network_mode: "service:gluetun"

  deadair:
    image: ghcr.io/peiralabs/deadair:latest
    network_mode: "service:gluetun"
    environment:
      DEADAIR_GLUETUN_APIKEY: ${DEADAIR_GLUETUN_APIKEY}
    depends_on: [gluetun, qbittorrent]
```

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `DEADAIR_GLUETUN_URL` | `http://localhost:8000` | gluetun control server |
| `DEADAIR_GLUETUN_APIKEY` | unset | sent as `X-API-Key` |
| `DEADAIR_GLUETUN_USER` / `DEADAIR_GLUETUN_PASS` | unset | HTTP basic auth |
| `DEADAIR_QBT_URL` | `http://localhost:8080` | qBittorrent WebUI |
| `DEADAIR_QBT_USER` / `DEADAIR_QBT_PASS` | unset | WebUI login; skipped when unset |
| `DEADAIR_INTERVAL` | `30` | seconds between samples in `watch` |
| `DEADAIR_STALL_WINDOW` | `900` | seconds of zero progress before a stall is a failure |
| `DEADAIR_CONFIRMATIONS` | `3` | consecutive bad samples before a check fails |
| `DEADAIR_LISTEN` | `0.0.0.0:9113` | metrics address |
| `DEADAIR_TIMEOUT` | `10` | per-request HTTP timeout |

Gluetun's control server requires authentication by default in current versions, so `DEADAIR_GLUETUN_APIKEY` is usually needed.

## Metrics

`deadair watch` exports `/metrics` and `/healthz`; health responds with 200 or 503. Levels are `0` ok, `1` warn, and `2` fail. Ports and the byte counter are omitted when unknown rather than reported as zero.

- `deadair_check{check="..."}`
- `deadair_level`
- `deadair_forwarded_port`
- `deadair_listen_port`
- `deadair_downloaded_bytes_total`
- `deadair_wanting_torrents`
- `deadair_seedless_torrents`
- `deadair_last_evaluation_timestamp_seconds`
- `deadair_uptime_seconds`

```yaml
# Prometheus scrape config
scrape_configs:
  - job_name: deadair
    static_configs:
      - targets: ["localhost:9113"]
```

## What it does not do

`deadair` does not sync the forwarded port, and it will not change your qBittorrent
settings. It only reads: the sole write it ever performs is the WebUI login needed to
read. Keep using whichever port syncer you already run — `deadair` tells you when that
syncer's work stopped being enough.

## Why another one of these?

There are already several tools that copy gluetun's forwarded port into qBittorrent. They
do that one job well, and once the port matches they report success and stop looking.

That leaves a gap, because a matching port is a statement about configuration, not about
traffic. The tunnel can drop, the forwarded port can stop being reachable from outside, or
the peer connection can die, and every port syncer will still report a clean sync while
nothing downloads. The failure is silent precisely because the thing being checked still
looks right.

`deadair` checks the other half: that the tunnel carries a real public IP, that the
forwarded port is genuinely reachable rather than merely configured, and that bytes are
actually arriving when torrents are asking for them — while staying quiet when the queue
is simply idle.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT).

## Contributing

Contributions are welcome; please run the formatter, linter, and tests.
