# Privacy Architecture & Security Threat Model

TokenTree is engineered from the ground up to protect developer source code and sensitive prompt data.

---

## Core Privacy Principles

1. **Local Storage Only**: All tokens, estimates, session IDs, and work items reside solely in your local SQLite ledger (`~/.tokentree/ledger.db`).
2. **No Accounts or Cloud Sync**: There is no remote account, login, or central database.
3. **No Network Telemetry**: TokenTree never transmits analytics, usage metrics, or error reports to any external server.
4. **No Prompt Retention**: Full prompts, agent completions, model reasoning traces, source-code files, file diffs, tool inputs, and tool outputs are **never persisted to disk**.

---

## What TokenTree Stores

| Category | Persisted in Ledger | Never Persisted |
|---|---|---|
| **Usage** | Token counts (input, cache read/write, output, reasoning), model name, duration, timestamps. | Raw API response payloads, generation tokens. |
| **Turns** | SHA-256 `prompt_fingerprint`, 3–8 word redacted derived label. | Prompt text, prompt variables, system prompts. |
| **Tools** | Tool name (e.g. `Edit`), affected file path. | File contents, edits, git diffs, command stdin/stdout. |
| **Identity** | Project display name, Git remote URL, branch name, commit hash. | User passwords, SSH keys, credentials. |
| **Notes** | Explicit human annotations entered via `tokentree note`. | Automated transcript scrapes. |

---

## Dashboard Security

The TokenTree local dashboard (`tokentree dashboard`) enforces strict isolation:
- **Loopback-Only Binding**: Listens exclusively on `127.0.0.1`.
- **Ephemeral Session Token**: A high-entropy 32-character random hex token is generated at startup and required on every request. The token dies when the process terminates.
- **Strict Content Security Policy**:
  ```text
  default-src 'self' 'unsafe-inline' data:; connect-src 'self'; frame-ancestors 'none'; object-src 'none';
  ```
- **Zero External CDNs**: All stylesheets, scripts, fonts, and inline SVG icons are embedded directly into the binary.
- **No Transcript Routes**: The dashboard server reads only the SQLite ledger; it contains no endpoints capable of reading or serving raw transcripts.

---

## Security Threat Model

### Protected Against By Design
- Accidental network exfiltration of source code or prompts by TokenTree.
- Prompt text leaking into database backups or logs.
- External web pages embedding the local dashboard via iframes (enforced via `frame-ancestors 'none'`).
- Local malicious scripts attempting unauthorized dashboard API calls without the session token.
- Shell injection attacks (all Git, file, and system commands avoid shell string interpolation).

### Not Protected Against By Default
- Another process running with the same OS user permissions accessing `~/.tokentree/ledger.db`.
- A compromised root/administrator account.
- Provider-side logging or data retention on the AI host (e.g., Anthropic or OpenAI API servers).
- The original agent host transcripts created by Claude Code or Codex.
