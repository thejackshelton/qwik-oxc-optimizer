/**
 * Thin-slice integration test: 7 representative fixtures through parse -> normalize pipeline.
 *
 * Phase gate: validates end-to-end pipeline works before Phase 4 builds full comparison matrix.
 * Uses stub comparison (all pass: true, zero failures) — real comparison comes in Phase 4.
 *
 * Requirement: THIN-01
 */

import { describe, test, expect } from "bun:test";
import * as path from "node:path";
import { parseSnapFile } from "../src/parser.js";
import { normalizeCode, assertOxfmtVersion } from "../src/normalizer.js";
import { FailureCategory, type HarnessOutput, type FailureCategoryValue } from "../src/contract.js";

// 7 representative fixtures (from 02-RESEARCH.md thin-slice selection)
const THIN_SLICE_FIXTURES = [
  "example_1",                                 // 3 segments, source maps, standard
  "example_11",                                // 0 ENTRY POINT labels
  "example_build_server",                      // 0 segments, parent-only
  "example_missing_custom_inlined_functions",  // non-empty diagnostics
  "component_level_self_referential_qrl",      // captureNames present
  "support_windows_paths",                     // backslash path edge case
  "example_prod_node",                         // multiple segments with source maps
];

// Resolve path to swc-snapshots directory
const SNAP_DIR = path.join(import.meta.dir, "..", "swc-snapshots");

/**
 * Get a stdinFilepath for normalizeCode from a section headerName.
 * Falls back to "snapshot-section.tsx" if headerName lacks a recognized extension.
 */
function getStdinFilepath(headerName: string): string {
  const recognized = [".ts", ".tsx", ".js", ".jsx", ".mjs"];
  const ext = path.extname(headerName);
  if (ext && recognized.includes(ext)) {
    return headerName;
  }
  return "snapshot-section.tsx";
}

/**
 * Build an empty byCategory record with all 19 FailureCategory keys set to 0.
 */
function buildEmptyByCategory(): Record<FailureCategoryValue, number> {
  const result = {} as Record<FailureCategoryValue, number>;
  for (const value of Object.values(FailureCategory) as FailureCategoryValue[]) {
    result[value] = 0;
  }
  return result;
}

describe("thin-slice integration", () => {
  test("oxfmt version check passes", () => {
    expect(() => assertOxfmtVersion()).not.toThrow();
  });

  test("all 7 fixtures parse without error", () => {
    for (const fixture of THIN_SLICE_FIXTURES) {
      const snapPath = path.join(SNAP_DIR, `${fixture}.snap`);
      expect(() => parseSnapFile(snapPath)).not.toThrow();
      const parsed = parseSnapFile(snapPath);
      expect(parsed.fixtureName).toBe(fixture);
      expect(Array.isArray(parsed.sections)).toBe(true);
      expect(Array.isArray(parsed.diagnostics)).toBe(true);
    }
  });

  test("all sections normalize without error", () => {
    for (const fixture of THIN_SLICE_FIXTURES) {
      const snapPath = path.join(SNAP_DIR, `${fixture}.snap`);
      const parsed = parseSnapFile(snapPath);
      for (const section of parsed.sections) {
        const stdinFilepath = getStdinFilepath(section.headerName);
        expect(() => normalizeCode(section.code, stdinFilepath)).not.toThrow();
      }
    }
  });

  test("pipeline produces valid HarnessOutput shape", () => {
    const fixtureResults: HarnessOutput["fixtures"] = [];

    for (const fixture of THIN_SLICE_FIXTURES) {
      const snapPath = path.join(SNAP_DIR, `${fixture}.snap`);
      const parsed = parseSnapFile(snapPath);

      // Normalize all sections (stub comparison: always pass)
      for (const section of parsed.sections) {
        const stdinFilepath = getStdinFilepath(section.headerName);
        normalizeCode(section.code, stdinFilepath);
      }

      fixtureResults.push({
        name: parsed.fixtureName,
        pass: true,
        failures: [],
      });
    }

    const byCategory = buildEmptyByCategory();
    const output: HarnessOutput = {
      fixtures: fixtureResults,
      summary: {
        total: fixtureResults.length,
        passed: fixtureResults.length,
        failed: 0,
        byCategory,
      },
    };

    // Validate shape
    expect(output.fixtures).toHaveLength(7);
    expect(output.fixtures.every(f => f.pass)).toBe(true);
    expect(output.fixtures.every(f => f.failures.length === 0)).toBe(true);
    expect(output.summary.total).toBe(7);
    expect(output.summary.passed).toBe(7);
    expect(output.summary.failed).toBe(0);

    // All 19 FailureCategory keys must be present and 0
    const categoryKeys = Object.values(FailureCategory) as FailureCategoryValue[];
    expect(categoryKeys).toHaveLength(19);
    for (const key of categoryKeys) {
      expect(output.summary.byCategory[key]).toBe(0);
    }
  });

  test("normalized code is non-empty for non-empty input sections", () => {
    for (const fixture of THIN_SLICE_FIXTURES) {
      const snapPath = path.join(SNAP_DIR, `${fixture}.snap`);
      const parsed = parseSnapFile(snapPath);
      for (const section of parsed.sections) {
        if (section.code.trim().length === 0) continue; // skip empty sections
        const stdinFilepath = getStdinFilepath(section.headerName);
        const normalized = normalizeCode(section.code, stdinFilepath);
        expect(normalized.trim().length).toBeGreaterThan(0);
      }
    }
  });

  test("metadata.loc preserved through pipeline", () => {
    const snapPath = path.join(SNAP_DIR, "example_1.snap");
    const parsed = parseSnapFile(snapPath);

    // Find a segment section (has metadata with loc)
    const segmentSection = parsed.sections.find(s => s.metadata !== null);
    expect(segmentSection).toBeDefined();
    expect(segmentSection!.metadata).not.toBeNull();

    const locBefore = segmentSection!.metadata!.loc;
    expect(Array.isArray(locBefore)).toBe(true);
    expect(locBefore).toHaveLength(2);

    // Normalize code — loc lives in metadata, not in code
    const stdinFilepath = getStdinFilepath(segmentSection!.headerName);
    normalizeCode(segmentSection!.code, stdinFilepath);

    // Re-parse to confirm loc is stable (not mutated)
    const reparsed = parseSnapFile(snapPath);
    const segmentSectionReparsed = reparsed.sections.find(s => s.metadata !== null);
    expect(segmentSectionReparsed!.metadata!.loc).toEqual(locBefore);
  });

  test("fixture with captureNames preserves field through pipeline", () => {
    const snapPath = path.join(SNAP_DIR, "component_level_self_referential_qrl.snap");
    const parsed = parseSnapFile(snapPath);

    // Find section with captureNames
    const sectionWithCaptures = parsed.sections.find(
      s => s.metadata !== null && s.metadata.captureNames !== undefined
    );
    expect(sectionWithCaptures).toBeDefined();
    expect(sectionWithCaptures!.metadata!.captureNames).toBeDefined();

    const captureNamesBefore = sectionWithCaptures!.metadata!.captureNames!;
    expect(Array.isArray(captureNamesBefore)).toBe(true);
    expect(captureNamesBefore.length).toBeGreaterThan(0);

    // Normalize the section's code
    const stdinFilepath = getStdinFilepath(sectionWithCaptures!.headerName);
    normalizeCode(sectionWithCaptures!.code, stdinFilepath);

    // captureNames lives in metadata, not in code — must be unchanged
    expect(sectionWithCaptures!.metadata!.captureNames).toEqual(captureNamesBefore);
  });
});
