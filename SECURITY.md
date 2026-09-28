# Security policy

## Reporting a vulnerability

Please report security issues privately through
[GitHub's private vulnerability reporting](https://github.com/peiralabs/deadair/security/advisories/new)
rather than opening a public issue. That keeps the details out of public view until
there is something for people to upgrade to.

If private reporting is unavailable to you for any reason, open an issue saying only
that you have a security concern and asking for a private channel. Do not include the
details in it.

## What to expect

`deadair` is maintained by one person as a side project. Reports are handled on a
best-effort basis, not against a service-level agreement. Expect an acknowledgement
within about a week. If a report turns out to be valid, the fix and the advisory are
published together.

Please do not use the issue tracker to chase a report; the advisory thread is the
right place.

## Supported versions

Only the latest release receives fixes. The project is pre-1.0, so there are no
maintained back-branches, and "upgrade to the latest tag" is the supported remedy.

## Scope

In scope:

- Anything that lets an untrusted network peer, an untrusted gluetun or qBittorrent
  response, or a malicious `/metrics` client affect `deadair`'s behaviour beyond
  producing a wrong verdict.
- Credential exposure. Credentials supplied via `DEADAIR_GLUETUN_APIKEY`,
  `DEADAIR_GLUETUN_USER`/`PASS` and `DEADAIR_QBT_USER`/`PASS` are sent only as HTTP
  request headers. They are never placed in a URL and never written to stdout, to the
  metrics output, or into the error strings that a failed probe reports. A path that
  leaks one is a valid report.
- Anything in the published container image or release binaries that is not built by
  the workflow in this repository.

Out of scope:

- A wrong or unhelpful verdict on its own. That is a bug; please file it as an issue.
- Vulnerabilities in gluetun or qBittorrent themselves. Report those upstream.
- The fact that `deadair` will talk to whatever `DEADAIR_GLUETUN_URL` and
  `DEADAIR_QBT_URL` point at. Pointing it at a hostile host is a configuration
  decision, though a hostile response causing memory-unsafety or credential leakage
  would be in scope.

## Notes on the build

`deadair` is `#![forbid(unsafe_code)]` and has no unsafe dependencies of its own
choosing beyond the standard Rust TLS stack. Release binaries are extracted from the
same build that produces the container image, so a released binary and the binary
inside the image for the same tag are byte-identical and verifiable against the
published `SHA256SUMS`.
