// SPDX-License-Identifier: Apache-2.0
import { DatabaseSync } from 'node:sqlite';
import { describe, expect, it } from 'vitest';
import { applyMigrations, assertAttributionWeights, CURRENT_SCHEMA_VERSION } from '../src/index.js';

const expected = ['projects','project_aliases','project_roots','work_items','sessions','turns','usage_events','usage_spans','attribution_groups','attributions','classification_events','pricing_versions','cost_calculations','ingestion_checkpoints','measurement_anomalies','adapter_capabilities','coverage_manifests','schema_metadata','notes'];

describe('initial ledger migration', () => {
  it('creates every PRD table and is idempotent', () => {
    const db = new DatabaseSync(':memory:');
    applyMigrations(db, 'test');
    applyMigrations(db, 'test');
    const tables = db.prepare("SELECT name FROM sqlite_master WHERE type='table'").all().map((row) => String(row.name));
    expect(expected.every((name) => tables.includes(name))).toBe(true);
    expect(db.prepare('SELECT schema_version FROM schema_metadata').get()?.schema_version).toBe(CURRENT_SCHEMA_VERSION);
    db.close();
  });

  it('keeps measured usage append-only and unavailable cost null', () => {
    const db = new DatabaseSync(':memory:');
    applyMigrations(db);
    db.prepare("INSERT INTO usage_events(id,adapter,source_kind,observed_at,ingested_at,event_hash,adapter_version,parser_version) VALUES ('e','claude','fixture','2026-01-01','2026-01-01','h','0','0')").run();
    expect(() => db.prepare("UPDATE usage_events SET model='x' WHERE id='e'").run()).toThrow(/append-only/);
    expect(() => db.prepare("INSERT INTO cost_calculations(usage_event_id,amount_micros,cost_type,attribution_policy,coverage_json,calculated_at) VALUES ('e',0,'unavailable','causal-request','{}','2026-01-01')").run()).toThrow();
    db.close();
  });

  it('validates active attribution weights transactionally', () => {
    const db = new DatabaseSync(':memory:');
    applyMigrations(db);
    db.exec("INSERT INTO projects VALUES ('p','p','P',NULL,'h','fixture',1,NULL,'2026','2026'); INSERT INTO sessions(id,adapter,started_at) VALUES ('s','fixture','2026'); INSERT INTO usage_spans(id,session_id,measurement_status) VALUES ('span','s','measured'); INSERT INTO attribution_groups VALUES ('g','span','causal-request',NULL,1,'2026');");
    db.prepare("INSERT INTO attributions(group_id,project_id,role,weight_basis_points,method) VALUES ('g','p','primary',?, 'manual')").run(9000);
    expect(() => assertAttributionWeights(db, 'g')).toThrow(/10000/);
    db.prepare("INSERT INTO attributions(group_id,project_id,role,weight_basis_points,method) VALUES ('g','p','overhead',?, 'manual')").run(1000);
    expect(() => assertAttributionWeights(db, 'g')).not.toThrow();
    db.close();
  });
});
