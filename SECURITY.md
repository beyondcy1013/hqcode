# Security Policy

## Scope & Deployment Guidance

HQCode provides direct shell execution, workspace file editing, and Git operations inside a single host binary.

- By default, bind to `127.0.0.1:9900` and expose remotely only over TLS reverse proxy authentication, VPN, or Tailscale.
- Initial bootstrap passwords and session secrets are stored with owner-only permissions (`0600` on Unix) inside the data directory (default `~/.hqcode`).
- Never commit `.hqcode-*` credential files, API keys, or private hostnames.

## Reporting a Vulnerability

Please report security issues privately via GitHub Security Advisories or contact the repository maintainers before disclosing publicly.
