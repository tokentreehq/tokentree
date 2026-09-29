// SPDX-License-Identifier: Apache-2.0
import{existsSync,readFileSync}from'node:fs';import{homedir}from'node:os';import{basename,dirname,join,resolve}from'node:path';
export type ProjectDetectionMethod='override'|'verified_mapping'|'config'|'git'|'manifest'|'cwd'|'personal_inbox';
export interface ProjectCandidate{readonly key:string;readonly displayName:string;readonly root:string;readonly method:ProjectDetectionMethod;readonly confidence:number;}
export interface ResolveProjectInput{readonly cwd:string;readonly override?:{key:string;title?:string}|undefined;readonly verified?:{key:string;title:string}|undefined;readonly home?:string|undefined;}
function title(value:string):string{return value.replace(/^@[^/]+\//,'').replace(/[-_]+/g,' ').replace(/\b\w/g,(c)=>c.toUpperCase());}
function key(value:string):string{return value.toLowerCase().replace(/[^a-z0-9]+/g,'-').replace(/^-|-$/g,'')||'personal-unassigned';}
function findUp(cwd:string,name:string,homePath?:string):string|null{let dir=resolve(cwd);const home=homePath?resolve(homePath):homedir();for(;;){if(name==='.git'&&dir===home)return null;if(existsSync(join(dir,name)))return dir;if(dir===home)return null;const parent=dirname(dir);if(parent===dir)return null;dir=parent;}}
function configAt(root:string):{key:string;title?:string}|null{const path=join(root,'.tokentree.yml');if(!existsSync(path))return null;const text=readFileSync(path,'utf8');if(/(?:command|exec|hook|egress|network|pricing)\s*:/i.test(text))throw new Error('Untrusted .tokentree.yml contains forbidden capability keys');const project=/^project:\s*([\w.-]+)\s*$/m.exec(text)?.[1];const display=/^title:\s*([^#\n]+)\s*$/m.exec(text)?.[1]?.trim();return project?(display?{key:project,title:display}:{key:project}):null;}
export function resolveProject(input:ResolveProjectInput):ProjectCandidate{
 const cwd=resolve(input.cwd);if(input.override)return{key:key(input.override.key),displayName:input.override.title??title(input.override.key),root:cwd,method:'override',confidence:1};
 if(input.verified)return{key:key(input.verified.key),displayName:input.verified.title,root:cwd,method:'verified_mapping',confidence:1};
 let dir=cwd;for(;;){const config=configAt(dir);if(config)return{key:key(config.key),displayName:config.title??title(config.key),root:dir,method:'config',confidence:.98};const parent=dirname(dir);if(parent===dir)break;dir=parent;}
 const git=findUp(cwd,'.git',input.home);if(git)return{key:key(basename(git)),displayName:title(basename(git)),root:git,method:'git',confidence:.92};
 const manifest=findUp(cwd,'package.json',input.home);if(manifest){let name=basename(manifest);try{const value=JSON.parse(readFileSync(join(manifest,'package.json'),'utf8')) as {name?:unknown};if(typeof value.name==='string')name=value.name;}catch{/* Fall back to directory identity. */}return{key:key(name),displayName:title(name),root:manifest,method:'manifest',confidence:.82};}
 const skip=input.home&&resolve(input.home)===cwd;if(skip)return{key:'personal-unassigned',displayName:'Personal Unassigned',root:cwd,method:'personal_inbox',confidence:.4};
 return{key:key(basename(cwd)),displayName:title(basename(cwd)),root:cwd,method:'cwd',confidence:.6};
}
