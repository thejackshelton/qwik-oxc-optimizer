/**
 * Reporter module — terminal formatting and step-trace explanations.
 *
 * Covers: REPT-01, REPT-02, REPT-04, REPT-05
 *
 * All formatting is plain text (no ANSI colors) — output is consumed by AI (Codex).
 */

import { FailureCategory } from "./contract.js";
import type { HarnessOutput, FailureCategoryValue } from "./contract.js";
import {
  decomposeDisplayName,
  recomputeHash,
  recomputeCanonicalFilename,
} from "./identity.js";
import type { SegmentMetadata } from "./types.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface FailureItem {
  category: FailureCategoryValue;
  field?: string;
  expected?: unknown;
  actual?: unknown;
}

export interface StepTraceEntry {
  fixtureName: string;
  failures: Array<{
    failure: FailureItem;
    swcMeta: SegmentMetadata | null;
    oxcMeta: SegmentMetadata | null;
    scope?: string | null;
    relPath?: string;
  }>;
}

// ---------------------------------------------------------------------------
// formatTerminalReport
// ---------------------------------------------------------------------------

/**
 * Build a plain-text terminal report from harness output.
 *
 * Format:
 *   Parsed {total} fixtures. {failed} failure(s).
 *     [PASS] {name}
 *     [FAIL] {name} ({N} failures)
 *   [blank line]
 *   Failures by category:
 *     {category}: {count}
 *   ...
 *
 * The optional second argument provides step-trace metadata for FAIL fixtures.
 * When provided, derived-field failures include algorithm step-trace lines.
 *
 * REPT-04: No ANSI escape sequences — plain text only.
 */
export function formatTerminalReport(
  output: HarnessOutput,
  stepTraces?: StepTraceEntry[]
): string {
  const lines: string[] = [];

  // Summary line
  const { total, failed } = output.summary;
  lines.push(`Parsed ${total} fixtures. ${failed} failure(s).`);

  // Per-fixture lines
  for (const fixture of output.fixtures) {
    if (fixture.pass) {
      lines.push(`  [PASS] ${fixture.name}`);
    } else {
      lines.push(`  [FAIL] ${fixture.name} (${fixture.failures.length} failures)`);

      // Optional step-trace for FAIL fixtures
      if (stepTraces) {
        const traceEntry = stepTraces.find((e) => e.fixtureName === fixture.name);
        if (traceEntry) {
          for (const { failure, swcMeta, oxcMeta, scope, relPath } of traceEntry.failures) {
            const trace = explainDerivedFailure(failure, swcMeta, oxcMeta, scope, relPath);
            if (trace) {
              // Indent trace lines
              for (const traceLine of trace.split("\n")) {
                lines.push(`    ${traceLine}`);
              }
            }
          }
        }
      }
    }
  }

  // Scorecard: non-zero categories only (REPT-02)
  const nonZeroCategories = (
    Object.values(FailureCategory) as FailureCategoryValue[]
  ).filter((cat) => output.summary.byCategory[cat] > 0);

  if (nonZeroCategories.length > 0) {
    lines.push("");
    lines.push("Failures by category:");
    for (const cat of nonZeroCategories) {
      lines.push(`  ${cat}: ${output.summary.byCategory[cat]}`);
    }
  }

  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// formatStableJson
// ---------------------------------------------------------------------------

/**
 * Produce machine-stable JSON output.
 *
 * Guarantees:
 * - Fixtures sorted by name ascending (clones input, never mutates)
 * - byCategory keys in FailureCategory enum order (stable because built via
 *   Object.values(FailureCategory) iteration in cli.ts)
 * - Two calls with identical input produce byte-identical output
 */
export function formatStableJson(output: HarnessOutput): string {
  // Clone and sort fixtures by name
  const sortedOutput: HarnessOutput = {
    ...output,
    fixtures: [...output.fixtures].sort((a, b) => a.name.localeCompare(b.name)),
  };

  return JSON.stringify(sortedOutput, null, 2);
}

// ---------------------------------------------------------------------------
// explainDerivedFailure
// ---------------------------------------------------------------------------

/**
 * Generate a step-trace explanation for derived-field mismatches (REPT-05).
 *
 * Returns a multi-line string showing the algorithm inputs and divergence
 * point for hash_mismatch, display_name_mismatch, and canonical_filename_mismatch.
 *
 * Returns null for:
 * - Non-derived failure categories
 * - null swcMeta or oxcMeta (cannot trace without both sides)
 */
export function explainDerivedFailure(
  failure: { category: FailureCategoryValue; field?: string; expected?: unknown; actual?: unknown },
  swcMeta: SegmentMetadata | null,
  oxcMeta: SegmentMetadata | null,
  fixtureScope?: string | null,
  relPath?: string
): string | null {
  const derived = new Set<FailureCategoryValue>([
    FailureCategory.HASH_MISMATCH,
    FailureCategory.DISPLAY_NAME_MISMATCH,
    FailureCategory.CANONICAL_FILENAME_MISMATCH,
  ]);

  if (!derived.has(failure.category)) {
    return null;
  }

  if (swcMeta === null || oxcMeta === null) {
    return null;
  }

  if (failure.category === FailureCategory.HASH_MISMATCH) {
    return explainHashMismatch(failure, swcMeta, oxcMeta, fixtureScope, relPath);
  }

  if (failure.category === FailureCategory.DISPLAY_NAME_MISMATCH) {
    return explainDisplayNameMismatch(failure, swcMeta, oxcMeta);
  }

  if (failure.category === FailureCategory.CANONICAL_FILENAME_MISMATCH) {
    return explainCanonicalFilenameMismatch(failure, swcMeta, oxcMeta);
  }

  return null;
}

// ---------------------------------------------------------------------------
// Step-trace helpers (private)
// ---------------------------------------------------------------------------

function explainHashMismatch(
  failure: { expected?: unknown; actual?: unknown },
  swcMeta: SegmentMetadata,
  _oxcMeta: SegmentMetadata,
  fixtureScope: string | null | undefined,
  relPath: string | undefined
): string {
  const decomposed = decomposeDisplayName(swcMeta.displayName, swcMeta.origin);
  const prePrefix = decomposed?.prePrefix ?? swcMeta.displayName;
  const effectiveRelPath = relPath ?? swcMeta.origin;
  const recomputed = recomputeHash(fixtureScope ?? null, effectiveRelPath, prePrefix);

  const lines: string[] = [
    `[step-trace: hash_mismatch]`,
    `  scope: ${fixtureScope ?? "(none)"}`,
    `  relPath: ${effectiveRelPath}`,
    `  prePrefix: ${prePrefix}`,
    `  recomputed: ${recomputed}`,
    `  expected (SWC): ${String(failure.expected ?? swcMeta.hash)}`,
    `  actual (OXC):   ${String(failure.actual ?? _oxcMeta.hash)}`,
  ];

  return lines.join("\n");
}

function explainDisplayNameMismatch(
  failure: { expected?: unknown; actual?: unknown },
  swcMeta: SegmentMetadata,
  oxcMeta: SegmentMetadata
): string {
  const swcDecomposed = decomposeDisplayName(swcMeta.displayName, swcMeta.origin);
  const oxcDecomposed = decomposeDisplayName(oxcMeta.displayName, oxcMeta.origin);

  const swcFilePrefix = swcDecomposed?.fileNamePrefix ?? "(unknown)";
  const swcPrePrefix = swcDecomposed?.prePrefix ?? swcMeta.displayName;
  const oxcFilePrefix = oxcDecomposed?.fileNamePrefix ?? "(unknown)";
  const oxcPrePrefix = oxcDecomposed?.prePrefix ?? oxcMeta.displayName;

  const lines: string[] = [
    `[step-trace: display_name_mismatch]`,
    `  expected (SWC): ${String(failure.expected ?? swcMeta.displayName)}`,
    `  actual (OXC):   ${String(failure.actual ?? oxcMeta.displayName)}`,
    `  SWC file prefix: ${swcFilePrefix}_`,
    `  SWC prePrefix:   ${swcPrePrefix}`,
    `  OXC file prefix: ${oxcFilePrefix}_`,
    `  OXC prePrefix:   ${oxcPrePrefix}`,
  ];

  return lines.join("\n");
}

function explainCanonicalFilenameMismatch(
  failure: { expected?: unknown; actual?: unknown },
  swcMeta: SegmentMetadata,
  oxcMeta: SegmentMetadata
): string {
  const tokens = swcMeta.name.split("_");
  const hashSuffix = tokens[tokens.length - 1];
  const recomputed = recomputeCanonicalFilename(swcMeta.displayName, swcMeta.name);

  const lines: string[] = [
    `[step-trace: canonical_filename_mismatch]`,
    `  displayName (SWC): ${swcMeta.displayName}`,
    `  symbolName (SWC):  ${swcMeta.name}`,
    `  hashSuffix (last _ token): ${hashSuffix}`,
    `  recomputed: ${recomputed}`,
    `  expected (SWC stored): ${String(failure.expected ?? swcMeta.canonicalFilename)}`,
    `  actual (OXC):          ${String(failure.actual ?? oxcMeta.canonicalFilename)}`,
  ];

  return lines.join("\n");
}
