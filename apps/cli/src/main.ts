#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
import { join } from 'node:path';
import { applyPrototype,doctor,importClaude,openDefaultLedger,previewPrototype,processClaudeHookSpool,reconcile,renderTextReport,resolvePaths,startManual,stopManual } from './index.js';
const args=process.argv.slice(2); const command=args[0]; const paths=resolvePaths();
async function main():Promise<number>{
 if(!command||command==='help'||command==='--help'){console.log('tokentree <doctor|import claude|report --text|reconcile|migrate prototype --preview|--apply|start|stop|classify>');return 0;}
 const db=openDefaultLedger(paths);
 try{
  if(command==='doctor'){const result=doctor(db,paths,join(import.meta.dirname,'../../../packages/pricing/data/prices.json'));console.log(result.lines.join('\n'));return result.ok?0:2;}
  if(command==='import'&&args[1]==='claude'){const path=args[2]??paths.claudeTranscripts;const result=await importClaude(db,path);console.log(JSON.stringify(result,null,2));return result.sessions?0:1;}
  if(command==='report'){if(!args.includes('--text')){console.error('Only --text is available in Phase 1; it never binds a port.');return 1;}console.log(renderTextReport(db));return 0;}
  if(command==='start'){const get=(flag:string)=>{const i=args.indexOf(flag);return i>=0?args[i+1]:undefined;};const project=get('--project'),task=get('--task');if(!project||!task){console.error('start requires --project and --task');return 1;}console.log(JSON.stringify(startManual(db,{projectKey:project,taskTitle:task,parentTitle:get('--parent'),cwd:process.cwd()}),null,2));return 0;}
  if(command==='stop'){const number=(flag:string)=>{const i=args.indexOf(flag);return i>=0&&args[i+1]!==undefined?Number(args[i+1]):undefined;};console.log(JSON.stringify(stopManual(db,{input:number('--input'),output:number('--output'),cacheRead:number('--cache-read'),cacheWrite:number('--cache-write'),reasoning:number('--reasoning'),model:(()=>{const i=args.indexOf('--model');return i>=0?args[i+1]:undefined;})()}),null,2));return 0;}
  if(command==='classify'){const spool=join(paths.tokenTreeHome,'spool','claude-hooks.jsonl');console.log(JSON.stringify(processClaudeHookSpool(db,spool),null,2));return 0;}
  if(command==='reconcile'){console.log(reconcile(db));return 0;}
  if(command==='migrate'&&args[1]==='prototype'){const source=args.find((value)=>value.startsWith('--source='))?.slice(9)??join(process.env.HOME??'', '.task-usage','ledger.json');if(args.includes('--preview')){console.log(JSON.stringify(previewPrototype(source),null,2));return 0;}if(args.includes('--apply')){console.log(JSON.stringify(applyPrototype(db,source,join(paths.tokenTreeHome,'backups')),null,2));return 0;}console.error('Choose --preview or --apply');return 1;}
  console.error(`Unknown command: ${args.join(' ')}`);return 1;
 }finally{db.close();}
}
main().then((code)=>{process.exitCode=code;}).catch((error:unknown)=>{console.error(error instanceof Error?error.message:String(error));process.exitCode=2;});
