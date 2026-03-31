/**
 * Frozen in Phase 0 -- must not change after initial commit without explicit justification.
 *
 * This file is the single source of truth for:
 * - FailureCategory enum (all 19 values)
 * - HarnessOutput JSON schema
 * - OXFMT_VERSION constant (must match package.json devDependencies.oxfmt)
 * - CORPUS_SIZE constant
 * - DIAGNOSTICS_SORT_KEY function
 *
 * All downstream phases import from this file. Do not inline these constants elsewhere.
 */

export const HARNESS_VERSION = "1.0.0";

/** Must match devDependencies.oxfmt in package.json */
export const OXFMT_VERSION = "0.32.0";

export const FIXTURES_JSON_VERSION = 1;

export const CORPUS_SIZE = 201;

/**
 * All 19 failure categories used by the comparison harness.
 * Frozen in Phase 0 — adding a new category requires an explicit commit to this file.
 */
export const FailureCategory = {
  // Structural: segment-level mismatches
  SEGMENT_COUNT_MISMATCH: "segment_count_mismatch",
  MISSING_SEGMENT: "missing_segment",
  EXTRA_SEGMENT: "extra_segment",
  // Metadata: per-segment field mismatches (15 fields per COMP-02 / PARSE-02)
  HASH_MISMATCH: "hash_mismatch",
  DISPLAY_NAME_MISMATCH: "display_name_mismatch",
  CANONICAL_FILENAME_MISMATCH: "canonical_filename_mismatch",
  WRONG_CAPTURES: "wrong_captures",
  WRONG_CAPTURE_NAMES: "wrong_capture_names",
  WRONG_CTX_KIND: "wrong_ctx_kind",
  WRONG_CTX_NAME: "wrong_ctx_name",
  WRONG_PARENT: "wrong_parent",
  WRONG_ENTRY: "wrong_entry",
  WRONG_LOC: "wrong_loc",
  WRONG_PARAM_NAMES: "wrong_param_names",
  WRONG_EXTENSION: "wrong_extension",
  WRONG_ORIGIN: "wrong_origin",
  WRONG_PATH: "wrong_path",
  // Code and diagnostics
  CODE_DIFF: "code_diff",
  DIAGNOSTICS_MISMATCH: "diagnostics_mismatch",
} as const;

export type FailureCategoryValue =
  (typeof FailureCategory)[keyof typeof FailureCategory];

/**
 * Frozen JSON output schema (emitted with --json flag).
 * Matches REPT-03 specification.
 */
export interface HarnessOutput {
  fixtures: Array<{
    name: string;
    pass: boolean;
    failures: Array<{
      category: FailureCategoryValue;
      field?: string;
      expected?: unknown;
      actual?: unknown;
    }>;
  }>;
  summary: {
    total: number;
    passed: number;
    failed: number;
    byCategory: Record<FailureCategoryValue, number>;
  };
}

/**
 * Diagnostics ordering rule: sort by [file, line, column] ascending before comparison.
 * Apply this sort to both SWC and OXC diagnostic arrays before diffing them.
 */
export const DIAGNOSTICS_SORT_KEY = (d: {
  file?: string;
  loc?: [number, number];
}): string =>
  `${d.file ?? ""}:${String(d.loc?.[0] ?? 0)}:${String(d.loc?.[1] ?? 0)}`;
