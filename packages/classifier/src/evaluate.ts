// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { classifyBoundary } from './classify.js';

export type Outcome = 'CONTINUE' | 'CHILD' | 'SWITCH' | 'UNCERTAIN';
export const OUTCOMES: readonly Outcome[] = ['CHILD', 'CONTINUE', 'SWITCH', 'UNCERTAIN'] as const;

export interface LabelRow {
  readonly id: string;
  readonly sanitized_input?: string;
  readonly expected: Outcome;
  readonly confidence: number;
  readonly parent_evidence?: boolean;
  readonly issue_id_changed?: boolean;
  readonly explicit_parent_request?: boolean;
  readonly moved_later: boolean;
}

export interface PredictionRow {
  readonly id: string;
  readonly predicted: Outcome;
  readonly confidence: number;
}

export interface BinaryMetrics {
  readonly precision: number | null;
  readonly recall: number | null;
  readonly errorRate: number;
  readonly truePositive: number;
  readonly falsePositive: number;
  readonly falseNegative: number;
}

export interface ConfusionMatrix {
  readonly classes: readonly Outcome[];
  readonly matrix: Record<Outcome, Record<Outcome, number>>;
}

export interface Evaluation {
  readonly CHILD: BinaryMetrics;
  readonly CONTINUE: BinaryMetrics;
  readonly SWITCH: BinaryMetrics;
  readonly UNCERTAIN: BinaryMetrics;
  readonly confusionMatrix: ConfusionMatrix;
  readonly overallAccuracy: number;
  readonly overallErrorRate: number;
  readonly highConfidenceMovedPercent: number | null;
  readonly missingPredictions: readonly string[];
}

export function parseJsonLines<T>(text: string): T[] {
  return text
    .split(/\r?\n/)
    .filter((line) => line.trim() !== '')
    .map((line) => JSON.parse(line) as T);
}

export function buildConfusionMatrix(
  labels: readonly LabelRow[],
  predictions: ReadonlyMap<string, PredictionRow>
): ConfusionMatrix {
  const matrix: Record<Outcome, Record<Outcome, number>> = {
    CHILD: { CHILD: 0, CONTINUE: 0, SWITCH: 0, UNCERTAIN: 0 },
    CONTINUE: { CHILD: 0, CONTINUE: 0, SWITCH: 0, UNCERTAIN: 0 },
    SWITCH: { CHILD: 0, CONTINUE: 0, SWITCH: 0, UNCERTAIN: 0 },
    UNCERTAIN: { CHILD: 0, CONTINUE: 0, SWITCH: 0, UNCERTAIN: 0 },
  };

  for (const label of labels) {
    const pred = predictions.get(label.id)?.predicted;
    if (pred && OUTCOMES.includes(pred) && OUTCOMES.includes(label.expected)) {
      matrix[label.expected][pred]++;
    }
  }

  return { classes: OUTCOMES, matrix };
}

function binaryMetrics(
  labels: readonly LabelRow[],
  predictions: ReadonlyMap<string, PredictionRow>,
  outcome: Outcome
): BinaryMetrics {
  let truePositive = 0;
  let falsePositive = 0;
  let falseNegative = 0;

  for (const label of labels) {
    const predicted = predictions.get(label.id)?.predicted;
    if (predicted === outcome && label.expected === outcome) truePositive++;
    else if (predicted === outcome) falsePositive++;
    else if (label.expected === outcome) falseNegative++;
  }

  const total = labels.length;
  const precision =
    truePositive + falsePositive === 0
      ? null
      : truePositive / (truePositive + falsePositive);
  const recall =
    truePositive + falseNegative === 0
      ? null
      : truePositive / (truePositive + falseNegative);
  const errorRate = total === 0 ? 0 : (falsePositive + falseNegative) / total;

  return {
    precision,
    recall,
    errorRate,
    truePositive,
    falsePositive,
    falseNegative,
  };
}

export function evaluate(
  labels: readonly LabelRow[],
  predictionRows: readonly PredictionRow[],
  highConfidence = 0.8
): Evaluation {
  const predictions = new Map(predictionRows.map((row) => [row.id, row]));
  const missingPredictions = labels
    .filter((row) => !predictions.has(row.id))
    .map((row) => row.id);

  let correctCount = 0;
  for (const label of labels) {
    if (predictions.get(label.id)?.predicted === label.expected) {
      correctCount++;
    }
  }

  const total = labels.length;
  const overallAccuracy = total === 0 ? 0 : correctCount / total;
  const overallErrorRate = total === 0 ? 0 : (total - correctCount) / total;

  const high = labels.filter(
    (row) => (predictions.get(row.id)?.confidence ?? -1) >= highConfidence
  );
  const moved = high.filter((row) => row.moved_later).length;

  return {
    CHILD: binaryMetrics(labels, predictions, 'CHILD'),
    CONTINUE: binaryMetrics(labels, predictions, 'CONTINUE'),
    SWITCH: binaryMetrics(labels, predictions, 'SWITCH'),
    UNCERTAIN: binaryMetrics(labels, predictions, 'UNCERTAIN'),
    confusionMatrix: buildConfusionMatrix(labels, predictions),
    overallAccuracy,
    overallErrorRate,
    highConfidenceMovedPercent:
      high.length === 0 ? null : (100 * moved) / high.length,
    missingPredictions,
  };
}

export function predictDynamically(labels: readonly LabelRow[]): PredictionRow[] {
  return labels.map((row) => {
    const res = classifyBoundary({
      text: row.sanitized_input ?? '',
      hasOpenParent: row.parent_evidence ?? false,
      issueIdChanged: row.issue_id_changed ?? false,
      explicitParentRequest: row.explicit_parent_request ?? false,
    });
    return {
      id: row.id,
      predicted: res.outcome,
      confidence: res.score,
    };
  });
}

function percent(value: number | null): string {
  return value === null ? 'N/A' : `${(value * 100).toFixed(1)}%`;
}

export function formatEvaluation(result: Evaluation): string {
  const lines: string[] = [];

  lines.push('=== Classifier Evaluation Report ===');
  lines.push(`Overall Accuracy:   ${percent(result.overallAccuracy)}`);
  lines.push(`Overall Error Rate: ${percent(result.overallErrorRate)}`);
  lines.push('');

  lines.push('Per-Class Metrics:');
  for (const outcome of OUTCOMES) {
    const m = result[outcome];
    lines.push(
      `  ${outcome.padEnd(10)} Precision: ${percent(m.precision).padEnd(8)} Recall: ${percent(m.recall).padEnd(8)} Error Rate: ${percent(m.errorRate)} (TP: ${m.truePositive}, FP: ${m.falsePositive}, FN: ${m.falseNegative})`
    );
  }
  lines.push('');

  lines.push('Confusion Matrix (Rows = Actual, Columns = Predicted):');
  const header = `  ${''.padEnd(12)}` + OUTCOMES.map((o) => o.padStart(11)).join('');
  lines.push(header);
  for (const actual of OUTCOMES) {
    const row = `  ${actual.padEnd(12)}` + OUTCOMES.map((pred) => String(result.confusionMatrix.matrix[actual][pred]).padStart(11)).join('');
    lines.push(row);
  }
  lines.push('');

  lines.push(
    `High-confidence rows later moved: ${
      result.highConfidenceMovedPercent === null
        ? 'N/A'
        : `${result.highConfidenceMovedPercent.toFixed(1)}%`
    }`
  );
  lines.push(`Missing predictions: ${result.missingPredictions.length}`);

  return lines.join('\n');
}

const [labelsPath, predictionsPath] = process.argv.slice(2);
if (labelsPath && import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const labels = parseJsonLines<LabelRow>(readFileSync(labelsPath, 'utf8'));
  const predictions = predictionsPath
    ? parseJsonLines<PredictionRow>(readFileSync(predictionsPath, 'utf8'))
    : predictDynamically(labels);

  const result = evaluate(labels, predictions);
  console.log(formatEvaluation(result));
  if (result.missingPredictions.length > 0 || result.overallErrorRate > 0.15) {
    process.exitCode = 1;
  }
}
