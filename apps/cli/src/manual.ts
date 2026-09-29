// SPDX-License-Identifier: Apache-2.0
import type{DatabaseSync}from'node:sqlite';import{randomUUID}from'node:crypto';import{eventHash,type UsageObservation}from'@tokentreehq/core';import{ingestObservations,stableId}from'@tokentreehq/database';
export interface ManualStart{readonly projectKey:string;readonly projectTitle?:string|undefined;readonly taskTitle:string;readonly parentTitle?:string|undefined;readonly cwd:string;}
export interface ManualCounts{readonly input?:number|undefined;readonly output?:number|undefined;readonly cacheRead?:number|undefined;readonly cacheWrite?:number|undefined;readonly reasoning?:number|undefined;readonly model?:string|undefined;}
export function startManual(db:DatabaseSync,input:ManualStart):{runId:string;projectId:string;workItemId:string;sessionId:string}{
 const now=new Date().toISOString(),projectId=stableId('prj',input.projectKey),parentId=input.parentTitle?stableId('wi',`${projectId}:${input.parentTitle}`):null,workItemId=stableId('wi',`${projectId}:${input.taskTitle}`),runId=`run_${randomUUID()}`,sessionId=stableId('ses',`manual:${runId}`);
 db.exec('BEGIN IMMEDIATE');try{
  db.prepare('INSERT OR IGNORE INTO projects(id,key,display_name,identity_hash,detection_method,confidence,verified_at,created_at,updated_at) VALUES(?,?,?,?,?,1,?,?,?)').run(projectId,input.projectKey,input.projectTitle??input.projectKey,stableId('identity',input.projectKey),'explicit_override',now,now,now);
  if(parentId)db.prepare("INSERT OR IGNORE INTO work_items(id,project_id,type,title,status,confidence,classifier_version,created_at) VALUES(?,?, 'objective',?,'open',1,'manual',?)").run(parentId,projectId,input.parentTitle??'',now);
  db.prepare("INSERT OR IGNORE INTO work_items(id,project_id,parent_id,type,title,status,confidence,classifier_version,created_at) VALUES(?,?,?,'manual_task',?,'open',1,'manual',?)").run(workItemId,projectId,parentId,input.taskTitle,now);
  db.prepare("INSERT INTO sessions(id,adapter,provider_session_id,project_id,cwd,started_at) VALUES(?, 'manual', ?, ?, ?, ?)").run(sessionId,runId,projectId,input.cwd,now);
  db.prepare("INSERT INTO manual_runs(id,session_id,project_id,work_item_id,started_at,state) VALUES(?,?,?,?,?,'active')").run(runId,sessionId,projectId,workItemId,now);db.exec('COMMIT');
 }catch(error){db.exec('ROLLBACK');throw error;}return{runId,projectId,workItemId,sessionId};
}
function valid(value:number|undefined):number|null{if(value===undefined)return null;if(!Number.isSafeInteger(value)||value<0)throw new Error('Token counts must be non-negative integers');return value;}
export function stopManual(db:DatabaseSync,counts:ManualCounts={}):{runId:string;measurementStatus:'measured'|'unavailable'}{
 const run=db.prepare("SELECT id,session_id,project_id,work_item_id FROM manual_runs WHERE state='active' ORDER BY started_at DESC LIMIT 1").get() as {id:string;session_id:string;project_id:string;work_item_id:string}|undefined;if(!run)throw new Error('No active manual run');
 const now=new Date().toISOString(),observation:UsageObservation={adapter:'manual',source:'explicit_cli',sourceEventId:run.id,providerSessionId:run.id,requestId:`manual:${run.id}`,observedAt:now,model:counts.model,inputTokens:valid(counts.input),cachedInputTokens:valid(counts.cacheRead),cacheWriteTokens:valid(counts.cacheWrite),outputTokens:valid(counts.output),reasoningTokens:valid(counts.reasoning),sourcePath:'manual',sourceOffset:0,adapterVersion:'0.1.0',parserVersion:'manual-v1'};
 const measured=[observation.inputTokens,observation.cachedInputTokens,observation.cacheWriteTokens,observation.outputTokens,observation.reasoningTokens].some((value)=>value!==null),summary=ingestObservations(db,[observation]);if(summary.inserted!==1)throw new Error('Manual usage event already exists');
 const eventId=stableId('evt',`manual:request:manual:${run.id}`),spanId=stableId('span',eventId),groupId=stableId('attr',eventHash(observation));
 db.exec('BEGIN IMMEDIATE');try{
  db.prepare('UPDATE sessions SET ended_at=? WHERE id=?').run(now,run.session_id);db.prepare("UPDATE manual_runs SET stopped_at=?,state='stopped' WHERE id=?").run(now,run.id);
  db.prepare('INSERT INTO usage_spans(id,session_id,measurement_status,measured_usage_json,completeness) VALUES(?,?,?,?,?)').run(spanId,run.session_id,measured?'measured':'unavailable',JSON.stringify({usage_event_id:eventId}),measured?100:0);
  db.prepare("INSERT INTO attribution_groups(id,usage_span_id,policy,active,created_at) VALUES(?,?, 'causal-request',1,?)").run(groupId,spanId,now);
  db.prepare("INSERT INTO attributions(group_id,project_id,work_item_id,role,weight_basis_points,method,confidence,verified_by_user) VALUES(?,?,?,'primary',10000,'manual',1,1)").run(groupId,run.project_id,run.work_item_id);db.exec('COMMIT');
 }catch(error){db.exec('ROLLBACK');throw error;}return{runId:run.id,measurementStatus:measured?'measured':'unavailable'};
}
