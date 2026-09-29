// SPDX-License-Identifier: Apache-2.0
import type { TokenUsage } from '@tokentreehq/core';

export interface ExactRates {
  readonly inputPerMillion: string;
  readonly cachedInputPerMillion?: string;
  readonly cacheWritePerMillion?: string;
  readonly outputPerMillion: string;
  readonly reasoningPerMillion?: string;
}
export interface CostResult { readonly amountMicros: bigint | null; readonly costType: 'api_equivalent_estimate' | 'unavailable'; readonly missingCategories: readonly string[]; }

function decimalDollarsToMicros(value:string):bigint {
  if (!/^\d+(?:\.\d{1,6})?$/.test(value)) throw new Error(`Invalid USD rate: ${value}`);
  const [whole='0',fraction='']=value.split('.');
  return BigInt(whole)*1_000_000n+BigInt(fraction.padEnd(6,'0'));
}
function halfUp(numerator:bigint,denominator:bigint):bigint { return (numerator+denominator/2n)/denominator; }
function category(tokens:number|null,rate:string|undefined,name:string,missing:string[]):bigint {
  if (tokens===null || tokens===0) return 0n;
  if (!rate) { missing.push(name); return 0n; }
  return halfUp(BigInt(tokens)*decimalDollarsToMicros(rate),1_000_000n);
}
export function calculateApiEquivalentCost(usage:TokenUsage,rates:ExactRates|null):CostResult {
  if (!rates) return {amountMicros:null,costType:'unavailable',missingCategories:['pricing']};
  const missing:string[]=[];
  const amount=category(usage.inputTokens,rates.inputPerMillion,'input',missing)
    +category(usage.cachedInputTokens,rates.cachedInputPerMillion,'cached_input',missing)
    +category(usage.cacheWriteTokens,rates.cacheWritePerMillion,'cache_write',missing)
    +category(usage.outputTokens,rates.outputPerMillion,'output',missing)
    +category(usage.reasoningTokens,rates.reasoningPerMillion,'reasoning',missing);
  return missing.length ? {amountMicros:null,costType:'unavailable',missingCategories:missing} : {amountMicros:amount,costType:'api_equivalent_estimate',missingCategories:[]};
}
