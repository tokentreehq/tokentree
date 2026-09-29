// SPDX-License-Identifier: Apache-2.0
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { discoverClaudeSessions, parseClaudeSession } from '../src/index.js';

describe('Claude JSONL fallback', () => {
  it('streams known usage and tolerates unknown/truncated records', async () => {
    const dir=mkdtempSync(join(tmpdir(),'tt-')); const path=join(dir,'session.jsonl');
    writeFileSync(path, [
      JSON.stringify({type:'assistant',session_id:'s',request_id:'r',message:{model:'m',usage:{input_tokens:12,output_tokens:3}},future:true}),
      JSON.stringify({type:'future_record',payload:1}),
      '{"truncated":'
    ].join('\n'));
    const result=await parseClaudeSession({providerSessionId:'s',sourcePath:path});
    expect(result.observations).toHaveLength(1); expect(result.stats).toEqual({parsed:1,unknown:1,malformed:1});
    expect(result.observations[0]?.inputTokens).toBe(12);
  });
  it('turns negative cumulative deltas into anomalies', async () => {
    const dir=mkdtempSync(join(tmpdir(),'tt-')); const path=join(dir,'s.jsonl');
    writeFileSync(path,[{type:'usage_snapshot',usage:{input_tokens:10,output_tokens:2}},{type:'usage_snapshot',usage:{input_tokens:8,output_tokens:3}}].map(JSON.stringify).join('\n'));
    const result=await parseClaudeSession({providerSessionId:'s',sourcePath:path});
    expect(result.observations).toHaveLength(0); expect(result.anomalies[0]?.type).toBe('negative_delta');
  });
  it('discovers nested JSONL files only', () => {
    const root=mkdtempSync(join(tmpdir(),'tt-')); mkdirSync(join(root,'p')); writeFileSync(join(root,'p','a.jsonl'),''); writeFileSync(join(root,'p','x.txt'),'');
    expect(discoverClaudeSessions(root)).toHaveLength(1);
  });
});

describe('public sanitized Claude Code compatibility fixture',()=>{
  it('parses camelCase request/session IDs and nested usage from v2.1.x records',async()=>{
    const path=new URL('../../../../fixtures/parsers/claude/public-small-v2.1.80.jsonl',import.meta.url).pathname;
    const result=await parseClaudeSession({providerSessionId:'fallback',sourcePath:path});
    const row=result.observations.find((item)=>item.requestId==='req_stage0_nested');
    expect(row).toMatchObject({providerSessionId:'ses_stage0_small',inputTokens:10,outputTokens:5,model:'claude-opus-5'});
    expect(result.stats.unknown).toBeGreaterThan(0);
  });
});
