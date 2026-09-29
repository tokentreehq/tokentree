// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export type Outcome = 'CONTINUE' | 'CHILD' | 'SWITCH' | 'UNCERTAIN';
export interface LabelRow { readonly id: string; readonly expected: Outcome; readonly confidence: number; readonly moved_later: boolean; }
export interface PredictionRow { readonly id: string; readonly predicted: Outcome; readonly confidence: number; }
export interface BinaryMetrics { readonly precision: number | null; readonly recall: number | null; readonly truePositive: number; readonly falsePositive: number; readonly falseNegative: number; }
export interface Evaluation { readonly CHILD: BinaryMetrics; readonly SWITCH: BinaryMetrics; readonly highConfidenceMovedPercent: number | null; readonly missingPredictions: readonly string[]; }

export function parseJsonLines<T>(text: string): T[] {
  return text.split(/\r?\n/).filter((line) => line.trim() !== '').map((line) => JSON.parse(line) as T);
}

function binaryMetrics(labels: readonly LabelRow[], predictions: ReadonlyMap<string, PredictionRow>, outcome: Outcome): BinaryMetrics {
  let truePositive = 0, falsePositive = 0, falseNegative = 0;
  for (const label of labels) {
    const predicted = predictions.get(label.id)?.predicted;
    if (predicted === outcome && label.expected === outcome) truePositive++;
    else if (predicted === outcome) falsePositive++;
    else if (label.expected === outcome) falseNegative++;
  }
  return {
    precision: truePositive + falsePositive === 0 ? null : truePositive / (truePositive + falsePositive),
    recall: truePositive + falseNegative === 0 ? null : truePositive / (truePositive + falseNegative),
    truePositive, falsePositive, falseNegative
  };
}

export function evaluate(labels: readonly LabelRow[], predictionRows: readonly PredictionRow[], highConfidence = 0.8): Evaluation {
  const predictions = new Map(predictionRows.map((row) => [row.id, row]));
  const missingPredictions = labels.filter((row) => !predictions.has(row.id)).map((row) => row.id);
  const high = labels.filter((row) => (predictions.get(row.id)?.confidence ?? -1) >= highConfidence);
  const moved = high.filter((row) => row.moved_later).length;
  return {
    CHILD: binaryMetrics(labels, predictions, 'CHILD'),
    SWITCH: binaryMetrics(labels, predictions, 'SWITCH'),
    highConfidenceMovedPercent: high.length === 0 ? null : 100 * moved / high.length,
    missingPredictions
  };
}

function percent(value: number | null): string { return value === null ? 'N/A' : `${(value * 100).toFixed(1)}%`; }

export function formatEvaluation(result: Evaluation): string {
  return [
    `CHILD precision ${percent(result.CHILD.precision)} recall ${percent(result.CHILD.recall)}`,
    `SWITCH precision ${percent(result.SWITCH.precision)} recall ${percent(result.SWITCH.recall)}`,
    `High-confidence rows later moved ${result.highConfidenceMovedPercent === null ? 'N/A' : `${result.highConfidenceMovedPercent.toFixed(1)}%`}`,
    `Missing predictions ${result.missingPredictions.length}`
  ].join('\n');
}

const [labelsPath, predictionsPath] = process.argv.slice(2);
if (labelsPath && predictionsPath && import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const result = evaluate(parseJsonLines<LabelRow>(readFileSync(labelsPath, 'utf8')), parseJsonLines<PredictionRow>(readFileSync(predictionsPath, 'utf8')));
  console.log(formatEvaluation(result));
  if (result.missingPredictions.length > 0) process.exitCode = 1;
}
