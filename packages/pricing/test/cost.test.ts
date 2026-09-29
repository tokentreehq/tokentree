// SPDX-License-Identifier: Apache-2.0
import { describe,expect,it } from 'vitest';
import { calculateApiEquivalentCost } from '../src/index.js';
const usage={inputTokens:1_000_000,cachedInputTokens:1_000_000,cacheWriteTokens:1_000_000,outputTokens:1_000_000,reasoningTokens:null};
describe('integer-micro costs',()=>{
 it('prices cache reads at their own rate',()=>{const value=calculateApiEquivalentCost(usage,{inputPerMillion:'3.00',cachedInputPerMillion:'0.30',cacheWritePerMillion:'3.75',outputPerMillion:'15.00'});expect(value.amountMicros).toBe(22_050_000n);});
 it('returns unavailable rather than zero when a used category lacks a rate',()=>{const value=calculateApiEquivalentCost(usage,{inputPerMillion:'3.00',outputPerMillion:'15.00'});expect(value.amountMicros).toBeNull();expect(value.missingCategories).toEqual(['cached_input','cache_write']);});
 it('uses deterministic half-up micro rounding',()=>{const value=calculateApiEquivalentCost({...usage,inputTokens:1,cachedInputTokens:null,cacheWriteTokens:null,outputTokens:0},{inputPerMillion:'0.50',outputPerMillion:'1'});expect(value.amountMicros).toBe(1n);});
});
