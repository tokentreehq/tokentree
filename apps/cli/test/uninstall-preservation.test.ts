// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { execFileSync, execSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';

const here = dirname(fileURLToPath(import.meta.url));
const cliDir = resolve(here, '..');

describe('Criterion 18: Real isolated npm install and uninstall data preservation', () => {
  it('installs packed @tokentreehq/cli globally in isolated prefix, records data, uninstalls, and verifies byte-for-byte ledger preservation', () => {
    // 1. Pack the actual @tokentreehq/cli package using pnpm pack (resolves workspace:* to semver)
    execSync('pnpm --filter @tokentreehq/cli build', { cwd: resolve(cliDir, '../..'), stdio: 'pipe' });
    execSync('pnpm pack', { cwd: cliDir, stdio: 'pipe' });
    const tarballFilename = readdirSync(cliDir).find((f) => f.startsWith('tokentreehq-cli-') && f.endsWith('.tgz'))!;
    const tarballPath = join(cliDir, tarballFilename);

    expect(existsSync(tarballPath)).toBe(true);

    const tmpBase = mkdtempSync(join(tmpdir(), 'tokentree-isolated-install-'));
    try {
      const isolatedPrefix = join(tmpBase, 'prefix');
      const isolatedHome = join(tmpBase, 'home');
      const isolatedXdgData = join(tmpBase, 'share');
      const isolatedXdgConfig = join(tmpBase, 'config');
      const isolatedTokenTreeHome = join(isolatedHome, '.tokentree');

      mkdirSync(isolatedPrefix, { recursive: true });
      mkdirSync(isolatedHome, { recursive: true });
      mkdirSync(isolatedXdgData, { recursive: true });
      mkdirSync(isolatedXdgConfig, { recursive: true });
      mkdirSync(isolatedTokenTreeHome, { recursive: true });

      // Create a mock native binary for the launcher to delegate to
      let mockBinPath = join(tmpBase, 'mock-tokentree');
      if (process.platform === 'win32') {
        mockBinPath += '.cmd';
        writeFileSync(mockBinPath, '@echo off\r\necho tokentree 0.2.0\r\n');
      } else {
        writeFileSync(mockBinPath, '#!/bin/sh\necho "tokentree 0.2.0"\n');
        chmodSync(mockBinPath, 0o755);
      }

      const isolatedEnv: NodeJS.ProcessEnv = {
        ...process.env,
        HOME: isolatedHome,
        USERPROFILE: isolatedHome,
        XDG_DATA_HOME: isolatedXdgData,
        XDG_CONFIG_HOME: isolatedXdgConfig,
        npm_config_prefix: isolatedPrefix,
        TOKENTREE_HOME: isolatedTokenTreeHome,
        TOKENTREE_BIN: mockBinPath,
      };

      // 2. Install packed tarball globally into isolated prefix
      execSync(`npm install -g "${tarballPath}" --prefix "${isolatedPrefix}"`, {
        env: isolatedEnv,
        stdio: 'pipe',
      });

      // 3. Locate the installed executable
      let installedBinPath = join(isolatedPrefix, 'bin', 'tokentree');
      if (process.platform === 'win32') {
        const winCmd = join(isolatedPrefix, 'tokentree.cmd');
        const winBin = join(isolatedPrefix, 'tokentree');
        installedBinPath = existsSync(winCmd) ? winCmd : winBin;
      }

      expect(existsSync(installedBinPath)).toBe(true);

      // 4. Verify installed launcher executes and delegates to TOKENTREE_BIN
      if (process.platform === 'win32') {
        const cmdLine = `"${installedBinPath}" --version`;
        const out = execSync(cmdLine, { env: isolatedEnv, encoding: 'utf8' });
        expect(out).toContain('tokentree');
      } else {
        const out = execFileSync(installedBinPath, ['--version'], { env: isolatedEnv, encoding: 'utf8' });
        expect(out).toContain('tokentree');
      }

      // Record user ledger data under isolatedTokenTreeHome
      const ledgerDbPath = join(isolatedTokenTreeHome, 'ledger.db');
      const db = new DatabaseSync(ledgerDbPath);
      db.exec(`
        CREATE TABLE projects (id TEXT PRIMARY KEY, key TEXT);
        CREATE TABLE work_items (id TEXT PRIMARY KEY, title TEXT);
        CREATE TABLE manual_runs (id TEXT PRIMARY KEY, project_id TEXT, work_item_id TEXT);
        CREATE TABLE notes (id TEXT PRIMARY KEY, text TEXT);
        INSERT INTO projects VALUES ('p1', 'c18-iso-proj');
        INSERT INTO work_items VALUES ('w1', 'Preservation task');
        INSERT INTO manual_runs VALUES ('m1', 'p1', 'w1');
        INSERT INTO notes VALUES ('n1', 'Critical data that must survive npm uninstall');
      `);
      db.close();

      // 5. Verify ledger.db exists under isolatedTokenTreeHome and record exact bytes + checksum
      expect(existsSync(ledgerDbPath)).toBe(true);
      const beforeBytes = readFileSync(ledgerDbPath);
      const beforeHash = createHash('sha256').update(beforeBytes).digest('hex');

      // 6. Run npm uninstall -g from the same isolated prefix
      execSync(`npm uninstall -g @tokentreehq/cli --prefix "${isolatedPrefix}"`, {
        env: isolatedEnv,
        stdio: 'pipe',
      });

      // 7. Assert package files and binary are removed
      expect(existsSync(installedBinPath)).toBe(false);
      const pkgInPrefix = join(isolatedPrefix, 'lib', 'node_modules', '@tokentreehq', 'cli');
      const winPkgInPrefix = join(isolatedPrefix, 'node_modules', '@tokentreehq', 'cli');
      expect(existsSync(pkgInPrefix)).toBe(false);
      expect(existsSync(winPkgInPrefix)).toBe(false);

      // 8. Assert ledger.db remains byte-for-byte unchanged
      expect(existsSync(ledgerDbPath)).toBe(true);
      const afterBytes = readFileSync(ledgerDbPath);
      const afterHash = createHash('sha256').update(afterBytes).digest('hex');

      expect(afterHash).toBe(beforeHash);
      expect(afterBytes.equals(beforeBytes)).toBe(true);

      // 9. Re-open the database and verify all data is intact
      const verifyDb = new DatabaseSync(ledgerDbPath, { readOnly: true });
      try {
        const run = verifyDb.prepare(`
          SELECT p.key AS project_key, w.title AS task_title
          FROM manual_runs m
          JOIN projects p ON p.id = m.project_id
          JOIN work_items w ON w.id = m.work_item_id
          WHERE p.key = 'c18-iso-proj'
        `).get() as {
          project_key: string;
          task_title: string;
        };
        expect(run).toBeDefined();
        expect(run.task_title).toBe('Preservation task');

        const note = verifyDb.prepare("SELECT text FROM notes WHERE text LIKE '%Critical data%'").get() as { text: string };
        expect(note).toBeDefined();
        expect(note.text).toBe('Critical data that must survive npm uninstall');
      } finally {
        verifyDb.close();
      }
    } finally {
      rmSync(tmpBase, { recursive: true, force: true });
      if (existsSync(tarballPath)) {
        rmSync(tarballPath, { force: true });
      }
    }
  }, 120_000);
});
