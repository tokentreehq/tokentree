// SPDX-License-Identifier: Apache-2.0
import { copyFileSync, mkdirSync, readFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { ingestObservations, stableId } from '@tokentreehq/database';
import type { UsageObservation } from '@tokentreehq/core';

type Row=Record<string,unknown>;
export interface PrototypePreview {readonly records:number;readonly measured:number;readonly unavailable:number;readonly projects:number;readonly backupPath:string|null;readonly inserted?:number;readonly duplicates?:number;}
function obj(value:unknown):value is Row{return typeof value==='object'&&value!==null&&!Array.isArray(value);}
function rows(value:unknown):Row[]{
 if(Array.isArray(value))return value.filter(obj);
 if(!obj(value))throw new Error('Prototype ledger must be an object or array');
 for(const key of ['records','entries','events','usage','tasks']){const candidate=value[key];if(Array.isArray(candidate))return candidate.filter(obj);}
 return Object.values(value).filter(obj);
}
function str(value:unknown,fallback:string):string{return typeof value==='string'&&value.trim()?value.trim():fallback;}
function token(value:unknown):number|null{return typeof value==='number'&&Number.isSafeInteger(value)&&value>=0?value:null;}
function projectName(row:Row):string{return str(row.project??row.project_name,'Prototype import');}
function taskName(row:Row):string{return str(row.task??row.task_name??row.name,'Imported work');}
export function previewPrototype(sourcePath:string):PrototypePreview{
 const parsed=JSON.parse(readFileSync(sourcePath,'utf8')) as unknown;const items=rows(parsed);let measured=0;
 for(const row of items){if([row.input_tokens,row.input,row.output_tokens,row.output,row.cache_read,row.cached_input_tokens].some((value)=>token(value)!==null))measured++;}
 return {records:items.length,measured,unavailable:items.length-measured,projects:new Set(items.map(projectName)).size,backupPath:null};
}
export function applyPrototype(db:DatabaseSync,sourcePath:string,backupDir:string):PrototypePreview{
 const before=readFileSync(sourcePath);const parsed=JSON.parse(before.toString('utf8')) as unknown;const items=rows(parsed);mkdirSync(backupDir,{recursive:true,mode:0o700});
 const backupPath=join(backupDir,`prototype-${Date.now()}-${basename(sourcePath)}`);copyFileSync(sourcePath,backupPath);
 let inserted=0,duplicates=0,measured=0;
 items.forEach((row,index)=>{
  const project=projectName(row),task=taskName(row),projectId=stableId('prj',project.toLowerCase()),workId=stableId('wi',`${project}:${task}`),sessionIdText=str(row.session_id??row.session,`prototype-${index}`),timestamp=str(row.timestamp??row.created_at,new Date(0).toISOString());
  const observation:UsageObservation={adapter:'prototype',source:'explicit_cli',sourceEventId:`${sourcePath}:${index}`,providerSessionId:sessionIdText,requestId:`prototype:${sourcePath}:${index}`,observedAt:timestamp,sourceTimestamp:timestamp,model:typeof row.model==='string'?row.model:undefined,inputTokens:token(row.input_tokens??row.input),cachedInputTokens:token(row.cached_input_tokens??row.cache_read),cacheWriteTokens:token(row.cache_write_tokens??row.cache_write),outputTokens:token(row.output_tokens??row.output),reasoningTokens:token(row.reasoning_tokens),sourcePath,sourceOffset:index,adapterVersion:'prototype-import-v1',parserVersion:'prototype-import-v1'};
  const rowMeasured=[observation.inputTokens,observation.cachedInputTokens,observation.cacheWriteTokens,observation.outputTokens,observation.reasoningTokens].some((value)=>value!==null);
  if(rowMeasured)measured++;
  db.prepare('INSERT OR IGNORE INTO projects(id,key,display_name,identity_hash,detection_method,confidence,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)').run(projectId,project.toLowerCase().replace(/[^a-z0-9]+/g,'-').replace(/^-|-$/g,'')||projectId,project,stableId('identity',project.toLowerCase()),'prototype_import',1,timestamp,timestamp);
  db.prepare('INSERT OR IGNORE INTO work_items(id,project_id,type,title,status,confidence,classifier_version,created_at) VALUES(?,?,?,?,?,?,?,?)').run(workId,projectId,'legacy_task',task,'open',1,'prototype-import-v1',timestamp);
  const summary=ingestObservations(db,[observation]);inserted+=summary.inserted;duplicates+=summary.duplicates;
  const sessionId=stableId('ses',`prototype:${sessionIdText}`);db.prepare('UPDATE sessions SET project_id=? WHERE id=?').run(projectId,sessionId);
  const eventId=stableId('evt',`prototype:request:prototype:${sourcePath}:${index}`);const spanId=stableId('span',eventId),groupId=stableId('attr',eventId);
  db.prepare("INSERT OR IGNORE INTO usage_spans(id,session_id,measurement_status,measured_usage_json,completeness) VALUES(?,?,?,?,?)").run(spanId,sessionId,rowMeasured?'measured':'unavailable',JSON.stringify({usage_event_id:eventId}),rowMeasured?100:0);
  db.prepare("INSERT OR IGNORE INTO attribution_groups(id,usage_span_id,policy,active,created_at) VALUES(?,?,?,1,?)").run(groupId,spanId,'causal-request',timestamp);
  db.prepare("INSERT OR IGNORE INTO attributions(group_id,project_id,work_item_id,role,weight_basis_points,method,confidence,verified_by_user) VALUES(?,?,?,?,10000,?,1,1)").run(groupId,projectId,workId,'primary','prototype_import');
 });
 if(!readFileSync(sourcePath).equals(before))throw new Error('Prototype source changed during migration');
 return {records:items.length,measured,unavailable:items.length-measured,projects:new Set(items.map(projectName)).size,backupPath,inserted,duplicates};
}
