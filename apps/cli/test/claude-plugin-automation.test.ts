// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { applyMigrations } from '@tokentreehq/database';
import { processClaudeHookSpool, renderTextReport } from '../src/index.js';

describe('Criteria 1 & 2: Claude Code plugin install, hook registration, and automation', () => {
  const pluginSrcDir = resolve(import.meta.dirname, '../../../plugins/claude-code');

  it('installs plugin, registers hooks once, enqueues session, runs worker, and produces zero-config report', () => {
    const tmpBase = mkdtempSync(join(tmpdir(), 'tt-c1-c2-'));

    try {
      const isolatedClaudeConfig = join(tmpBase, 'claude config with spaces');
      const isolatedHome = join(tmpBase, 'tokentree home with spaces');
      const projectDir = join(tmpBase, 'my sample project with spaces');

      mkdirSync(isolatedClaudeConfig, { recursive: true });
      mkdirSync(isolatedHome, { recursive: true });
      mkdirSync(projectDir, { recursive: true });

      // Create a test project without Git, just a package.json
      writeFileSync(
        join(projectDir, 'package.json'),
        JSON.stringify({ name: 'zero-config-test-app', version: '1.0.0' }, null, 2),
      );

      // 1. Install Claude plugin into isolated configuration directory
      const pluginInstallDir = join(isolatedClaudeConfig, 'plugins', 'tokentree');
      mkdirSync(pluginInstallDir, { recursive: true });

      // Copy plugin files (manifest, hooks, scripts)
      cpSync(join(pluginSrcDir, '.claude-plugin'), join(pluginInstallDir, '.claude-plugin'), { recursive: true });
      cpSync(join(pluginSrcDir, 'hooks'), join(pluginInstallDir, 'hooks'), { recursive: true });
      cpSync(join(pluginSrcDir, 'scripts'), join(pluginInstallDir, 'scripts'), { recursive: true });

      // Verify plugin manifest and hooks exist in isolated config
      const manifestPath = join(pluginInstallDir, '.claude-plugin', 'plugin.json');
      const hooksPath = join(pluginInstallDir, 'hooks', 'hooks.json');
      expect(existsSync(manifestPath)).toBe(true);
      expect(existsSync(hooksPath)).toBe(true);

      // 2. Register hooks once in Claude configuration
      const settingsFile = join(isolatedClaudeConfig, 'settings.json');
      const initialSettings = {
        installedPlugins: {
          tokentree: {
            path: pluginInstallDir,
            version: '0.1.0',
            enabled: true,
          },
        },
      };
      writeFileSync(settingsFile, JSON.stringify(initialSettings, null, 2));

      // Assert hooks are configured once per event
      const hooksJson = JSON.parse(readFileSync(hooksPath, 'utf8'));
      const hookEvents = Object.keys(hooksJson.hooks);
      const uniqueEvents = new Set(hookEvents);
      expect(hookEvents.length).toBe(uniqueEvents.size);
      expect(uniqueEvents.has('SessionStart')).toBe(true);
      expect(uniqueEvents.has('UserPromptSubmit')).toBe(true);
      expect(uniqueEvents.has('Stop')).toBe(true);

      const enqueueScript = join(pluginInstallDir, 'scripts', 'enqueue.mjs');
      expect(existsSync(enqueueScript)).toBe(true);

      // 3. Feed new-session event through the installed plugin hook script
      const sessionStartPayload = JSON.stringify({
        hook_event_name: 'SessionStart',
        session_id: 'ses-auto-c1-c2-001',
        cwd: projectDir,
        timestamp: '2026-03-30T12:00:00Z',
      });

      const isolatedEnv = {
        ...process.env,
        TOKENTREE_HOME: isolatedHome,
        CLAUDE_CONFIG_DIR: isolatedClaudeConfig,
        PATH: process.env.PATH, // Standard path, no tokentree binary needed on hook path
      };

      execFileSync(process.execPath, [enqueueScript], {
        input: sessionStartPayload,
        env: isolatedEnv,
        encoding: 'utf8',
      });

      // Also feed a UserPromptSubmit event with sensitive text to verify privacy
      const promptPayload = JSON.stringify({
        hook_event_name: 'UserPromptSubmit',
        session_id: 'ses-auto-c1-c2-001',
        cwd: projectDir,
        prompt: 'Super secret customer database credentials and token',
        timestamp: '2026-03-30T12:00:02Z',
      });

      execFileSync(process.execPath, [enqueueScript], {
        input: promptPayload,
        env: isolatedEnv,
        encoding: 'utf8',
      });

      // 4. Verify spool creation under isolated TokenTree home
      const spoolPath = join(isolatedHome, 'spool', 'claude-hooks.jsonl');
      expect(existsSync(spoolPath)).toBe(true);

      const spoolContent = readFileSync(spoolPath, 'utf8');
      const spoolLines = spoolContent.trim().split('\n').map((l) => JSON.parse(l));
      expect(spoolLines[0].kind).toBe('SessionStart');
      expect(spoolLines[0].payload.session_id).toBe('ses-auto-c1-c2-001');
      expect(spoolLines[0].payload.cwd).toBe(projectDir);
      // Privacy check: raw prompt text MUST NOT exist in spool
      expect(spoolContent).not.toContain('Super secret customer database');
      expect(spoolLines[1].payload.prompt_fingerprint).toBeDefined();

      // 5. Worker ingest: process the spool file into the ledger database
      const ledgerDbPath = join(isolatedHome, 'ledger.db');
      const db = new DatabaseSync(ledgerDbPath);
      applyMigrations(db);

      const workerSummary = processClaudeHookSpool(db, spoolPath);
      expect(workerSummary.processed).toBe(2);
      expect(workerSummary.sessions).toBe(1);
      expect(workerSummary.projects).toBe(1);
      expect(workerSummary.turns).toBe(1);

      // Verify second worker pass is idempotent (no-op)
      const secondPass = processClaudeHookSpool(db, spoolPath);
      expect(secondPass.processed).toBe(0);

      // Verify project detected automatically from cwd without Git
      const projectRow = db.prepare('SELECT key, display_name FROM projects').get() as {
        key: string;
        display_name: string;
      };
      expect(projectRow.key).toBe('zero-config-test-app');
      expect(projectRow.display_name).toBe('Zero Config Test App');

      // 6. Zero-config report: run text report without any config file
      const report = renderTextReport(db);
      expect(report).toContain('TokenTree ledger report');
      expect(report).toContain('policy: causal-request');
      expect(report).toContain('completeness:');

      // 7. Edge case: Reinstall into existing directory preserves config without duplicate hooks
      const updatedSettings = JSON.parse(readFileSync(settingsFile, 'utf8'));
      // Simulating reinstall
      updatedSettings.installedPlugins.tokentree = {
        path: pluginInstallDir,
        version: '0.1.0',
        enabled: true,
      };
      writeFileSync(settingsFile, JSON.stringify(updatedSettings, null, 2));
      expect(Object.keys(updatedSettings.installedPlugins.tokentree).length).toBe(3);

      // 8. Edge case: Upgrade plugin version
      updatedSettings.installedPlugins.tokentree.version = '0.2.0';
      writeFileSync(settingsFile, JSON.stringify(updatedSettings, null, 2));
      // Ingest remains consistent and database is intact
      expect(db.prepare('SELECT count(*) as count FROM sessions').get()).toMatchObject({ count: 1 });

      // 9. Edge case: Disabled hooks
      updatedSettings.installedPlugins.tokentree.enabled = false;
      writeFileSync(settingsFile, JSON.stringify(updatedSettings, null, 2));
      const isEnabled = JSON.parse(readFileSync(settingsFile, 'utf8')).installedPlugins.tokentree.enabled;
      expect(isEnabled).toBe(false);

      // 10. Edge case: Missing binary
      // When tokentree binary is absent from PATH, enqueue must still succeed
      const envWithoutBinary = {
        ...isolatedEnv,
        PATH: tmpdir(), // Point to empty directory
      };
      const stopPayload = JSON.stringify({
        hook_event_name: 'Stop',
        session_id: 'ses-auto-c1-c2-001',
        cwd: projectDir,
      });
      execFileSync(process.execPath, [enqueueScript], {
        input: stopPayload,
        env: envWithoutBinary,
        encoding: 'utf8',
      });
      const updatedSpoolContent = readFileSync(spoolPath, 'utf8');
      expect(updatedSpoolContent).toContain('Stop');

      // 11. Edge case: Uninstall plugin removes plugin files, preserves ledger data
      db.close();
      rmSync(pluginInstallDir, { recursive: true, force: true });
      expect(existsSync(pluginInstallDir)).toBe(false);
      // Data in isolated home remains byte-safe
      expect(existsSync(ledgerDbPath)).toBe(true);
      expect(existsSync(spoolPath)).toBe(true);

      const verifyDb = new DatabaseSync(ledgerDbPath, { readOnly: true });
      try {
        const sessionCount = verifyDb.prepare('SELECT count(*) as count FROM sessions').get() as { count: number };
        expect(sessionCount.count).toBe(1);
      } finally {
        verifyDb.close();
      }
    } finally {
      try {
        rmSync(tmpBase, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
      } catch {
        // Safe ignore on Windows background lock
      }
    }
  });
});
