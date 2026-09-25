# Security Policy

Nisaba runs on an operator's own machine and holds credentials for live selling
accounts. A vulnerability here is not abstract: it can read marketplace OAuth
tokens, write to real inventory, or expose a company's catalog to a peer that
should not have it. Please report anything you find.

## Supported versions

Nisaba has not cut a tagged release yet. Only the `master` branch is supported;
fixes land there.

| Version | Supported |
|---------|-----------|
| `master` | ✅ |
| Anything older | ❌ |

## Reporting a vulnerability

**Do not open a public issue for a security problem.**

Use GitHub's private vulnerability reporting on this repository: go to the
**Security** tab → **Report a vulnerability**. That opens a private advisory
visible only to you and the maintainers.

Please include:

- what an attacker can do, and what they need first (local access? a malicious
  vendor plugin? a peer they control? a compromised marketplace response?)
- the affected crate or file
- reproduction steps, or a proof-of-concept plugin/payload
- the commit you tested against

You should get an acknowledgement within a week. Please give us a chance to ship
a fix before disclosing publicly.

## What we consider a vulnerability

The parts of Nisaba that carry real security weight:

- **Credential handling.** Marketplace OAuth tokens and refresh tokens, the
  Squarespace API key, XMR Bazaar session cookies, the P2P company secret, and
  any vendor-plugin config field marked `secret: true` belong in the OS keyring.
  Anything that writes one of those to `config.toml`, to a log line, to an
  export, or into an error message shown in the UI is a vulnerability.
- **The vendor plugin sandbox** (`crates/vendor-runtime`). Vendor plugins are
  third-party TypeScript executed in an embedded Deno runtime, and the
  `op_nisaba_*` ops are the whole security boundary. A plugin reaching the
  filesystem, spawning a process, or making network requests outside
  `op_nisaba_fetch` is a sandbox escape — report it.
- **The P2P layer** (`crates/p2p`). Payloads between installs are AES-GCM
  encrypted and carried over a Tor onion service. Anything that lets an
  unauthorized peer read or write a company's catalog, or that weakens the
  encryption or the onion service's addressing, counts.
- **Platform adapters.** Marketplace responses are untrusted input. A crafted
  API response or scraped HTML page that causes Nisaba to write the wrong
  quantity or price to a *different* listing is a vulnerability, not a bug —
  it costs the operator real stock.

## What we do not consider a vulnerability

- An operator choosing to run a malicious vendor plugin *and* granting it hosts
  explicitly. Plugins are third-party code by design; the sandbox is the
  boundary, and the trust decision at install time is the operator's.
- Findings that require an attacker who already has the operator's OS user
  account. Nisaba trusts the local user and the OS keyring.
- Rate limits, quotas, or terms-of-service issues with a marketplace API.

## Handling secrets in contributions

If you are contributing:

- never commit a `config.toml`, a `*.db`, an OAuth token, or a `.har` capture —
  `.gitignore` covers these, keep it that way
- never add a real credential to a test fixture, even an expired one
- never log a token, a cookie, or the company secret, at any `tracing` level
