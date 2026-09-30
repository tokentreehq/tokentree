# TokenTree Friend Validation Kit

This standalone kit allows external collaborators with paid Claude Code or OpenAI Codex accounts to run standardized, automated validation of TokenTree's adapters and produce privacy-safe verification evidence.

## Privacy & Redaction Guarantees

The validation kit strictly enforces:
- **Versioned Allowlisted Evidence Schema**: The emitted JSON conform strictly to `schema_version: "1.0.0"` and contains **only booleans and aggregate counters**.
- **Zero Raw Prompts & Completions**: No prompt text, assistant completions, or tool payloads are stored.
- **Zero Command Output or Logs**: No raw terminal stdout, stderr, or log text is stored.
- **Zero Usernames, Home Directories, or Paths**: No file paths, usernames, or directory structures are included.
- **Zero Credential Exposure**: API keys (`sk-ant-`, `sk-`, `Bearer `, `x-api-key`, etc.) are prohibited and filtered.
- **Recursive Allowlist Rejection**: Any unexpected or unknown field in any object or nested object is recursively rejected.

---

## Allowlisted Evidence Schema (`v1.0.0`)

The evidence file contains exclusively the following allowlisted structure:

```json
{
  "schema_version": "1.0.0",
  "adapter": "claude",
  "environment": {
    "isolated_workspace": true,
    "isolated_home": true,
    "clean_baseline": true
  },
  "checks": {
    "live_capture_verified": true,
    "ledger_created": true,
    "doctor_clean": true,
    "no_leaks_detected": true,
    "reconcile_clean": true
  },
  "counters": {
    "total_sessions": 1,
    "total_turns": 1,
    "total_requests": 1,
    "measured_requests": 1,
    "unavailable_requests": 0,
    "anomalous_requests": 0,
    "duplicate_requests": 0,
    "unresolved_anomalies": 0,
    "total_tokens": 1500,
    "input_tokens": 1200,
    "output_tokens": 300,
    "cache_read_tokens": 0,
    "cache_write_tokens": 0,
    "reasoning_tokens": 0,
    "cost_micros": 0
  }
}
```

---

## 1. Claude Code Validation

### Prerequisites
1. Installed Claude Code CLI (`claude`).
2. An active, authenticated Claude session.

### Execution

#### On Windows (PowerShell):
```powershell
pwsh ./scripts/validation-kit/run-claude-validation.ps1
```

#### On macOS / Linux (Bash):
```bash
./scripts/validation-kit/run-claude-validation.sh
```

### What this does:
1. Detects your local Claude CLI version.
2. Creates an isolated test workspace and isolated TokenTree home.
3. Sends a single low-cost standardized test prompt:
   `"Reply with exactly: TokenTree live capture verified."`
4. Processes the hook spool through TokenTree's accounting engine.
5. Runs `doctor`, `reconcile`, and `report --text`.
6. Extracts verification booleans and aggregate token counters.
7. Validates the generated `claude-validation-evidence.json` with `verify-evidence.ts`.

---

## 2. OpenAI Codex Validation

### Prerequisites
1. Installed OpenAI Codex app-server or rollout sessions directory (`~/.codex/sessions`).

### Execution

#### On Windows (PowerShell):
```powershell
pwsh ./scripts/validation-kit/run-codex-validation.ps1 -CodexSessionsDir "$HOME/.codex/sessions"
```

#### On macOS / Linux (Bash):
```bash
./scripts/validation-kit/run-codex-validation.sh "$HOME/.codex/sessions"
```

### What this does:
1. Discovers Codex rollout and app-server JSONL session streams.
2. Ingests usage into an isolated TokenTree ledger.
3. Checks turn correlation, covered counter suppression, and checkpoint resumption.
4. Runs `doctor`, `reconcile`, and `report --text`.
5. Extracts verification booleans and aggregate token counters.
6. Validates the generated `codex-validation-evidence.json` with `verify-evidence.ts`.

---

## 3. Evidence Verification

Before sharing the evidence file with the TokenTree maintainers, verify it locally:

```bash
pnpm tsx ./scripts/validation-kit/verify-evidence.ts ./scripts/validation-kit/evidence-claude/claude-validation-evidence.json
```

The verifier will assert that:
- Zero forbidden credentials or tokens match known patterns.
- Every field is strictly allowlisted, with all unknown fields recursively rejected.
- All checks are boolean flags and all counters are safe non-negative integers.
- The file is clean and ready to attach to acceptance issues or PR reviews.
