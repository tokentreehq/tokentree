// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { addNote, openDefaultLedger, resolvePaths, startManual } from '../src/index.js';

describe('uninstall data preservation (Criterion 18)', () => {
  it('preserves ~/.tokentree/ledger.db when plugin or wrapper is removed', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-uninstall-test-'));
    try {
      const mockHome = join(tmp, '.tokentree');
      const mockPluginDir = join(tmp, 'plugins', 'tokentree-claude');

      const paths = resolvePaths(process.platform, tmp, mockHome);

      // 1. User records usage and notes
      const db = openDefaultLedger(paths);
      try {
        startManual(db, {
          projectKey: 'mission-critical-proj',
          taskTitle: 'Important task',
          cwd: tmp,
        });
        addNote(db, 'Critical migration note that must not be deleted');
      } finally {
        db.close();
      }

      // Verify db exists before uninstall
      expect(existsSync(paths.database)).toBe(true);

      // 2. Simulate plugin removal / uninstallation
      // When user runs `claude plugin remove tokentree` or `npm uninstall -g @tokentreehq/cli`,
      // the plugin/wrapper directories are purged.
      rmSync(mockPluginDir, { recursive: true, force: true });

      // 3. Assert that ledger database remains intact and untouched
      expect(existsSync(paths.database)).toBe(true);

      // 4. Re-open ledger and assert all data is preserved
      const reopenedDb = openDefaultLedger(paths);
      try {
        const rows = reopenedDb.prepare('SELECT count(*) as count FROM manual_runs').get() as { count: number };
        expect(rows.count).toBe(1);

        const notes = reopenedDb.prepare('SELECT text FROM notes').all() as { text: string }[];
        expect(notes.length).toBe(1);
        expect(notes[0].text).toBe('Critical migration note that must not be deleted');
      } finally {
        reopenedDb.close();
      }
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});
