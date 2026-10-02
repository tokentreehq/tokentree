# Security Policy

## Supported versions

| Version | Supported          |
| ------- | ------------------ |
| 0.2.x   | :white_check_mark: |
| < 0.2   | :x:                |

## Reporting a vulnerability

**Do not open a public issue for security vulnerabilities.**

TokenTree is local-first software: the primary attack surface is the local
dashboard, the OTLP receiver, and the parsers that ingest untrusted
transcript/hook data. If you find something:

1. Open a [private security advisory](https://github.com/tokentreehq/tokentree/security/advisories/new)
   on this repo, **or**
2. DM [@0xshrikar](https://x.com/0xshrikar) on X.

Include:
- What you found and where (file:line if possible)
- Steps to reproduce (a minimal transcript or command sequence)
- What you think the impact is (data corruption? prompt leakage? RCE?)

## What happens next

- We acknowledge within **48 hours**.
- We aim to ship a fix within **14 days** for anything affecting ledger
  integrity or prompt privacy, faster for RCE.
- We credit reporters in the release notes unless you ask not to be named.

## Scope notes

- TokenTree has no network services by design (dashboard and OTLP bind to
  loopback only). Reports about "no TLS on localhost" are out of scope.
- Dependency vulnerabilities are in scope only if reachable through
  TokenTree's actual code paths — run `cargo audit` / `pnpm audit` and
  check reachability before reporting.
- The full threat model lives in [docs/privacy.md](./docs/privacy.md) and
  [docs/security.md](./docs/security.md).
