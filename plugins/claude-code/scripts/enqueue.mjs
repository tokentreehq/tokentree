#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
import{createHash}from'node:crypto';import{appendFileSync,chmodSync,mkdirSync,readFileSync}from'node:fs';import{homedir}from'node:os';import{dirname,join}from'node:path';import{pathToFileURL}from'node:url';
const text=(value)=>typeof value==='string'&&value?value:undefined;
export function sanitizeHook(input,capturedAt=new Date().toISOString()){
 const event=text(input.hook_event_name)??'Unknown',tool=typeof input.tool_input==='object'&&input.tool_input?input.tool_input:{},safe={version:1,kind:event,capturedAt,payload:{}};
 const fields=['session_id','prompt_id','transcript_path','cwd','permission_mode','agent_id','parent_agent_id','task_id','tool_name','tool_use_id','source'];for(const field of fields){const value=text(input[field]);if(value)safe.payload[field]=value;}
 const prompt=text(input.prompt);if(prompt)safe.payload.prompt_fingerprint=createHash('sha256').update(prompt).digest('hex');
 if(event==='PostToolUse'&&['Write','Edit','MultiEdit'].includes(text(input.tool_name)??'')){const path=text(tool.file_path??tool.path);if(path)safe.payload.file_path=path;}
 return safe;
}
export function enqueue(input,home=process.env.TOKENTREE_HOME||join(homedir(),'.tokentree')){const path=join(home,'spool','claude-hooks.jsonl');mkdirSync(dirname(path),{recursive:true,mode:0o700});appendFileSync(path,`${JSON.stringify(sanitizeHook(input))}\n`,{encoding:'utf8',mode:0o600});try{chmodSync(path,0o600);}catch{/* POSIX modes may be unavailable. */}return path;}
async function main(){let raw;try{raw=readFileSync(0,'utf8');if(Buffer.byteLength(raw)>1_048_576)throw new Error('hook input exceeds 1 MiB');enqueue(JSON.parse(raw));}catch(error){process.stderr.write(`TokenTree enqueue failed: ${error instanceof Error?error.message:String(error)}\n`);process.exitCode=1;}}
if(import.meta.url===pathToFileURL(process.argv[1]??'').href)await main();
