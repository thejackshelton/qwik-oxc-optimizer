/**
 * Spec-derived identity recomputation functions.
 *
 * These pure functions recompute display_name, hash, and canonical_filename
 * from known inputs so the harness can independently verify that OXC produces
 * the correct identity fields (SPEC-01, SPEC-02, SPEC-03).
 *
 * All functions are pure (no I/O, no side effects).
 */

import * as path from "node:path";
import type { SegmentMetadata } from "./types.js";

// SipHash13 uses CommonJS exports — load via require-style dynamic import
// to avoid ESM/CJS interop issues at the call site.
// We use the SipHash-1-3 variant which matches Rust's DefaultHasher.
// eslint-disable-next-line @typescript-eslint/no-require-imports
const SipHash13 = require("siphash/lib/siphash13.js") as {
  hash: (key: Uint32Array, m: Uint8Array) => { h: number; l: number };
};

/** SipHash-1-3 seed (0, 0) key — all 16 bytes zero */
const SIPHASH_ZERO_KEY = new Uint32Array([0, 0, 0, 0]);

// ---------------------------------------------------------------------------
// escapeSymbol
// ---------------------------------------------------------------------------

/**
 * Replace non-alphanumeric characters with underscores, squash consecutive
 * underscores, and trim leading/trailing underscores.
 *
 * This mirrors Rust's `escape_sym` function for the stack_ctxt portion of
 * display_name computation. NOTE: the file_name prefix in displayName uses
 * the RAW filename (not escape_sym'd) — only the pre-prefix portion is
 * processed through escapeSymbol.
 *
 * Does NOT add a digit-prefix underscore — that is the caller's responsibility
 * per SPEC Step 3.
 */
export function escapeSymbol(s: string): string {
  return s
    .replace(/[^A-Za-z0-9]+/g, "_") // non-alphanumeric → _
    .replace(/_+/g, "_") // squash consecutive _
    .replace(/^_+|_+$/g, ""); // trim leading/trailing _
}

// ---------------------------------------------------------------------------
// decomposeDisplayName
// ---------------------------------------------------------------------------

/**
 * Decompose a stored displayName into its file prefix and pre-prefix parts.
 *
 * The stored displayName format is:
 *   `{file_basename_with_extension}_{pre_prefix_display_name}`
 *
 * IMPORTANT: The file prefix uses the RAW filename with extension (NOT
 * escape_sym'd). For example, "test.tsx" → prefix is "test.tsx_", not
 * "test_tsx_". This is confirmed by snap data.
 *
 * @param displayName - The stored displayName from SegmentMetadata
 * @param origin - The origin path from SegmentMetadata (may be a relative path)
 * @returns { fileNamePrefix, prePrefix } or null if displayName doesn't match
 */
export function decomposeDisplayName(
  displayName: string,
  origin: string
): { fileNamePrefix: string; prePrefix: string } | null {
  const fileNamePrefix = path.basename(origin); // raw basename with extension
  const expectedStart = fileNamePrefix + "_";
  if (!displayName.startsWith(expectedStart)) {
    return null;
  }
  const prePrefix = displayName.slice(expectedStart.length);
  return { fileNamePrefix, prePrefix };
}

// ---------------------------------------------------------------------------
// recomputeHash
// ---------------------------------------------------------------------------

/**
 * Recompute the 11-character SipHash-based hash for a segment.
 *
 * Algorithm (SPEC §Hash):
 *   1. Concatenate raw bytes (no separators):
 *      - scope bytes (if scope is non-null/non-empty)
 *      - relPath bytes
 *      - preFilePrefixDisplayName bytes
 *   2. SipHash-1-3 with seed (0, 0) over the concatenated bytes
 *   3. u64 → 8 little-endian bytes
 *   4. Buffer.from(leBytes).toString("base64url")
 *   5. Replace all `-` and `_` with `0`
 *
 * @param scope - TransformModulesOptions.scope (null/undefined if absent)
 * @param relPath - Forward-slash path of source file relative to src_dir.
 *   Backslashes are normalized to forward slashes (handles Windows paths).
 * @param preFilePrefixDisplayName - The pre-prefix portion of displayName
 *   (i.e., displayName with the leading "{basename}_" stripped)
 * @returns 11-character hash string
 */
export function recomputeHash(
  scope: string | null | undefined,
  relPath: string,
  preFilePrefixDisplayName: string
): string {
  const encoder = new TextEncoder();

  // Normalize Windows backslashes to forward slashes
  const normalizedRelPath = relPath.replace(/\\/g, "/");

  // Build byte arrays in order: scope?, relPath, prePrefix
  const parts: Uint8Array[] = [];
  if (scope) {
    parts.push(encoder.encode(scope));
  }
  parts.push(encoder.encode(normalizedRelPath));
  parts.push(encoder.encode(preFilePrefixDisplayName));

  // Concatenate all byte arrays
  const totalLength = parts.reduce((sum, p) => sum + p.length, 0);
  const allBytes = new Uint8Array(totalLength);
  let offset = 0;
  for (const part of parts) {
    allBytes.set(part, offset);
    offset += part.length;
  }

  // SipHash-1-3 with seed (0, 0)
  const result = SipHash13.hash(SIPHASH_ZERO_KEY, allBytes);

  // Convert {h, l} pair to 8 little-endian bytes (l = low 32 bits, h = high 32 bits)
  const leBytes = new Uint8Array(8);
  const view = new DataView(leBytes.buffer);
  view.setUint32(0, result.l, true); // low 32 bits, LE
  view.setUint32(4, result.h, true); // high 32 bits, LE

  // base64url encode then replace - and _ with 0
  const encoded = Buffer.from(leBytes).toString("base64url");
  return encoded.replace(/[-_]/g, "0"); // always 11 chars
}

// ---------------------------------------------------------------------------
// validateDisplayName
// ---------------------------------------------------------------------------

/**
 * Validate that a segment's stored displayName is structurally correct.
 *
 * Checks that:
 *   1. The displayName starts with the expected "{basename}_" prefix
 *   2. The pre-prefix portion is non-empty
 *
 * Returns null on success, or a descriptive error string on failure.
 *
 * Note: Full re-derivation of displayName from stack_ctxt is NOT possible
 * because stack_ctxt is not stored in snapshots. This function validates
 * structural invariants only.
 */
export function validateDisplayName(
  metadata: SegmentMetadata,
  origin: string
): string | null {
  const decomposed = decomposeDisplayName(metadata.displayName, origin);
  if (decomposed === null) {
    const expectedPrefix = path.basename(origin) + "_";
    return (
      `displayName "${metadata.displayName}" does not start with expected ` +
      `file prefix "${expectedPrefix}" (origin: "${origin}")`
    );
  }
  if (decomposed.prePrefix.length === 0) {
    return (
      `displayName "${metadata.displayName}" has empty pre-prefix ` +
      `after stripping file prefix`
    );
  }
  return null;
}

// ---------------------------------------------------------------------------
// recomputeCanonicalFilename
// ---------------------------------------------------------------------------

/**
 * Recompute the canonical filename for a segment.
 *
 * Algorithm (SPEC §Canonical Filename):
 *   Split symbolName on `_`, take the last token as hashSuffix,
 *   return `{displayName}_{hashSuffix}`.
 *
 * @param displayName - The segment's displayName
 * @param symbolName - The segment's name (symbol name), e.g. "renderHeader1_div_onClick_USi8k1jUb40"
 * @returns The expected canonicalFilename
 */
export function recomputeCanonicalFilename(
  displayName: string,
  symbolName: string
): string {
  const tokens = symbolName.split("_");
  const hashSuffix = tokens[tokens.length - 1];
  return `${displayName}_${hashSuffix}`;
}

// ---------------------------------------------------------------------------
// validateCanonicalFilename
// ---------------------------------------------------------------------------

/**
 * Validate that a segment's stored canonicalFilename matches the recomputed value.
 *
 * Returns null on match, or a descriptive error string on mismatch.
 */
export function validateCanonicalFilename(
  metadata: SegmentMetadata
): string | null {
  const recomputed = recomputeCanonicalFilename(
    metadata.displayName,
    metadata.name
  );
  if (metadata.canonicalFilename !== recomputed) {
    return (
      `canonicalFilename mismatch: stored "${metadata.canonicalFilename}" ` +
      `vs recomputed "${recomputed}" ` +
      `(displayName: "${metadata.displayName}", name: "${metadata.name}")`
    );
  }
  return null;
}
