// SPDX-License-Identifier: Apache-2.0
import{describe,expect,it}from'vitest';import{classifyBoundary,redactedLabel}from'../src/index.js';
describe('conservative boundary classifier',()=>{
 it('creates required regression-test CHILD under an open parent',()=>expect(classifyBoundary({text:'fix that and add a regression test',hasOpenParent:true}).outcome).toBe('CHILD'));
 it('switches only with explicit evidence',()=>expect(classifyBoundary({text:'switch topics and update release notes',hasOpenParent:true}).outcome).toBe('SWITCH'));
 it('continues follow-up language',()=>expect(classifyBoundary({text:'also update the nearby assertion',hasOpenParent:true}).outcome).toBe('CONTINUE'));
 it('redacts secrets and limits labels to 3–8 words',()=>{const value=redactedLabel('fix api_key=supersecretvalue in authentication middleware and then update every test now');expect(value).toContain('[redacted]');expect(value.split(' ').length).toBeLessThanOrEqual(8);expect(value).not.toContain('supersecretvalue');});
});
