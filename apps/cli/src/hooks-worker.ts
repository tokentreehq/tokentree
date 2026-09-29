// SPDX-License-Identifier: Apache-2.0
import { dirname } from 'node:path';
import { statSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';
import { resolveProject } from '@tokentreehq/core';
import { stableId } from '@tokentreehq/database';
import { optionalText, readCompleteHookRecords, type HookEnvelope } from './hook-records.js';

export interface HookWorkerSummary {
  readonly processed: number;
  readonly skipped: number;
  readonly projects: number;
  readonly sessions: number;
  readonly turns: number;
  readonly pendingClassify: number;
}

interface Counters {
  projects: number;
  sessions: number;
  turns: number;
  pendingClassify: number;
}

export function processClaudeHookSpool(db: DatabaseSync, path: string): HookWorkerSummary {
  const stat = statSync(path);
  const checkpoint = db.prepare(
    "SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook' AND source_path=?",
  ).get(path) as { last_offset: number } | undefined;
  const startOffset = checkpoint?.last_offset ?? 0;
  const batch = readCompleteHookRecords(path, startOffset);
  const counters: Counters = { projects: 0, sessions: 0, turns: 0, pendingClassify: 0 };

  db.exec('BEGIN IMMEDIATE');
  try {
    for (const event of batch.records) applyHookEvent(db, event, path, counters);
    saveCheckpoint(db, path, stat.size, stat.mtime.toISOString(), startOffset + batch.consumedBytes);
    db.exec('COMMIT');
  } catch (error) {
    db.exec('ROLLBACK');
    throw error;
  }

  return {
    processed: batch.records.length,
    skipped: batch.skipped,
    ...counters,
  };
}

function applyHookEvent(db: DatabaseSync, event: HookEnvelope, spoolPath: string, counters: Counters): void {
  const sessionKey = optionalText(event.payload.session_id);
  if (!sessionKey) return;

  const cwd = optionalText(event.payload.cwd) ?? dirname(spoolPath);
  const project = resolveProject({ cwd });
  const projectId = stableId('prj', project.key);
  const sessionId = stableId('ses', `claude:${sessionKey}`);

  counters.projects += createProject(db, projectId, project, event.capturedAt);
  counters.sessions += createSession(db, sessionId, sessionKey, projectId, cwd, event);
  db.prepare('UPDATE sessions SET project_id=?, cwd=? WHERE id=?').run(projectId, cwd, sessionId);

  switch (event.kind) {
    case 'UserPromptSubmit':
      createPendingTurn(db, sessionId, projectId, event, counters);
      break;
    case 'Stop':
    case 'StopFailure':
      endLatestTurn(db, sessionId, event.capturedAt);
      break;
    case 'SessionEnd':
      db.prepare('UPDATE sessions SET ended_at=? WHERE id=?').run(event.capturedAt, sessionId);
      break;
    default:
      break;
  }
}

function createProject(
  db: DatabaseSync,
  projectId: string,
  project: ReturnType<typeof resolveProject>,
  capturedAt: string,
): number {
  const change = db.prepare(
    'INSERT OR IGNORE INTO projects(id,key,display_name,identity_hash,detection_method,confidence,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)',
  ).run(
    projectId,
    project.key,
    project.displayName,
    stableId('identity', `${project.method}:${project.root}`),
    project.method,
    project.confidence,
    capturedAt,
    capturedAt,
  );
  db.prepare(
    'INSERT OR IGNORE INTO project_roots(project_id,canonical_path,root_type,fingerprint,active,first_seen_at,last_seen_at) VALUES(?,?,?,?,1,?,?)',
  ).run(projectId, project.root, project.method, stableId('root', project.root), capturedAt, capturedAt);
  return Number(change.changes);
}

function createSession(
  db: DatabaseSync,
  sessionId: string,
  sessionKey: string,
  projectId: string,
  cwd: string,
  event: HookEnvelope,
): number {
  const change = db.prepare(
    "INSERT OR IGNORE INTO sessions(id,adapter,provider_session_id,project_id,source_path,cwd,started_at) VALUES(?,'claude',?,?,?,?,?)",
  ).run(
    sessionId,
    sessionKey,
    projectId,
    optionalText(event.payload.transcript_path) ?? null,
    cwd,
    event.capturedAt,
  );
  return Number(change.changes);
}

function createPendingTurn(
  db: DatabaseSync,
  sessionId: string,
  projectId: string,
  event: HookEnvelope,
  counters: Counters,
): void {
  const sequence = (db.prepare('SELECT count(*) n FROM turns WHERE session_id=?').get(sessionId) as { n: number }).n;
  const turnIdentity = optionalText(event.payload.prompt_id) ?? String(sequence);
  const turnId = stableId('turn', `${sessionId}:${turnIdentity}`);
  const workItemId = stableId('wi', `${projectId}:uncategorized`);

  db.prepare(
    "INSERT OR IGNORE INTO work_items(id,project_id,type,title,status,confidence,classifier_version,created_at) VALUES(?,?,'inbox','Uncategorized','open',0,'pending',?)",
  ).run(workItemId, projectId, event.capturedAt);

  const change = db.prepare(
    "INSERT OR IGNORE INTO turns(id,session_id,sequence_number,started_at,prompt_fingerprint,prompt_storage_mode) VALUES(?,?,?,?,?,'fingerprint_only')",
  ).run(turnId, sessionId, sequence, event.capturedAt, optionalText(event.payload.prompt_fingerprint) ?? null);
  counters.turns += Number(change.changes);

  if (!change.changes) return;
  db.prepare(
    "INSERT INTO classification_events(id,turn_id,outcome,signals_json,score,classifier_version,created_at) VALUES(?,?,'UNCERTAIN','[\"prompt_not_persisted\",\"pending_worker\"]',0,'pending',?)",
  ).run(stableId('class', turnId), turnId, event.capturedAt);
  counters.pendingClassify++;
}

function endLatestTurn(db: DatabaseSync, sessionId: string, capturedAt: string): void {
  db.prepare(
    'UPDATE turns SET ended_at=? WHERE id=(SELECT id FROM turns WHERE session_id=? AND ended_at IS NULL ORDER BY sequence_number DESC LIMIT 1)',
  ).run(capturedAt, sessionId);
}

function saveCheckpoint(
  db: DatabaseSync,
  path: string,
  fileSize: number,
  modifiedAt: string,
  lastOffset: number,
): void {
  db.prepare(
    `INSERT INTO ingestion_checkpoints(adapter,source_path,file_size,modified_at,last_offset,last_event_hash)
     VALUES('claude-hook',?,?,?,?,NULL)
     ON CONFLICT(adapter,source_path) DO UPDATE SET
       file_size=excluded.file_size,
       modified_at=excluded.modified_at,
       last_offset=excluded.last_offset`,
  ).run(path, fileSize, modifiedAt, lastOffset);
}
