// SPDX-License-Identifier: Apache-2.0
export type BoundaryOutcome='CONTINUE'|'CHILD'|'SWITCH'|'UNCERTAIN';
export interface BoundaryInput{readonly text:string;readonly hasOpenParent:boolean;readonly issueIdChanged?:boolean;readonly explicitParentRequest?:boolean;}
export interface BoundaryResult{readonly outcome:BoundaryOutcome;readonly score:number;readonly signals:readonly string[];readonly derivedLabel:string;}
const SECRET=/\b(?:sk-[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9]{8,}|(?:api[_-]?key|token|password)\s*[:=]\s*\S+)\b/gi;
const STOP=new Set(['a','an','and','the','to','for','of','in','on','that','this','please','now','also']);
export function redactedLabel(text:string):string{
 const clean=text.replace(SECRET,'[redacted]').replace(/https?:\/\/\S+/g,'').replace(/[^\p{L}\p{N}[\]-]+/gu,' ').trim();
 const words=clean.split(/\s+/).filter((word)=>word&&(!STOP.has(word.toLowerCase())||word==='[redacted]')).slice(0,8);
 while(words.length<3)words.push('work');
 return words.join(' ');
}
export function classifyBoundary(input:BoundaryInput):BoundaryResult{
 const lower=input.text.toLowerCase(),signals:string[]=[];
 const isNegativeSwitch=/\bswitch\s+(?:statement|case|expression|block|syntax|branch)\b/.test(lower);
 if(input.issueIdChanged||(!isNegativeSwitch&&/\b(?:switch\s+(?:topics?|to|gears|context)|new task|unrelated|separately|start working on|pause this)\b/.test(lower))){
  signals.push(input.issueIdChanged?'issue_id_changed':'explicit_switch');
  return{outcome:'SWITCH',score:.92,signals,derivedLabel:redactedLabel(input.text)};
 }
 if(input.hasOpenParent&&(/\b(?:regression test|add (?:a )?test|document (?:the|this) fix|benchmark|microbenchmark|extract|investigate|profile)\b/.test(lower)||input.explicitParentRequest)){
  signals.push('open_parent','bounded_follow_up');
  return{outcome:'CHILD',score:.9,signals,derivedLabel:redactedLabel(input.text)};
 }
 if(/\b(?:also|continue|same|that|it|nearby|follow.?up|too|above|keep going|finish)\b/.test(lower)||isNegativeSwitch){
  signals.push('continuity_language');
  return{outcome:'CONTINUE',score:.82,signals,derivedLabel:redactedLabel(input.text)};
 }
 signals.push('insufficient_boundary_evidence');
 return{outcome:'UNCERTAIN',score:.45,signals,derivedLabel:redactedLabel(input.text)};
}
