# TokenTree Friend Validation Kit

This standalone kit allows external collaborators with paid Claude Code or OpenAI Codex accounts to run standardized, automated validation of TokenTree's adapters and produce privacy-safe verification evidence.

## Privacy & Redaction Guarantees

The validation kit strictly enforces:
- **Zero Prompt / Completion Storage**: No raw prompt strings, completions, user source code, or file contents are persisted or exported.
- **Zero Credential Exposure**: API keys (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, etc.), auth cookies, and bearer tokens are blocked by validation filters.
- **Schema-Safe Metadata Only**: The export bundle contains only OS/architecture metadata, token counts, cost calculations, completeness percentages, and doctor/reconcile status logs.

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
6. Generates `claude-validation-evidence.json`.

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
5. Generates `codex-validation-evidence.json`.

---

## 3. Evidence Verification

Before sharing the evidence file with the TokenTree maintainers, verify it locally:

```bash
pnpm tsx ./scripts/validation-kit/verify-evidence.ts ./scripts/validation-kit/evidence-claude/claude-validation-evidence.json
```

The verifier will assert that:
- Zero forbidden credentials or tokens match known patterns.
- Required verification sections (`doctor`, `reconcile`, `report`) are present and uncorrupted.
- The file is clean and ready to attach to acceptance issues or PR reviews.
