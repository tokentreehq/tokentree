// SPDX-License-Identifier: Apache-2.0
import type{DatabaseSync}from'node:sqlite';
export interface UsageTotals{input:number;cacheRead:number;cacheWrite:number;output:number;reasoning:number;requests:number;measured:number;unavailable:number;priced:number;amountMicros:number;}
export interface WorkTreeNode{readonly id:string;readonly title:string;readonly parentId:string|null;readonly direct:UsageTotals;readonly inclusive:UsageTotals;readonly children:WorkTreeNode[];}
export interface ProjectTree{readonly id:string;readonly key:string;readonly title:string;readonly roots:WorkTreeNode[];readonly totals:UsageTotals;}
const zero=():UsageTotals=>({input:0,cacheRead:0,cacheWrite:0,output:0,reasoning:0,requests:0,measured:0,unavailable:0,priced:0,amountMicros:0});
const add=(a:UsageTotals,b:UsageTotals):UsageTotals=>({input:a.input+b.input,cacheRead:a.cacheRead+b.cacheRead,cacheWrite:a.cacheWrite+b.cacheWrite,output:a.output+b.output,reasoning:a.reasoning+b.reasoning,requests:a.requests+b.requests,measured:a.measured+b.measured,unavailable:a.unavailable+b.unavailable,priced:a.priced+b.priced,amountMicros:a.amountMicros+b.amountMicros});
export function loadProjectTrees(db:DatabaseSync,projectFilter?:string):ProjectTree[]{
 const projects=(projectFilter?db.prepare('SELECT id,key,display_name FROM projects WHERE key=? OR display_name LIKE ? ORDER BY display_name').all(projectFilter,`%${projectFilter}%`):db.prepare('SELECT id,key,display_name FROM projects ORDER BY display_name').all()) as Array<{id:string;key:string;display_name:string}>;
 return projects.map((project)=>loadProjectTree(db,project));
}
function loadProjectTree(db:DatabaseSync,project:{id:string;key:string;display_name:string}):ProjectTree{
 const rows=db.prepare(`SELECT wi.id,wi.parent_id,wi.title,
 coalesce(sum(ue.input_tokens),0) input,coalesce(sum(ue.cached_input_tokens),0) cache_read,coalesce(sum(ue.cache_write_tokens),0) cache_write,coalesce(sum(ue.output_tokens),0) output,coalesce(sum(ue.reasoning_tokens),0) reasoning,
 count(ue.id) requests,coalesce(sum(CASE WHEN us.measurement_status='measured' THEN 1 ELSE 0 END),0) measured,coalesce(sum(CASE WHEN us.measurement_status<>'measured' THEN 1 ELSE 0 END),0) unavailable,
 count(cc.usage_event_id) priced,coalesce(sum(cc.amount_micros),0) amount_micros
 FROM work_items wi
 LEFT JOIN attributions a ON a.work_item_id=wi.id
 LEFT JOIN attribution_groups ag ON ag.id=a.group_id AND ag.active=1
 LEFT JOIN usage_spans us ON us.id=ag.usage_span_id
 LEFT JOIN usage_events ue ON ue.id=json_extract(us.measured_usage_json,'$.usage_event_id')
 LEFT JOIN cost_calculations cc ON cc.rowid=(SELECT c2.rowid FROM cost_calculations c2 WHERE c2.usage_event_id=ue.id ORDER BY c2.calculated_at DESC LIMIT 1)
 WHERE wi.project_id=? GROUP BY wi.id ORDER BY wi.created_at,wi.title`).all(project.id) as Array<Record<string,string|number|null>>;
 const nodes=new Map<string,WorkTreeNode>();for(const row of rows){const direct:UsageTotals={input:Number(row.input),cacheRead:Number(row.cache_read),cacheWrite:Number(row.cache_write),output:Number(row.output),reasoning:Number(row.reasoning),requests:Number(row.requests),measured:Number(row.measured),unavailable:Number(row.unavailable),priced:Number(row.priced),amountMicros:Number(row.amount_micros)};nodes.set(String(row.id),{id:String(row.id),title:String(row.title),parentId:typeof row.parent_id==='string'?row.parent_id:null,direct,inclusive:zero(),children:[]});}
 const roots:WorkTreeNode[]=[];for(const node of nodes.values()){const parent=node.parentId?nodes.get(node.parentId):undefined;if(parent)parent.children.push(node);else roots.push(node);}
 const roll=(node:WorkTreeNode):UsageTotals=>{let total=node.direct;for(const child of node.children)total=add(total,roll(child));Object.assign(node,{inclusive:total});return total;};let totals=zero();for(const root of roots)totals=add(totals,roll(root));return{id:project.id,key:project.key,title:project.display_name,roots,totals};
}
function formatTotals(value:UsageTotals):string{const tokens=value.input+value.cacheRead+value.cacheWrite+value.output+value.reasoning,complete=value.requests===0?null:100*value.measured/(value.measured+value.unavailable),cost=value.requests>0&&value.priced===value.requests?`$${(value.amountMicros/1_000_000).toFixed(2)} est. API-equivalent`:'cost unavailable';return`${cost} · ${tokens.toLocaleString()} tok · ${complete===null?'—':`${complete.toFixed(0)}% complete`} · ${value.unavailable} unavailable`;}
export function renderProjectTrees(trees:readonly ProjectTree[]):string{const lines:string[]=[];for(const tree of trees){lines.push(`${tree.title} — ${formatTotals(tree.totals)}`);const walk=(nodes:readonly WorkTreeNode[],prefix:string):void=>nodes.forEach((node,index)=>{const last=index===nodes.length-1;lines.push(`${prefix}${last?'└──':'├──'} ${node.title}  ${formatTotals(node.inclusive)}`);walk(node.children,`${prefix}${last?'    ':'│   '}`);});walk(tree.roots,'');}return lines.length?lines.join('\n'):'No projects in the ledger.';}
export function queryLedger(db:DatabaseSync,input:{project?:string|undefined;workItem?:string|undefined;includeDescendants:boolean}):ProjectTree|WorkTreeNode{
 const trees=loadProjectTrees(db,input.project);if(trees.length!==1)throw new Error(trees.length?`Project is ambiguous (${trees.map((t)=>t.title).join(', ')})`:'Project not found');const tree=trees[0];if(!tree)throw new Error('Project not found');if(!input.workItem)return tree;const matches:WorkTreeNode[]=[];const visit=(node:WorkTreeNode):void=>{if(node.title.toLowerCase().includes(input.workItem!.toLowerCase()))matches.push(node);node.children.forEach(visit);};tree.roots.forEach(visit);if(matches.length!==1)throw new Error(matches.length?`Work item is ambiguous (${matches.map((m)=>m.title).join(', ')})`:'Work item not found');const match=matches[0];if(!match)throw new Error('Work item not found');return input.includeDescendants?match:{...match,children:[],inclusive:match.direct};
}
