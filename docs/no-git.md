# No-Git Project Resolution & Multiple Roots

TokenTree is designed to operate seamlessly in any directory structure, whether or not the codebase uses Git.

---

## Resolution Precedence Order

When an event or session is processed, TokenTree resolves the project identity using the following strict hierarchy:

```text
1. Explicit Override (TOKENTREE_PROJECT env var or --project CLI flag)
        ↓
2. Local Config (.tokentree.yml / .tokentree.json)
        ↓
3. Git Repository Root (excluding $HOME)
        ↓
4. Manifest Discovery (package.json, Cargo.toml, pyproject.toml, go.mod, etc.)
        ↓
5. Current Working Directory (CWD)
```

---

## Manifest-Based Detection (No Git Required)

If a folder is not a Git repository (e.g. `~/Desktop/space-game`), TokenTree walks upwards looking for standard package manifests:
- JavaScript/TypeScript: `package.json`
- Rust: `Cargo.toml`
- Python: `pyproject.toml`, `setup.py`
- Go: `go.mod`
- Java/JVM: `pom.xml`, `build.gradle`

The project title is derived from the manifest name (e.g., `"name": "space-game"` becomes `"Space Game"`).

---

## Safety Guard: Excluding `$HOME`

A common issue in development environments is an accidental `.git` folder in the user's home directory (`C:\Users\username` or `/home/username`). TokenTree explicitly halts upward Git traversal at `$HOME` to ensure that scratch folders or downloads are not lumped together into a giant pseudo-project called `username`.

---

## Multi-Root Projects

A single logical project can span multiple roots (e.g., a frontend app in `~/work/client` and a backend service in `~/work/api` sharing `.tokentree.yml` with the same `project_key`).

TokenTree aggregates usage across roots under the single project key without duplicating totals.
