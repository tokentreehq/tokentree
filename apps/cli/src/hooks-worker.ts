// SPDX-License-Identifier: Apache-2.0
import{readFileSync,statSync}from'node:fs';import{dirname}from'node:path';import type{DatabaseSync}from'node:sqlite';import{resolveProject}from'@tokentreehq/core';import{stableId}from'@tokentreehq/database';
type Payload=Record<string,unknown>;interface Envelope{version:1;kind:string;capturedAt:string;payload:Payload;}
const text=(value:unknown):string|undefined=>typeof value==='string'&&value?value:undefined;
export interface HookWorkerSummary{readonly processed:number;readonly skipped:number;readonly projects:number;readonly sessions:number;readonly turns:number;readonly pendingClassify:number;}
export function processClaudeHookSpool(db:DatabaseSync,path:string):HookWorkerSummary{
 const stat=statSync(path),checkpoint=db.prepare("SELECT last_offset FROM ingestion_checkpoints WHERE adapter='claude-hook' AND source_path=?").get(path) as {last_offset:number}|undefined,start=checkpoint?.last_offset??0,buffer=readFileSync(path),slice=buffer.subarray(Math.min(start,buffer.length)).toString('utf8'),endsWithNewline=slice.endsWith('\n'),parts=slice.split(/\n/);if(!endsWithNewline)parts.pop();
 let processed=0,skipped=0,projects=0,sessions=0,turns=0,pendingClassify=0,consumed=0;
 db.exec('BEGIN IMMEDIATE');try{
  for(const line of parts){consumed+=Buffer.byteLength(line)+1;if(!line.trim())continue;let event:Envelope;try{event=JSON.parse(line) as Envelope;}catch{skipped++;continue;}if(event.version!==1||typeof event.kind!=='string'||!event.payload){skipped++;continue;}
   const sessionKey=text(event.payload.session_id),cwd=text(event.payload.cwd)??dirname(path);if(!sessionKey){skipped++;continue;}const project=resolveProject({cwd}),projectId=stableId('prj',project.key),sessionId=stableId('ses',`claude:${sessionKey}`),now=event.capturedAt;
   const projectChange=db.prepare('INSERT OR IGNORE INTO projects(id,key,display_name,identity_hash,detection_method,confidence,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)').run(projectId,project.key,project.displayName,stableId('identity',`${project.method}:${project.root}`),project.method,project.confidence,now,now);projects+=Number(projectChange.changes);
   db.prepare('INSERT OR IGNORE INTO project_roots(project_id,canonical_path,root_type,fingerprint,active,first_seen_at,last_seen_at) VALUES(?,?,?,?,1,?,?)').run(projectId,project.root,project.method,stableId('root',project.root),now,now);
   const sessionChange=db.prepare("INSERT OR IGNORE INTO sessions(id,adapter,provider_session_id,project_id,source_path,cwd,started_at) VALUES(?,'claude',?,?,?,?,?)").run(sessionId,sessionKey,projectId,text(event.payload.transcript_path)??null,cwd,now);sessions+=Number(sessionChange.changes);db.prepare('UPDATE sessions SET project_id=?,cwd=? WHERE id=?').run(projectId,cwd,sessionId);
   if(event.kind==='UserPromptSubmit'){
    const sequence=(db.prepare('SELECT count(*) n FROM turns WHERE session_id=?').get(sessionId) as {n:number}).n,turnId=stableId('turn',`${sessionId}:${text(event.payload.prompt_id)??sequence}`),fingerprint=text(event.payload.prompt_fingerprint)??null,workId=stableId('wi',`${projectId}:uncategorized`);
    db.prepare("INSERT OR IGNORE INTO work_items(id,project_id,type,title,status,confidence,classifier_version,created_at) VALUES(?,?,'inbox','Uncategorized','open',0,'pending',?)").run(workId,projectId,now);
    const turnChange=db.prepare("INSERT OR IGNORE INTO turns(id,session_id,sequence_number,started_at,prompt_fingerprint,prompt_storage_mode) VALUES(?,?,?,?,?,'fingerprint_only')").run(turnId,sessionId,sequence,now,fingerprint);turns+=Number(turnChange.changes);if(turnChange.changes){db.prepare("INSERT INTO classification_events(id,turn_id,outcome,signals_json,score,classifier_version,created_at) VALUES(?,?,'UNCERTAIN','[\"prompt_not_persisted\",\"pending_worker\"]',0,'pending',?)").run(stableId('class',turnId),turnId,now);pendingClassify++;}
   }else if(event.kind==='Stop'||event.kind==='StopFailure'){db.prepare('UPDATE turns SET ended_at=? WHERE id=(SELECT id FROM turns WHERE session_id=? AND ended_at IS NULL ORDER BY sequence_number DESC LIMIT 1)').run(now,sessionId);
   }else if(event.kind==='SessionEnd'){db.prepare('UPDATE sessions SET ended_at=? WHERE id=?').run(now,sessionId);}
   processed++;
  }
  const finalOffset=start+consumed;db.prepare(`INSERT INTO ingestion_checkpoints(adapter,source_path,file_size,modified_at,last_offset,last_event_hash) VALUES('claude-hook',?,?,?,?,NULL) ON CONFLICT(adapter,source_path) DO UPDATE SET file_size=excluded.file_size,modified_at=excluded.modified_at,last_offset=excluded.last_offset`).run(path,stat.size,stat.mtime.toISOString(),finalOffset);db.exec('COMMIT');
 }catch(error){db.exec('ROLLBACK');throw error;}
 return{processed,skipped,projects,sessions,turns,pendingClassify};
}
