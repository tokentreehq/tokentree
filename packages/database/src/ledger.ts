// SPDX-License-Identifier: Apache-2.0
import { createHash } from 'node:crypto';
import { chmodSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { canonicalIdentity, deduplicateObservations, eventHash, hasMeasuredTokens, type UsageObservation } from '@tokentreehq/core';
import { applyMigrations } from './index.js';

export interface IngestAnomaly { readonly type: string; readonly sourcePath: string; readonly sourceOffset: number; readonly sourceValues: unknown; readonly providerSessionId?: string | undefined; readonly turnId?: string | undefined; }
export interface IngestSummary { readonly inserted: number; readonly duplicates: number; readonly unavailable: number; readonly anomalies: number; readonly conflicts: number; }

export function stableId(prefix: string, value: string): string { return `${prefix}_${createHash('sha256').update(value).digest('hex').slice(0,24)}`; }

export function openLedger(path: string, applicationVersion='0.0.0'): DatabaseSync {
  mkdirSync(dirname(path), { recursive:true, mode:0o700 });
  try { chmodSync(dirname(path), 0o700); } catch { /* POSIX permissions unavailable. */ }
  const db = new DatabaseSync(path);
  applyMigrations(db, applicationVersion);
  try { chmodSync(path, 0o600); } catch { /* POSIX permissions unavailable. */ }
  return db;
}

export function ingestObservations(db: DatabaseSync, input: readonly UsageObservation[], parseAnomalies: readonly IngestAnomaly[] = []): IngestSummary {
  const result = deduplicateObservations(input);
  let inserted=0, duplicates=0, unavailable=0, anomalyCount=0;
  db.exec('BEGIN IMMEDIATE');
  try {
    for (const item of result.canonical) {
      const sessionId=stableId('ses',`${item.adapter}:${item.providerSessionId}`);
      db.prepare(`INSERT OR IGNORE INTO sessions(id,adapter,provider_session_id,source_path,started_at)
        VALUES (?,?,?,?,?)`).run(sessionId,item.adapter,item.providerSessionId,item.sourcePath,item.sourceTimestamp ?? item.observedAt);
      const info=db.prepare(`INSERT OR IGNORE INTO usage_events(
        id,adapter,source_kind,source_event_id,source_process_id,source_sequence,session_id,prompt_id,turn_id,request_id,agent_id,parent_agent_id,source_timestamp,observed_at,ingested_at,model,service_tier,region,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,reasoning_tokens,provider_reported_cost_micros,source_path,source_offset,event_hash,adapter_version,parser_version
      ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`).run(
        stableId('evt',canonicalIdentity(item)),item.adapter,item.source,item.sourceEventId ?? null,item.sourceProcessId ?? null,item.sourceSequence ?? null,sessionId,null,null,item.requestId ?? null,item.agentId ?? null,item.parentAgentId ?? null,item.sourceTimestamp ?? null,item.observedAt,new Date().toISOString(),item.model ?? null,item.serviceTier ?? null,item.region ?? null,item.inputTokens,item.cachedInputTokens,item.cacheWriteTokens,item.outputTokens,item.reasoningTokens,item.providerReportedCostMicros ?? null,item.sourcePath,item.sourceOffset,eventHash(item),item.adapterVersion,item.parserVersion
      );
      if (info.changes === 1) { inserted++; if (!hasMeasuredTokens(item)) unavailable++; } else duplicates++;
    }
    for (const conflict of result.conflicts) {
      const item=conflict.kept;
      insertAnomaly(db,{ type:'source_conflict',sourcePath:item.sourcePath,sourceOffset:item.sourceOffset,sourceValues:{ identity:conflict.identity, keptSource:item.source, rejectedSource:conflict.rejected.source },providerSessionId:item.providerSessionId,turnId:item.turnId });
      anomalyCount++;
    }
    for (const anomaly of parseAnomalies) { insertAnomaly(db,anomaly); anomalyCount++; }
    db.exec('COMMIT');
  } catch (error) { db.exec('ROLLBACK'); throw error; }
  return { inserted,duplicates,unavailable,anomalies:anomalyCount,conflicts:result.conflicts.length };
}

function insertAnomaly(db: DatabaseSync, value: IngestAnomaly): void {
  const sessionId=value.providerSessionId ? stableId('ses',`claude:${value.providerSessionId}`) : null;
  db.prepare(`INSERT OR IGNORE INTO measurement_anomalies(id,session_id,turn_id,type,source_values_json,created_at)
    VALUES (?,?,?,?,?,?)`).run(stableId('anom',`${value.type}:${value.sourcePath}:${value.sourceOffset}`),sessionId,null,value.type,JSON.stringify(value.sourceValues),new Date().toISOString());
}

export class LedgerWriter {
  private tail: Promise<void> = Promise.resolve();
  enqueue<T>(work:(db:DatabaseSync)=>T, db:DatabaseSync): Promise<T> {
    const next=this.tail.then(()=>work(db));
    this.tail=next.then(()=>undefined,()=>undefined);
    return next;
  }
  async drain(): Promise<void> { await this.tail; }
}
