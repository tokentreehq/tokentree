#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { addNote,applyPriceSnapshot,DISCLAIMER,applyPrototype,attachSession,detachSession,doctor,importClaude,loadProjectTrees,openDefaultLedger,previewPrototype,processClaudeHookSpool,queryLedger,reconcile,renderProjectTrees,renderTextReport,resolvePaths,startManual,stopManual } from './index.js';

import { findNativeBinary, resolvePlatformTarget } from './launcher.js';

// The TypeScript CLI is a launcher for the Rust measurement engine — it is
// not a second implementation. Per the locked Rust-first architecture decision
// (docs/decisions.md), it must fail clearly when no supported native binary
// is present and must never silently fall back to the JS engine. The JS engine
// below remains only as an explicit opt-in reference harness
// (TOKENTREE_FORCE_JS=1); it is not the production measurement path.
if (process.env.TOKENTREE_FORCE_JS) {
  console.error('tokentree: TOKENTREE_FORCE_JS=1 — running the TypeScript reference engine (not the production measurement path).');
} else {
  const nativeBin = findNativeBinary();
  if (!nativeBin) {
    const target = resolvePlatformTarget(process.platform, process.arch);
    const vendorDir = target
      ? join(import.meta.dirname, '..', 'vendor', target.rustTarget)
      : join(import.meta.dirname, '..', 'vendor');
    console.error(
      'tokentree: no native tokentree binary found for this platform.\n' +
      'The @tokentreehq/cli package is a launcher for the Rust measurement engine and cannot run without it.\n' +
      'Install the native binary:\n' +
      '  1. Download the archive for your platform from https://github.com/tokentreehq/tokentree/releases\n' +
      `  2. Extract the \`${target?.binaryName ?? 'tokentree'}\` binary into ${vendorDir}\n` +
      '     (or set TOKENTREE_BIN=/path/to/tokentree to point at an existing binary).'
    );
    process.exit(1);
  }
  const result = spawnSync(nativeBin, process.argv.slice(2), { stdio: 'inherit' });
  process.exit(result.status ?? (result.signal ? 1 : 0));
}

// prices.json ships inside the published npm package at <pkg>/data/prices.json
// (see `files` in apps/cli/package.json; staged by apps/cli/scripts/build.ts).
// In a repo checkout it also lives at packages/pricing/data/prices.json.
// Resolve the published layout first so the installed package finds its data.
function resolvePricesPath(): string {
  const publishedLayout = join(import.meta.dirname, '..', 'data', 'prices.json');
  const repoLayout = join(import.meta.dirname, '..', '..', '..', 'packages', 'pricing', 'data', 'prices.json');
  for (const candidate of [publishedLayout, repoLayout]) {
    if (existsSync(candidate)) return candidate;
  }
  return publishedLayout;
}

const args=process.argv.slice(2); const command=args[0]; const paths=resolvePaths(); const pricesPath=resolvePricesPath();
async function main():Promise<number>{
 if(!command||command==='help'||command==='--help'){console.log('tokentree <doctor|validate [adapter] [--all] [--self-test] [--require-live]|import claude|report --text|query|attach|detach|note|reconcile|migrate prototype --preview|--apply|start|stop|classify>');return 0;}
 const db=openDefaultLedger(paths);
 try{
  if(command==='validate'){const nativeBin=findNativeBinary();if(nativeBin){const result=spawnSync(nativeBin,process.argv.slice(2),{stdio:'inherit'});return result.status??(result.signal?1:0);}console.error('tokentree validate requires the native engine.');return 1;}
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
