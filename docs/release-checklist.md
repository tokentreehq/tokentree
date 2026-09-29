# TokenTree Public-Beta Release Checklist

This checklist documents the exact procedures for publishing, verifying, and rolling back release candidates of TokenTree (`@tokentreehq/cli` and native binary archives).

## 1. Release Identification

- **Release Candidate Version**: `0.2.0`
- **Release Git Tag**: `v0.2.0`
- **Scoped NPM Package**: `@tokentreehq/cli`
- **Target Distribution Channels**:
  - npm: `@tokentreehq/cli` (with bundled JS runner and native launcher)
  - GitHub Releases: Native release archives across 5 targets
- **Publish Status**: Release gate pending human authorization (Criterion 36 remains **PARTIAL** until published to registry).

## 2. Supported Platform Targets & Artifact Layout

| Platform / Architecture | Target Triple | Archive Artifact | Binary Name |
|---|---|---|---|
| macOS (Apple Silicon) | `aarch64-apple-darwin` | `tokentree-aarch64-apple-darwin.tar.gz` | `tokentree` |
| macOS (Intel x64) | `x86_64-apple-darwin` | `tokentree-x86_64-apple-darwin.tar.gz` | `tokentree` |
| Linux (x64 glibc) | `x86_64-unknown-linux-gnu` | `tokentree-x86_64-unknown-linux-gnu.tar.gz` | `tokentree` |
| Linux (ARM64) | `aarch64-unknown-linux-gnu` | `tokentree-aarch64-unknown-linux-gnu.tar.gz` | `tokentree` |
| Windows (x64 MSVC) | `x86_64-pc-windows-msvc` | `tokentree-x86_64-pc-windows-msvc.zip` | `tokentree.exe` |

## 3. Pre-Release Verification Steps

1. **Workspace Compilation & Typecheck**:
   ```bash
   pnpm typecheck
   pnpm -r build
   cargo check --workspace --all-targets
   ```

2. **Full Test Suite Execution**:
   ```bash
   pnpm test
   pnpm fixture:boundaries
   cargo test --workspace
   ```

3. **Lint & Code Style Checks**:
   ```bash
   pnpm lint
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   ```

4. **NPM Pack Validation**:
   ```bash
   cd apps/cli
   npm pack --dry-run
   ```
   Verify that:
   - Tarball contains only `dist/`, `migrations/`, `data/`, `README.md`, `LICENSE`, `package.json`.
   - No development scratch files, test fixtures, or temporary databases are packed.
   - Package size is under 100 kB.
   - `publishConfig.access` is `"public"` and `publishConfig.provenance` is `true`.

5. **Isolated Install / Data Preservation Test**:
   ```bash
   npx vitest run apps/cli/test/uninstall-preservation.test.ts
   npx vitest run apps/cli/test/claude-plugin-automation.test.ts
   ```
   Ensures npm install, run, and uninstall cleanly preserves `ledger.db` byte-for-byte.

## 4. Checksum Generation & Verification

1. **Generate SHA-256 Checksums**:
   ```bash
   cd release-assets
   sha256sum * > SHA256SUMS.txt
   cat SHA256SUMS.txt
   ```

2. **Verify Checksums**:
   ```bash
   sha256sum -c SHA256SUMS.txt
   ```
   Every artifact must match its recorded SHA-256 hash. Never label unverified binaries as signed unless Sigstore/cosign attestation is present.

3. **Generate SBOM**:
   ```bash
   cargo metadata --format-version 1 > release-assets/tokentree-cargo-sbom.json
   pnpm list --json --depth=Infinity > release-assets/tokentree-npm-sbom.json
   ```

## 5. Publishing Procedure (Human-Only Gate)

When human release authorization is granted:

1. **Tag Commit on Main**:
   ```bash
   git tag -a v0.2.0 -m "Release v0.2.0 - TokenTree Public Beta Candidate"
   git push origin v0.2.0
   ```

2. **Publish NPM Package with Provenance**:
   ```bash
   cd apps/cli
   npm publish --access public --provenance
   ```

3. **Verify Published Package**:
   ```bash
   npm view @tokentreehq/cli version
   npm view @tokentreehq/cli dist-tags
   ```

4. **Update Criterion 36 in `docs/public-beta-evidence.md`**:
   - Change status from `PARTIAL` to `PASS`.
   - Record publication timestamp and npm registry URL.

## 6. Rollback & Emergency Deprecation Plan

In the event of a critical security advisory or fatal regression:

1. **Deprecate NPM Package**:
   ```bash
   npm deprecate @tokentreehq/cli@0.2.0 "Critical defect detected. Please downgrade or wait for 0.2.1."
   ```

2. **Mark GitHub Release as Pre-Release or Draft**:
   - Edit release `v0.2.0` on GitHub.
   - Mark as pre-release with prominent notice describing the advisory.

3. **Hotfix Release**:
   - Branch from `v0.2.0`.
   - Apply targeted fix and regression test.
   - Bump version to `0.2.1`.
   - Run full verification suite and release `v0.2.1`.
