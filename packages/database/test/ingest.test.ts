// SPDX-License-Identifier: Apache-2.0
import { DatabaseSync } from 'node:sqlite';
import { describe, expect, it } from 'vitest';
import type { UsageObservation } from '@tokentreehq/core';
import { applyMigrations, ingestObservations } from '../src/index.js';
const row:UsageObservation={adapter:'claude',source:'transcript_request',providerSessionId:'s',requestId:'r',observedAt:'2026',inputTokens:10,cachedInputTokens:null,cacheWriteTokens:null,outputTokens:2,reasoningTokens:null,sourcePath:'fixture',sourceOffset:0,adapterVersion:'a',parserVersion:'p'};
describe('ledger ingest',()=>{
  it('is idempotent across replay',()=>{const db=new DatabaseSync(':memory:');applyMigrations(db);expect(ingestObservations(db,[row]).inserted).toBe(1);expect(ingestObservations(db,[row]).duplicates).toBe(1);expect(db.prepare('select count(*) n from usage_events').get()?.n).toBe(1);db.close();});
  it('deduplicates observations by truth precedence',()=>{const db=new DatabaseSync(':memory:');applyMigrations(db);const official={...row,source:'official_telemetry' as const,inputTokens:11};const result=ingestObservations(db,[row,official]);expect(result.inserted).toBe(1);expect(result.conflicts).toBe(1);expect(db.prepare('select source_kind,input_tokens from usage_events').get()).toMatchObject({source_kind:'official_telemetry',input_tokens:11});db.close();});
});
