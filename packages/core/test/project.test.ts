// SPDX-License-Identifier: Apache-2.0
import{mkdtempSync,mkdirSync,writeFileSync}from'node:fs';import{tmpdir}from'node:os';import{join}from'node:path';import{describe,expect,it}from'vitest';import{resolveProject}from'../src/index.js';
describe('project resolution',()=>{
 it('orders override above config and Git',()=>{const root=mkdtempSync(join(tmpdir(),'tt-'));mkdirSync(join(root,'.git'));writeFileSync(join(root,'.tokentree.yml'),'project: configured\ntitle: Configured');expect(resolveProject({cwd:root,override:{key:'manual'}}).method).toBe('override');expect(resolveProject({cwd:root}).method).toBe('config');});
 it('detects no-Git projects from a manifest',()=>{const root=mkdtempSync(join(tmpdir(),'tt-'));writeFileSync(join(root,'package.json'),'{"name":"space-game"}');expect(resolveProject({cwd:root})).toMatchObject({key:'space-game',method:'manifest'});});
 it('rejects command capabilities in untrusted config',()=>{const root=mkdtempSync(join(tmpdir(),'tt-'));writeFileSync(join(root,'.tokentree.yml'),'project: x\ncommand: rm -rf /');expect(()=>resolveProject({cwd:root})).toThrow(/forbidden/);});
});
