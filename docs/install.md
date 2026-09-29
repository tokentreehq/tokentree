# Installation and First Run

TokenTree is a local-first AI coding-agent usage measurement engine. It automatically organizes provider requests into projects, work items, and hierarchical subtasks without requiring a cloud account, telemetry, or external database.

---

## Requirements

- **Supported Platforms**: macOS (Apple Silicon / Intel), Linux (x86_64, aarch64), Windows (x64).
- **Supported Hosts**: Claude Code (lifecycle hooks & plugin), with generic CLI support for any host via explicit `tokentree start` and `tokentree stop`.

---

## 1. Quick Install via Claude Code Plugin

The easiest way to use TokenTree with Claude Code is installing the plugin directly:

```bash
# Add marketplace plugin
claude plugin add tokentree@tokentreehq/tokentree
```

Once installed, Claude Code automatically runs TokenTree's lightweight lifecycle hooks (`SessionStart`, `UserPromptSubmit`, `TaskCreated`, `TaskCompleted`, `SubagentStart`, `SessionEnd`).

---

## 2. Standalone CLI Installation

You can install the official `@tokentreehq/cli` binary wrapper via npm or download prebuilt signed binaries from GitHub Releases:

### Via npm
```bash
npm install -g @tokentreehq/cli
```

### Prebuilt Binary (GitHub Releases)
Download the latest release archive from [https://github.com/tokentreehq/tokentree/releases](https://github.com/tokentreehq/tokentree/releases), extract `tokentree`, and move it into your `$PATH` (e.g., `/usr/local/bin` or `%USERPROFILE%\bin`).

---

## 3. Verifying Your Setup (`doctor`)

Run `tokentree doctor` to confirm that the SQLite ledger, pricing snapshot, and filesystem permissions are healthy:

```bash
tokentree doctor
```

Example healthy output:
```text
ok: ledger sqlite version 3.48.0
ok: ledger schema version 1
ok: pricing snapshot verified (sha256 valid, 1 models)
ok: filesystem permissions secure (0700 / 0600)
ok: no prompt text or sensitive payloads leaked to disk
ok: loopback security enforced
```

---

## 4. First Run

Work normally in your projects with Claude Code or another coding agent. TokenTree automatically detects project boundaries from Git roots, project manifests (`package.json`, `Cargo.toml`, etc.), or working directories.

To inspect usage and cost rollups at any time:

```bash
# Print recursive terminal tree
tokentree report --text

# Launch local interactive web dashboard
tokentree dashboard

# Generate self-contained static HTML report
tokentree report --html
```
