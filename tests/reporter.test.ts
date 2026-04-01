/**
 * Unit tests for src/reporter.ts
 *
 * Covers: REPT-01, REPT-02, REPT-04, REPT-05 behaviors
 *
 * REPT-01: Per-fixture PASS/FAIL lines with failure counts
 * REPT-02: Categorized scorecard of non-zero failure categories
 * REPT-04: Human-readable summary (plain text, no ANSI)
 * REPT-05: Algorithm step-trace for derived-field mismatches
 */

import { describe, it, expect } from "bun:test";
import { formatTerminalReport, formatStableJson, explainDerivedFailure } from "../src/reporter.js";
import { FailureCategory } from "../src/contract.js";
import type { HarnessOutput, FailureCategoryValue } from "../src/contract.js";
import type { SegmentMetadata } from "../src/types.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeEmptyByCategory(): Record<FailureCategoryValue, number> {
  const obj = {} as Record<FailureCategoryValue, number>;
  for (const val of Object.values(FailureCategory) as FailureCategoryValue[]) {
    obj[val] = 0;
  }
  return obj;
}

function buildSyntheticOutput(overrides: Partial<HarnessOutput> = {}): HarnessOutput {
  const byCategory = makeEmptyByCategory();
  return {
    fixtures: [],
    summary: {
      total: 0,
      passed: 0,
      failed: 0,
      byCategory,
    },
    ...overrides,
  };
}

function makeMeta(overrides: Partial<SegmentMetadata> = {}): SegmentMetadata {
  return {
    origin: "test.tsx",
    name: "test_component_abc12345678",
    entry: null,
    displayName: "test.tsx_component",
    hash: "abc12345678",
    canonicalFilename: "test.tsx_component_abc12345678",
    path: "",
    extension: "tsx",
    parent: null,
    ctxKind: "function",
    ctxName: "component$",
    captures: false,
    loc: [0, 100],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Task 1: formatTerminalReport
// ---------------------------------------------------------------------------

describe("formatTerminalReport — REPT-01: per-fixture pass/fail lines", () => {
  it("all-pass output shows 0 failures in summary line", () => {
    const output = buildSyntheticOutput({
      fixtures: [
        { name: "fixture_a", pass: true, failures: [] },
        { name: "fixture_b", pass: true, failures: [] },
      ],
      summary: { total: 2, passed: 2, failed: 0, byCategory: makeEmptyByCategory() },
    });

    const report = formatTerminalReport(output);

    expect(report).toContain("0");
    expect(report).toContain("failure");
    expect(report).not.toContain("[FAIL]");
    expect(report).not.toContain("Failures by category");
  });

  it("all-pass output has [PASS] prefix for each fixture", () => {
    const output = buildSyntheticOutput({
      fixtures: [
        { name: "fixture_a", pass: true, failures: [] },
        { name: "fixture_b", pass: true, failures: [] },
      ],
      summary: { total: 2, passed: 2, failed: 0, byCategory: makeEmptyByCategory() },
    });

    const report = formatTerminalReport(output);

    expect(report).toContain("[PASS] fixture_a");
    expect(report).toContain("[PASS] fixture_b");
  });

  it("mixed pass/fail: each fixture has correct prefix", () => {
    const byCategory = makeEmptyByCategory();
    byCategory[FailureCategory.HASH_MISMATCH] = 2;
    byCategory[FailureCategory.DISPLAY_NAME_MISMATCH] = 1;

    const output = buildSyntheticOutput({
      fixtures: [
        { name: "pass_fixture", pass: true, failures: [] },
        {
          name: "fail_fixture",
          pass: false,
          failures: [
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.DISPLAY_NAME_MISMATCH },
          ],
        },
        { name: "another_pass", pass: true, failures: [] },
      ],
      summary: { total: 3, passed: 2, failed: 1, byCategory },
    });

    const report = formatTerminalReport(output);

    expect(report).toContain("[PASS] pass_fixture");
    expect(report).toContain("[FAIL] fail_fixture");
    expect(report).toContain("[PASS] another_pass");
  });

  it("FAIL fixture line shows failure count", () => {
    const byCategory = makeEmptyByCategory();
    byCategory[FailureCategory.HASH_MISMATCH] = 3;

    const output = buildSyntheticOutput({
      fixtures: [
        {
          name: "fail_fixture",
          pass: false,
          failures: [
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.HASH_MISMATCH },
          ],
        },
      ],
      summary: { total: 1, passed: 0, failed: 1, byCategory },
    });

    const report = formatTerminalReport(output);

    expect(report).toContain("[FAIL] fail_fixture (3 failures)");
  });

  it("fixture summary line shows total and failed counts", () => {
    const output = buildSyntheticOutput({
      fixtures: [
        { name: "f1", pass: true, failures: [] },
        { name: "f2", pass: true, failures: [] },
        { name: "f3", pass: true, failures: [] },
      ],
      summary: { total: 3, passed: 3, failed: 0, byCategory: makeEmptyByCategory() },
    });

    const report = formatTerminalReport(output);

    expect(report).toContain("Parsed 3 fixtures.");
    expect(report).toContain("0 failure");
  });
});

describe("formatTerminalReport — REPT-02: categorized scorecard", () => {
  it("scorecard shows only non-zero categories", () => {
    const byCategory = makeEmptyByCategory();
    byCategory[FailureCategory.HASH_MISMATCH] = 5;
    byCategory[FailureCategory.DISPLAY_NAME_MISMATCH] = 2;
    // All other categories remain 0

    const output = buildSyntheticOutput({
      fixtures: [
        {
          name: "fail_f",
          pass: false,
          failures: [
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.HASH_MISMATCH },
            { category: FailureCategory.DISPLAY_NAME_MISMATCH },
            { category: FailureCategory.DISPLAY_NAME_MISMATCH },
          ],
        },
      ],
      summary: { total: 1, passed: 0, failed: 1, byCategory },
    });

    const report = formatTerminalReport(output);

    expect(report).toContain("hash_mismatch: 5");
    expect(report).toContain("display_name_mismatch: 2");
    // Zero categories must not appear in scorecard
    expect(report).not.toContain("segment_count_mismatch: 0");
    expect(report).not.toContain("wrong_captures: 0");
  });

  it("scorecard section header appears only when there are failures", () => {
    const outputWithFailures = buildSyntheticOutput({
      fixtures: [
        {
          name: "f",
          pass: false,
          failures: [{ category: FailureCategory.CODE_DIFF }],
        },
      ],
      summary: {
        total: 1,
        passed: 0,
        failed: 1,
        byCategory: { ...makeEmptyByCategory(), [FailureCategory.CODE_DIFF]: 1 },
      },
    });

    const outputNoFailures = buildSyntheticOutput({
      fixtures: [{ name: "f", pass: true, failures: [] }],
      summary: { total: 1, passed: 1, failed: 0, byCategory: makeEmptyByCategory() },
    });

    const reportWithFailures = formatTerminalReport(outputWithFailures);
    const reportNoFailures = formatTerminalReport(outputNoFailures);

    expect(reportWithFailures).toContain("Failures by category:");
    expect(reportNoFailures).not.toContain("Failures by category:");
  });

  it("plain text only — no ANSI escape sequences", () => {
    const byCategory = makeEmptyByCategory();
    byCategory[FailureCategory.HASH_MISMATCH] = 1;

    const output = buildSyntheticOutput({
      fixtures: [
        { name: "fail_f", pass: false, failures: [{ category: FailureCategory.HASH_MISMATCH }] },
      ],
      summary: { total: 1, passed: 0, failed: 1, byCategory },
    });

    const report = formatTerminalReport(output);

    // ESC character (\x1b or \u001b) must not appear
    expect(report).not.toContain("\x1b");
    expect(report).not.toContain("\u001b");
  });
});

// ---------------------------------------------------------------------------
// Task 1: formatStableJson
// ---------------------------------------------------------------------------

describe("formatStableJson — stability and sorting", () => {
  it("byte-identical on two calls with same input", () => {
    const byCategory = makeEmptyByCategory();
    byCategory[FailureCategory.HASH_MISMATCH] = 3;

    const output = buildSyntheticOutput({
      fixtures: [
        { name: "z_fixture", pass: false, failures: [{ category: FailureCategory.HASH_MISMATCH }] },
        { name: "a_fixture", pass: true, failures: [] },
      ],
      summary: { total: 2, passed: 1, failed: 1, byCategory },
    });

    const json1 = formatStableJson(output);
    const json2 = formatStableJson(output);

    expect(json1).toBe(json2);
  });

  it("fixtures sorted by name ascending", () => {
    const output = buildSyntheticOutput({
      fixtures: [
        { name: "z_fixture", pass: true, failures: [] },
        { name: "a_fixture", pass: true, failures: [] },
        { name: "m_fixture", pass: true, failures: [] },
      ],
      summary: { total: 3, passed: 3, failed: 0, byCategory: makeEmptyByCategory() },
    });

    const json = formatStableJson(output);
    const parsed = JSON.parse(json) as HarnessOutput;

    expect(parsed.fixtures[0].name).toBe("a_fixture");
    expect(parsed.fixtures[1].name).toBe("m_fixture");
    expect(parsed.fixtures[2].name).toBe("z_fixture");
  });

  it("does not mutate the input (fixtures order unchanged)", () => {
    const output = buildSyntheticOutput({
      fixtures: [
        { name: "z_fixture", pass: true, failures: [] },
        { name: "a_fixture", pass: true, failures: [] },
      ],
      summary: { total: 2, passed: 2, failed: 0, byCategory: makeEmptyByCategory() },
    });

    formatStableJson(output);

    // Original input not mutated
    expect(output.fixtures[0].name).toBe("z_fixture");
    expect(output.fixtures[1].name).toBe("a_fixture");
  });

  it("produces valid JSON that can be parsed", () => {
    const output = buildSyntheticOutput({
      fixtures: [{ name: "f", pass: true, failures: [] }],
      summary: { total: 1, passed: 1, failed: 0, byCategory: makeEmptyByCategory() },
    });

    const json = formatStableJson(output);

    expect(() => JSON.parse(json)).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// Task 2: explainDerivedFailure
// ---------------------------------------------------------------------------

describe("explainDerivedFailure — REPT-05: step-trace for derived fields", () => {
  it("returns null for non-derived category (wrong_captures)", () => {
    const swcMeta = makeMeta();
    const oxcMeta = makeMeta({ captures: true });

    const result = explainDerivedFailure(
      { category: FailureCategory.WRONG_CAPTURES, expected: false, actual: true },
      swcMeta,
      oxcMeta,
      null,
      "test.tsx"
    );

    expect(result).toBeNull();
  });

  it("returns null for non-derived category (segment_count_mismatch)", () => {
    const result = explainDerivedFailure(
      { category: FailureCategory.SEGMENT_COUNT_MISMATCH, expected: 2, actual: 1 },
      null,
      null,
      null,
      "test.tsx"
    );

    expect(result).toBeNull();
  });

  it("returns null when swcMeta is null (cannot trace)", () => {
    const result = explainDerivedFailure(
      { category: FailureCategory.HASH_MISMATCH },
      null,
      makeMeta(),
      null,
      "test.tsx"
    );

    expect(result).toBeNull();
  });

  it("returns null when oxcMeta is null (cannot trace)", () => {
    const result = explainDerivedFailure(
      { category: FailureCategory.HASH_MISMATCH },
      makeMeta(),
      null,
      null,
      "test.tsx"
    );

    expect(result).toBeNull();
  });

  it("hash_mismatch: returns string containing scope, relPath, recomputed", () => {
    const swcMeta = makeMeta({
      displayName: "test.tsx_component",
      hash: "abc12345678",
      origin: "test.tsx",
    });
    const oxcMeta = makeMeta({
      displayName: "test.tsx_component",
      hash: "zzz99999999",
      origin: "test.tsx",
    });

    const result = explainDerivedFailure(
      { category: FailureCategory.HASH_MISMATCH, expected: "abc12345678", actual: "zzz99999999" },
      swcMeta,
      oxcMeta,
      null,
      "test.tsx"
    );

    expect(result).not.toBeNull();
    expect(result).toContain("scope:");
    expect(result).toContain("relPath:");
    expect(result).toContain("recomputed:");
  });

  it("hash_mismatch: trace shows expected (SWC) and actual (OXC) values", () => {
    const swcMeta = makeMeta({
      displayName: "test.tsx_component",
      hash: "abc12345678",
      origin: "test.tsx",
    });
    const oxcMeta = makeMeta({
      displayName: "test.tsx_component",
      hash: "zzz99999999",
      origin: "test.tsx",
    });

    const result = explainDerivedFailure(
      { category: FailureCategory.HASH_MISMATCH, expected: "abc12345678", actual: "zzz99999999" },
      swcMeta,
      oxcMeta,
      null,
      "test.tsx"
    );

    expect(result).toContain("abc12345678");
    expect(result).toContain("zzz99999999");
  });

  it("display_name_mismatch: returns string with both displayNames and file prefix info", () => {
    const swcMeta = makeMeta({
      displayName: "test.tsx_component",
      origin: "test.tsx",
    });
    const oxcMeta = makeMeta({
      displayName: "test.tsx_Component",
      origin: "test.tsx",
    });

    const result = explainDerivedFailure(
      {
        category: FailureCategory.DISPLAY_NAME_MISMATCH,
        expected: "test.tsx_component",
        actual: "test.tsx_Component",
      },
      swcMeta,
      oxcMeta,
      null,
      "test.tsx"
    );

    expect(result).not.toBeNull();
    expect(result).toContain("test.tsx_component");
    expect(result).toContain("test.tsx_Component");
    expect(result).toContain("test.tsx_");
  });

  it("display_name_mismatch: contains prePrefix for both sides", () => {
    const swcMeta = makeMeta({
      displayName: "test.tsx_component",
      origin: "test.tsx",
    });
    const oxcMeta = makeMeta({
      displayName: "test.tsx_Component",
      origin: "test.tsx",
    });

    const result = explainDerivedFailure(
      {
        category: FailureCategory.DISPLAY_NAME_MISMATCH,
        expected: "test.tsx_component",
        actual: "test.tsx_Component",
      },
      swcMeta,
      oxcMeta,
      null,
      "test.tsx"
    );

    expect(result).toContain("prePrefix");
    expect(result).toContain("component");
    expect(result).toContain("Component");
  });

  it("canonical_filename_mismatch: returns string with displayName and hash suffix", () => {
    const swcMeta = makeMeta({
      displayName: "test.tsx_component",
      name: "test_component_abc12345678",
      canonicalFilename: "test.tsx_component_abc12345678",
      origin: "test.tsx",
    });
    const oxcMeta = makeMeta({
      displayName: "test.tsx_component",
      name: "test_component_zzz99999999",
      canonicalFilename: "test.tsx_component_zzz99999999",
      origin: "test.tsx",
    });

    const result = explainDerivedFailure(
      {
        category: FailureCategory.CANONICAL_FILENAME_MISMATCH,
        expected: "test.tsx_component_abc12345678",
        actual: "test.tsx_component_zzz99999999",
      },
      swcMeta,
      oxcMeta,
      null,
      "test.tsx"
    );

    expect(result).not.toBeNull();
    expect(result).toContain("test.tsx_component");
    // hashSuffix is last _ token from name
    expect(result).toContain("abc12345678");
  });

  it("canonical_filename_mismatch: shows recomputed vs actual", () => {
    const swcMeta = makeMeta({
      displayName: "test.tsx_component",
      name: "test_component_abc12345678",
      canonicalFilename: "test.tsx_component_abc12345678",
    });
    const oxcMeta = makeMeta({
      displayName: "test.tsx_component",
      name: "test_component_zzz99999999",
      canonicalFilename: "test.tsx_component_zzz99999999",
    });

    const result = explainDerivedFailure(
      {
        category: FailureCategory.CANONICAL_FILENAME_MISMATCH,
        expected: "test.tsx_component_abc12345678",
        actual: "test.tsx_component_zzz99999999",
      },
      swcMeta,
      oxcMeta,
      null,
      "test.tsx"
    );

    expect(result).toContain("recomputed");
    expect(result).toContain("test.tsx_component_abc12345678");
    expect(result).toContain("test.tsx_component_zzz99999999");
  });
});
