// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { evaluate, type LabelRow, type PredictionRow } from '../src/index.js';

const labels: LabelRow[] = [
  { id: 'c1', expected: 'CHILD', confidence: 1, moved_later: false },
  { id: 'c2', expected: 'CHILD', confidence: 1, moved_later: true },
  { id: 's1', expected: 'SWITCH', confidence: 1, moved_later: false },
  { id: 'n1', expected: 'CONTINUE', confidence: 1, moved_later: false }
];
const predictions: PredictionRow[] = [
  { id: 'c1', predicted: 'CHILD', confidence: .9 },
  { id: 'c2', predicted: 'CONTINUE', confidence: .9 },
  { id: 's1', predicted: 'SWITCH', confidence: .9 },
  { id: 'n1', predicted: 'SWITCH', confidence: .7 }
];

describe('boundary evaluation', () => {
  it('computes CHILD/SWITCH precision and recall', () => {
    const result = evaluate(labels, predictions);
    expect(result.CHILD.precision).toBe(1);
    expect(result.CHILD.recall).toBe(.5);
    expect(result.SWITCH.precision).toBe(.5);
    expect(result.SWITCH.recall).toBe(1);
    expect(result.highConfidenceMovedPercent).toBeCloseTo(100 / 3);
  });
  it('uses null rather than inventing a perfect zero-denominator metric', () => {
    const result = evaluate([{ id: 'n', expected: 'CONTINUE', confidence: .5, moved_later: false }], [{ id: 'n', predicted: 'CONTINUE', confidence: .5 }]);
    expect(result.CHILD.precision).toBeNull();
    expect(result.CHILD.recall).toBeNull();
    expect(result.highConfidenceMovedPercent).toBeNull();
  });
  it('reports missing predictions', () => {
    expect(evaluate(labels, []).missingPredictions).toEqual(['c1','c2','s1','n1']);
  });
});
