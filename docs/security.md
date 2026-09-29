# Security and privacy threat model

## Assets
Immutable usage measurements, attribution history, local paths/Git metadata, optional notes, price provenance, and database backups.

## Trust boundaries
Host logs and project `.tokentree.yml` are untrusted inputs. Hooks are latency-sensitive enqueue-only producers. The SQLite ledger is authoritative. The dashboard will be loopback-only with a per-process random token. Network features are opt-in.

## Protected by design
- No prompts, completions, reasoning, source, diffs, or tool payloads retained by default.
- No default network egress or telemetry.
- Project config cannot execute commands, configure hooks/egress, weaken privacy, or change global prices.
- SQL parameters, escaped HTML, and no title interpolation into shell commands.
- Data directory mode `0700` and database/sensitive export mode `0600` where supported.
- Atomic writes, checksummed artifacts, transactional migrations, and pre-migration backups.
- Plugin uninstall preserves `~/.tokentree/` unless deletion is explicitly requested.

## Not protected by default
Another process running as the same OS user, administrator/root or a compromised machine, provider-side retention, original host transcript storage, or content deliberately sent to an enabled external classifier.

## Phase gates
Phase 1 adds prompt-leak scanning in `doctor`, permissions enforcement, integrity checks, and spool recovery. Phase 3 adds dashboard CSP, loopback/token tests, and transcript-route denial. Phase 4 adds field-level purge/export and no-network privacy tests.
