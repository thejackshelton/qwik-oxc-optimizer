/**
 * Core comparison engine.
 *
 * compareFixture wires together matcher, normalizer, and contract to produce
 * a list of typed FailureItem values for a single fixture.
 *
 * Design constraints:
 * - COMP-01: If segment counts differ, push SEGMENT_COUNT_MISMATCH and return immediately.
 * - COMP-02: All 14 metadata fields (excluding `name`) are checked independently — no early return.
 * - COMP-03: Code blocks are normalized via normalizeCode before comparison.
 * - COMP-07: loc is always checked as WRONG_LOC, never suppressed.
 * - COMP-06: Every failure carries a typed FailureCategoryValue from the frozen enum.
 */

import * as path from "node:path";
import { FailureCategory, type FailureCategoryValue } from "./contract.js";
import { matchSegments } from "./matcher.js";
import { normalizeCode } from "./normalizer.js";
import type { ParsedSnapshot, SegmentMetadata } from "./types.js";

/** A single typed failure emitted by compareFixture. */
export interface FailureItem {
  category: FailureCategoryValue;
  /** The metadata field name when applicable */
  field?: string;
  expected?: unknown;
  actual?: unknown;
}

// ---------------------------------------------------------------------------
// Table-driven metadata check definitions
// 14 fields: every SegmentMetadata field except `name`
// ---------------------------------------------------------------------------

interface MetadataCheck {
  field: keyof SegmentMetadata;
  category: FailureCategoryValue;
}

const METADATA_CHECKS: MetadataCheck[] = [
  { field: "hash", category: FailureCategory.HASH_MISMATCH },
  { field: "displayName", category: FailureCategory.DISPLAY_NAME_MISMATCH },
  { field: "canonicalFilename", category: FailureCategory.CANONICAL_FILENAME_MISMATCH },
  { field: "captures", category: FailureCategory.WRONG_CAPTURES },
  { field: "captureNames", category: FailureCategory.WRONG_CAPTURE_NAMES },
  { field: "ctxKind", category: FailureCategory.WRONG_CTX_KIND },
  { field: "ctxName", category: FailureCategory.WRONG_CTX_NAME },
  { field: "parent", category: FailureCategory.WRONG_PARENT },
  { field: "entry", category: FailureCategory.WRONG_ENTRY },
  { field: "loc", category: FailureCategory.WRONG_LOC },
  { field: "paramNames", category: FailureCategory.WRONG_PARAM_NAMES },
  { field: "extension", category: FailureCategory.WRONG_EXTENSION },
  { field: "origin", category: FailureCategory.WRONG_ORIGIN },
  { field: "path", category: FailureCategory.WRONG_PATH },
];

// ---------------------------------------------------------------------------
// Helper: stdinFilepath for normalizeCode
// ---------------------------------------------------------------------------

const RECOGNIZED_EXTENSIONS = new Set([".ts", ".tsx", ".js", ".jsx", ".mjs"]);

/**
 * Derive a stdinFilepath for normalizeCode from a section headerName.
 * Falls back to "snapshot-section.tsx" if headerName lacks a recognized extension.
 */
function getStdinFilepath(headerName: string): string {
  const ext = path.extname(headerName);
  if (ext && RECOGNIZED_EXTENSIONS.has(ext)) {
    return headerName;
  }
  return "snapshot-section.tsx";
}

// ---------------------------------------------------------------------------
// Core comparison function
// ---------------------------------------------------------------------------

/**
 * Compare a single fixture's SWC and OXC snapshots, returning typed failures.
 *
 * @param swcSnapshot - Parsed SWC golden snapshot
 * @param oxcSnapshot - Parsed OXC candidate snapshot
 * @returns Array of typed FailureItem — empty array means the fixture passes
 */
export function compareFixture(
  swcSnapshot: ParsedSnapshot,
  oxcSnapshot: ParsedSnapshot
): FailureItem[] {
  const failures: FailureItem[] = [];

  // Filter to segment sections (metadata !== null) — parent sections excluded
  const swcSegs = swcSnapshot.sections.filter((s) => s.metadata !== null);
  const oxcSegs = oxcSnapshot.sections.filter((s) => s.metadata !== null);

  // COMP-01: Segment count check — short-circuit only for count mismatch
  if (swcSegs.length !== oxcSegs.length) {
    failures.push({
      category: FailureCategory.SEGMENT_COUNT_MISMATCH,
      expected: swcSegs.length,
      actual: oxcSegs.length,
    });
    return failures; // Do NOT proceed to per-segment comparison
  }

  // Match segments by order (passes all sections, matcher filters internally)
  const matchResult = matchSegments(swcSnapshot.sections, oxcSnapshot.sections);

  // Process each matched pair
  for (const match of matchResult.matches) {
    const swcMeta = match.swcSection.metadata!;
    const oxcMeta = match.oxcSection.metadata!;

    // COMP-02 + COMP-07: Check all 14 metadata fields independently
    for (const check of METADATA_CHECKS) {
      const swcVal = swcMeta[check.field];
      const oxcVal = oxcMeta[check.field];

      // Use JSON.stringify for deep equality on arrays (loc, paramNames, captureNames)
      const equal =
        typeof swcVal === "object" || typeof oxcVal === "object"
          ? JSON.stringify(swcVal) === JSON.stringify(oxcVal)
          : swcVal === oxcVal;

      if (!equal) {
        failures.push({
          category: check.category,
          field: check.field,
          expected: swcVal,
          actual: oxcVal,
        });
      }
    }

    // COMP-03: Normalized code comparison
    const swcFilepath = getStdinFilepath(match.swcSection.headerName);
    const oxcFilepath = getStdinFilepath(match.oxcSection.headerName);

    const swcNormalized = normalizeCode(match.swcSection.code, swcFilepath);
    const oxcNormalized = normalizeCode(match.oxcSection.code, oxcFilepath);

    if (swcNormalized !== oxcNormalized) {
      failures.push({
        category: FailureCategory.CODE_DIFF,
        expected: swcNormalized,
        actual: oxcNormalized,
      });
    }
  }

  return failures;
}
