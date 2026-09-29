// SPDX-License-Identifier: Apache-2.0
import { existsSync, readFileSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { tokenCompleteness } from '@tokentreehq/core';
import { ingestObservations, openLedger, stableId } from '@tokentreehq/database';
import { ensureSessionAttribution } from './attribution.js';
import { discoverClaudeSessions, parseClaudeSession } from '@tokentreehq/adapter-claude';

export const DISCLAIMER='Amounts are list-price estimates from public per-token rates unless labeled otherwise. They are not your provider invoice, prepaid credit balance, or subscription allowance.';
export interface ResolvedPaths { readonly claudeTranscripts:string; readonly codexSessions:string; readonly tokenTreeHome:string; readonly database:string; }
export function resolvePaths(platform:NodeJS.Platform=process.platform,home=homedir(),override=process.env.TOKENTREE_HOME):ResolvedPaths {
  const tokenTreeHome=override || join(home,'.tokentree');
  if (platform==='win32') return {claudeTranscripts:join(home,'.claude','projects'),codexSessions:join(home,'.codex','sessions'),tokenTreeHome,database:join(tokenTreeHome,'ledger.db')};
  return {claudeTranscripts:join(home,'.claude','projects'),codexSessions:join(home,'.codex','sessions'),tokenTreeHome,database:join(tokenTreeHome,'ledger.db')};
}
export interface DoctorResult { readonly ok:boolean; readonly lines:string[]; }
export function doctor(db:DatabaseSync,paths:ResolvedPaths,pricesPath?:string):DoctorResult {
  const lines=[`database: ${paths.database}`,`Claude transcripts: ${paths.claudeTranscripts}`,`Codex sessions: ${paths.codexSessions}`,`TokenTree home: ${paths.tokenTreeHome}`];
  lines.push(`Claude capture mode: ${existsSync(paths.claudeTranscripts)?'logs (historical fallback; automatic hooks not verified)':'manual/unavailable'}`);
  const integrity=db.prepare('PRAGMA integrity_check').get() as {integrity_check:string};
  let ok=integrity.integrity_check==='ok'; lines.push(`database integrity: ${integrity.integrity_check}`);
  const leaks=promptLeakFindings(db); if(leaks.length){ok=false;lines.push(`prompt leakage: FAIL (${leaks.join('; ')})`);}else lines.push('prompt leakage: PASS');
  if(pricesPath&&existsSync(pricesPath)){
    const parsed=JSON.parse(readFileSync(pricesPath,'utf8')) as {updated?:string}; const age=parsed.updated?Math.floor((Date.now()-new Date(parsed.updated).getTime())/86_400_000):Infinity;
    lines.push(`prices: ${Number.isFinite(age)?`${age} days old${age>90?' (STALE)':''}`:'invalid updated date'}`);
  } else lines.push('prices: unavailable');
  return {ok,lines};
}
export function promptLeakFindings(db:DatabaseSync):string[] {
  const findings:string[]=[];
  const badModes=db.prepare("SELECT count(*) n FROM turns WHERE prompt_storage_mode NOT IN ('none','fingerprint_only','redacted_label')").get() as {n:number};
  if(badModes.n) findings.push(`${badModes.n} invalid prompt storage modes`);
  const labels=db.prepare('SELECT derived_label FROM turns WHERE derived_label IS NOT NULL').all() as Array<{derived_label:string}>;
  const long=labels.filter((row)=>row.derived_label.trim().split(/\s+/).length>8).length; if(long)findings.push(`${long} labels exceed 8 words`);
  const schema=db.prepare("SELECT sql FROM sqlite_master WHERE type='table'").all() as Array<{sql:string|null}>;
  if(schema.some((row)=>/\b(prompt_text|completion|reasoning|tool_payload|tool_output)\b/i.test(row.sql??''))) findings.push('forbidden content column present');
  return findings;
}
export async function importClaude(db:DatabaseSync,root:string):Promise<{sessions:number;inserted:number;duplicates:number;unknown:number;malformed:number;anomalies:number}> {
  let sessions=0,inserted=0,duplicates=0,unknown=0,malformed=0,anomalies=0;
  for(const ref of discoverClaudeSessions(root)){
    const parsed=await parseClaudeSession(ref); const summary=ingestObservations(db,parsed.observations,parsed.anomalies.map((item)=>({...item,providerSessionId:ref.providerSessionId})));
    ensureSessionAttribution(db,stableId('ses',`claude:${ref.providerSessionId}`));
    sessions++;inserted+=summary.inserted;duplicates+=summary.duplicates;unknown+=parsed.stats.unknown;malformed+=parsed.stats.malformed;anomalies+=summary.anomalies;
    const stat=statSync(ref.sourcePath); const last=parsed.observations.at(-1);
    db.prepare(`INSERT INTO ingestion_checkpoints(adapter,source_path,file_size,modified_at,last_offset,last_event_hash) VALUES ('claude',?,?,?,?,?) ON CONFLICT(adapter,source_path) DO UPDATE SET file_size=excluded.file_size,modified_at=excluded.modified_at,last_offset=excluded.last_offset,last_event_hash=excluded.last_event_hash`).run(ref.sourcePath,stat.size,stat.mtime.toISOString(),stat.size,last?.requestId??null);
  }
  return {sessions,inserted,duplicates,unknown,malformed,anomalies};
}
export function renderTextReport(db:DatabaseSync):string {
  const usage=db.prepare(`SELECT count(*) total,
    sum(CASE WHEN input_tokens IS NOT NULL OR cached_input_tokens IS NOT NULL OR cache_write_tokens IS NOT NULL OR output_tokens IS NOT NULL OR reasoning_tokens IS NOT NULL THEN 1 ELSE 0 END) measured,
    sum(CASE WHEN input_tokens IS NULL AND cached_input_tokens IS NULL AND cache_write_tokens IS NULL AND output_tokens IS NULL AND reasoning_tokens IS NULL THEN 1 ELSE 0 END) unavailable,
    coalesce(sum(input_tokens),0) input_tokens,coalesce(sum(cached_input_tokens),0) cached_tokens,coalesce(sum(cache_write_tokens),0) cache_write_tokens,coalesce(sum(output_tokens),0) output_tokens,coalesce(sum(reasoning_tokens),0) reasoning_tokens FROM usage_events`).get() as Record<string,number>;
  const anomalies=(db.prepare('SELECT count(*) n FROM measurement_anomalies WHERE resolved_at IS NULL').get() as {n:number}).n;
  const completeness=tokenCompleteness({measuredRequests:usage.measured??0,unavailableRequests:usage.unavailable??0,anomalousRequests:anomalies});
  return [`TokenTree ledger report`,`requests: ${usage.total??0} measured ${usage.measured??0} unavailable ${usage.unavailable??0} anomalies ${anomalies}`,`tokens: input ${usage.input_tokens??0} cache-read ${usage.cached_tokens??0} cache-write ${usage.cache_write_tokens??0} output ${usage.output_tokens??0} reasoning ${usage.reasoning_tokens??0}`,`completeness: ${completeness===null?'—':`${completeness.toFixed(1)}%`}`,`policy: causal-request`,`cost: unavailable (no verified matching rates)`].join('\n');
}
export function reconcile(db:DatabaseSync):string {
  const duplicateRequests=db.prepare(`SELECT count(*) n FROM (SELECT request_id FROM usage_events WHERE request_id IS NOT NULL GROUP BY request_id HAVING count(*)>1)`).get() as {n:number};
  const unresolved=db.prepare('SELECT count(*) n FROM measurement_anomalies WHERE resolved_at IS NULL').get() as {n:number};
  const sessions=db.prepare('SELECT count(*) n FROM sessions').get() as {n:number};
  return [`sessions: ${sessions.n}`,`duplicate canonical request IDs: ${duplicateRequests.n}`,`unresolved anomalies: ${unresolved.n}`,`subagent reconciliation: unavailable until adapter capability is verified`].join('\n');
}
export function openDefaultLedger(paths=resolvePaths()):DatabaseSync{return openLedger(paths.database,'0.1.0');}
export { applyPrototype, previewPrototype } from './prototype.js';
export type { PrototypePreview } from './prototype.js';
export { startManual, stopManual } from './manual.js';
export type { ManualCounts, ManualStart } from './manual.js';
export { processClaudeHookSpool } from './hooks-worker.js';
export type { HookWorkerSummary } from './hooks-worker.js';
export { applyPriceSnapshot } from './pricing-ledger.js';
export type { PricingSummary } from './pricing-ledger.js';
export { addNote, attachSession, detachSession, ensureSessionAttribution } from './attribution.js';
export { loadProjectTrees, queryLedger, renderProjectTrees } from './tree.js';
export type { ProjectTree, UsageTotals, WorkTreeNode } from './tree.js';
