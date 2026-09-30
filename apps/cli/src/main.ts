#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { addNote,applyPriceSnapshot,DISCLAIMER,applyPrototype,attachSession,detachSession,doctor,importClaude,loadProjectTrees,openDefaultLedger,previewPrototype,processClaudeHookSpool,queryLedger,reconcile,renderProjectTrees,renderTextReport,resolvePaths,startManual,stopManual } from './index.js';

import { findNativeBinary } from './launcher.js';

if (!process.env.TOKENTREE_FORCE_JS) {
  const nativeBin = findNativeBinary();
  if (nativeBin) {
    const result = spawnSync(nativeBin, process.argv.slice(2), { stdio: 'inherit' });
    process.exit(result.status ?? (result.signal ? 1 : 0));
  }
}

const args=process.argv.slice(2); const command=args[0]; const paths=resolvePaths(); const pricesPath=join(import.meta.dirname,'../../../packages/pricing/data/prices.json');
async function main():Promise<number>{
 if(!command||command==='help'||command==='--help'){console.log('tokentree <doctor|import claude|report --text|query|attach|detach|note|reconcile|migrate prototype --preview|--apply|start|stop|classify>');return 0;}
 const db=openDefaultLedger(paths);
 try{
  if(command==='doctor'){const result=doctor(db,paths,pricesPath);console.log(result.lines.join('\n'));return result.ok?0:2;}
  if(command==='import'&&args[1]==='claude'){const path=args[2]??paths.claudeTranscripts;const result=await importClaude(db,path);console.log(JSON.stringify(result,null,2));return result.sessions?0:1;}
  if(command==='report'){if(!args.includes('--text')){console.error('Use --text; it never binds a port.');return 1;}applyPriceSnapshot(db,pricesPath);const i=args.indexOf('--project');console.log(renderProjectTrees(loadProjectTrees(db,i>=0?args[i+1]:undefined)));console.log(`\n${renderTextReport(db)}\n\n${DISCLAIMER}`);return 0;}
  if(command==='query'){applyPriceSnapshot(db,pricesPath);const get=(flag:string)=>{const i=args.indexOf(flag);return i>=0?args[i+1]:undefined;};console.log(JSON.stringify(queryLedger(db,{project:get('--project'),workItem:get('--work-item'),includeDescendants:args.includes('--include-descendants')}),null,2));return 0;}
  if(command==='attach'){const get=(flag:string)=>{const i=args.indexOf(flag);return i>=0?args[i+1]:undefined;};const session=get('--session'),task=get('--task');if(!session||!task){console.error('attach requires --session and --task');return 1;}console.log(JSON.stringify({changed:attachSession(db,session,task)}));return 0;}
  if(command==='detach'){const i=args.indexOf('--session'),session=i>=0?args[i+1]:undefined;if(!session){console.error('detach requires --session');return 1;}console.log(JSON.stringify({changed:detachSession(db,session)}));return 0;}
  if(command==='note'){const ti=args.indexOf('--text'),wi=args.indexOf('--task'),text=ti>=0?args[ti+1]:undefined;if(!text){console.error('note requires --text');return 1;}console.log(JSON.stringify({id:addNote(db,text,wi>=0?args[wi+1]:undefined)}));return 0;}
  if(command==='start'){const get=(flag:string)=>{const i=args.indexOf(flag);return i>=0?args[i+1]:undefined;};const project=get('--project'),task=get('--task');if(!project||!task){console.error('start requires --project and --task');return 1;}console.log(JSON.stringify(startManual(db,{projectKey:project,taskTitle:task,parentTitle:get('--parent'),cwd:process.cwd()}),null,2));return 0;}
  if(command==='stop'){const number=(flag:string)=>{const i=args.indexOf(flag);return i>=0&&args[i+1]!==undefined?Number(args[i+1]):undefined;};console.log(JSON.stringify(stopManual(db,{input:number('--input'),output:number('--output'),cacheRead:number('--cache-read'),cacheWrite:number('--cache-write'),reasoning:number('--reasoning'),model:(()=>{const i=args.indexOf('--model');return i>=0?args[i+1]:undefined;})()}),null,2));return 0;}
  if(command==='classify'){const spool=join(paths.tokenTreeHome,'spool','claude-hooks.jsonl');console.log(JSON.stringify(processClaudeHookSpool(db,spool),null,2));return 0;}
  if(command==='reconcile'){console.log(reconcile(db));return 0;}
  if(command==='migrate'&&args[1]==='prototype'){const source=args.find((value)=>value.startsWith('--source='))?.slice(9)??join(process.env.HOME??'', '.task-usage','ledger.json');if(args.includes('--preview')){console.log(JSON.stringify(previewPrototype(source),null,2));return 0;}if(args.includes('--apply')){console.log(JSON.stringify(applyPrototype(db,source,join(paths.tokenTreeHome,'backups')),null,2));return 0;}console.error('Choose --preview or --apply');return 1;}
  console.error(`Unknown command: ${args.join(' ')}`);return 1;
 }finally{db.close();}
}
main().then((code)=>{process.exitCode=code;}).catch((error:unknown)=>{console.error(error instanceof Error?error.message:String(error));process.exitCode=2;});
