/**
 * Core comparison engine.
 *
 * compareFixture wires together matcher, normalizer, and contract to produce
 * a list of typed FailureItem values for a single fixture.
 *
 * Design constraints:
 * - COMP-01: If segment counts differ, push typed MISSING_SEGMENT/EXTRA_SEGMENT with identity context (ctxName + loc) and return immediately.
 * - COMP-02: All 14 metadata fields (excluding `name`) are checked independently — no early return.
 * - COMP-03: Code blocks are normalized via normalizeCode before comparison.
 * - COMP-07: loc is always checked as WRONG_LOC, never suppressed.
 * - COMP-06: Every failure carries a typed FailureCategoryValue from the frozen enum.
 */

import * as path from "node:path";
import { FailureCategory, DIAGNOSTICS_SORT_KEY, type FailureCategoryValue } from "./contract.js";
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
export function getStdinFilepath(headerName: string): string {
  const ext = path.extname(headerName);
  if (ext && RECOGNIZED_EXTENSIONS.has(ext)) {
    return headerName;
  }
  return "snapshot-section.tsx";
}

/**
 * Attempt normalization, falling back to trimmed raw code if oxfmt fails.
 * This is symmetric: if both sides fail the same way, self-comparison still passes.
 * The fallback uses ".tsx" extension on retry to handle cases where the
 * original extension causes parsing issues (e.g., .js + top-level await).
 */
function tryNormalizeCode(code: string, stdinFilepath: string): string {
  try {
    return normalizeCode(code, stdinFilepath);
  } catch {
    // Retry with .tsx extension (module mode) before giving up
    try {
      const fallbackPath = stdinFilepath.replace(/\.[^.]+$/, ".tsx");
      if (fallbackPath !== stdinFilepath) {
        return normalizeCode(code, fallbackPath);
      }
    } catch {
      // Both attempts failed — return trimmed raw code as last resort
    }
    return code.trim();
  }
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

  // COMP-01: Segment count check — structural mismatch reporting (REPT-01, SEG-02)
  if (swcSegs.length !== oxcSegs.length) {
    // Use matchSegments to identify which specific segments are missing/extra
    const countMatchResult = matchSegments(swcSnapshot.sections, oxcSnapshot.sections);

    // MISSING_SEGMENT: segments in SWC that have no OXC counterpart
    for (const seg of countMatchResult.unmatched_swc) {
      const meta = seg.metadata!;
      failures.push({
        category: FailureCategory.MISSING_SEGMENT,
        field: "segment",
        expected: { ctxName: meta.ctxName, loc: meta.loc },
        actual: undefined,
      });
    }

    // EXTRA_SEGMENT: segments in OXC that have no SWC counterpart
    for (const seg of countMatchResult.unmatched_oxc) {
      const meta = seg.metadata!;
      failures.push({
        category: FailureCategory.EXTRA_SEGMENT,
        field: "segment",
        expected: undefined,
        actual: { ctxName: meta.ctxName, loc: meta.loc },
      });
    }

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

    const swcNormalized = tryNormalizeCode(match.swcSection.code, swcFilepath);
    const oxcNormalized = tryNormalizeCode(match.oxcSection.code, oxcFilepath);

    if (swcNormalized !== oxcNormalized) {
      failures.push({
        category: FailureCategory.CODE_DIFF,
        expected: swcNormalized,
        actual: oxcNormalized,
      });
    }
  }

  // COMP-04: Parent module comparison
  // Match parent sections by headerName for identity-based pairing
  const swcParents = swcSnapshot.sections.filter((s) => s.metadata === null);
  const oxcParents = oxcSnapshot.sections.filter((s) => s.metadata === null);

  // Build headerName → section lookup for OXC parents
  const oxcParentByName = new Map(oxcParents.map((s) => [s.headerName, s]));
  const matchedOxcNames = new Set<string>();

  for (const swcParent of swcParents) {
    const oxcParent = oxcParentByName.get(swcParent.headerName) ?? null;
    if (oxcParent !== null) {
      matchedOxcNames.add(swcParent.headerName);
      const swcFilepath = getStdinFilepath(swcParent.headerName);
      const oxcFilepath = getStdinFilepath(oxcParent.headerName);
      const swcNorm = tryNormalizeCode(swcParent.code, swcFilepath);
      const oxcNorm = tryNormalizeCode(oxcParent.code, oxcFilepath);
      if (swcNorm !== oxcNorm) {
        failures.push({
          category: FailureCategory.CODE_DIFF,
          field: "parentModule",
          expected: swcNorm,
          actual: oxcNorm,
        });
      }
    } else {
      // SWC has parent but OXC does not
      failures.push({
        category: FailureCategory.CODE_DIFF,
        field: "parentModule",
        expected: swcParent.code,
        actual: undefined,
      });
    }
  }
  // OXC parents not matched to any SWC parent
  for (const oxcParent of oxcParents) {
    if (!matchedOxcNames.has(oxcParent.headerName)) {
      failures.push({
        category: FailureCategory.CODE_DIFF,
        field: "parentModule",
        expected: undefined,
        actual: oxcParent.code,
      });
    }
  }

  // COMP-05: Structural diagnostics comparison (all fields, sorted)
  const swcDiag = swcSnapshot.diagnostics as Array<{ file?: string; loc?: [number, number] }>;
  const oxcDiag = oxcSnapshot.diagnostics as Array<{ file?: string; loc?: [number, number] }>;

  // Canonicalize: sort object keys for order-independent comparison
  const canonicalize = (obj: unknown): unknown => {
    if (obj === null || typeof obj !== "object") return obj;
    if (Array.isArray(obj)) return obj.map(canonicalize);
    const sorted: Record<string, unknown> = {};
    for (const key of Object.keys(obj).sort()) {
      sorted[key] = canonicalize((obj as Record<string, unknown>)[key]);
    }
    return sorted;
  };

  // Sort with canonical JSON tiebreaker for diagnostics sharing the same file/loc
  const stableSort = (arr: typeof swcDiag) =>
    [...arr].sort((a, b) => {
      const cmp = DIAGNOSTICS_SORT_KEY(a).localeCompare(DIAGNOSTICS_SORT_KEY(b));
      return cmp !== 0 ? cmp : JSON.stringify(canonicalize(a)).localeCompare(JSON.stringify(canonicalize(b)));
    });
  const swcSorted = stableSort(swcDiag);
  const oxcSorted = stableSort(oxcDiag);

  if (JSON.stringify(swcSorted.map(canonicalize)) !== JSON.stringify(oxcSorted.map(canonicalize))) {
    failures.push({
      category: FailureCategory.DIAGNOSTICS_MISMATCH,
      field: "diagnostics",
      expected: swcSorted,
      actual: oxcSorted,
    });
  }

  return failures;
}
