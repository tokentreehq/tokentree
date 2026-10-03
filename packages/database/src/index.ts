// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { DatabaseSync } from 'node:sqlite';

export const CURRENT_SCHEMA_VERSION = 1;

export function migrationPath(): string {
  return join(dirname(fileURLToPath(import.meta.url)), '..', 'migrations', '0001_initial.sql');
}

export function applyMigrations(db: DatabaseSync, applicationVersion = '0.0.0'): void {
  db.exec('PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;');
  const hasMetadata = db.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_metadata'").get();
  if (hasMetadata) {
    const row = db.prepare('SELECT max(schema_version) AS version FROM schema_metadata WHERE migration_state = ?').get('applied') as { version: number | null };
    if (row.version === CURRENT_SCHEMA_VERSION) return;
    throw new Error(`Unsupported schema version ${String(row.version)}`);
  }
  const sql = readFileSync(migrationPath(), 'utf8');
  const now = new Date().toISOString();
  db.exec('BEGIN EXCLUSIVE');
  try {
    db.exec(sql);
    db.prepare('INSERT INTO schema_metadata(schema_version, application_version, migration_state, created_at, updated_at) VALUES (?, ?, ?, ?, ?)')
      .run(CURRENT_SCHEMA_VERSION, applicationVersion, 'applied', now, now);
    db.exec('COMMIT');
  } catch (error) {
    db.exec('ROLLBACK');
    throw error;
  }
}

export function assertAttributionWeights(db: DatabaseSync, groupId: string): void {
  const row = db.prepare('SELECT coalesce(sum(weight_basis_points), 0) AS total FROM attributions WHERE group_id = ?').get(groupId) as { total: number };
  if (row.total !== 10_000) throw new Error(`Attribution group ${groupId} totals ${row.total} basis points; expected 10000`);
}

