/**
 * Unit tests for src/comparator.ts
 *
 * Covers: COMP-01, COMP-02, COMP-03, COMP-04, COMP-05, COMP-06, COMP-07
 *
 * COMP-01: Segment count mismatch detected; per-segment work skipped
 * COMP-02: All 14 metadata fields checked independently (no short-circuit)
 * COMP-03: Normalized code comparison; code_diff reported when different
 * COMP-04: Parent module code compared after normalization
 * COMP-05: Diagnostics comparison is structural (all fields, sorted)
 * COMP-06: Every failure has a typed FailureCategory value
 * COMP-07: loc mismatch reported as WRONG_LOC; never suppressed
 */

import { describe, it, expect } from "bun:test";
import * as path from "node:path";
import { parseSnapFile } from "../src/parser.js";
import { compareFixture } from "../src/comparator.js";
import { FailureCategory } from "../src/contract.js";
import type { ParsedSection, ParsedSnapshot, SegmentMetadata } from "../src/types.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeMeta(overrides: Partial<SegmentMetadata> = {}): SegmentMetadata {
  return {
    origin: "test.tsx",
    name: "test_abc12345678",
    entry: null,
    displayName: "test",
    hash: "abc12345678",
    canonicalFilename: "test_abc12345678",
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

function makeSyntheticSection(
  overrides: Partial<SegmentMetadata> = {},
  sectionOverrides: Partial<Omit<ParsedSection, "metadata">> = {}
): ParsedSection {
  return {
    headerName: "test_section.tsx",
    isEntryPoint: false,
    code: "",
    sourceMap: null,
    metadata: makeMeta(overrides),
    ...sectionOverrides,
  };
}

function makeSnapshot(
  sections: ParsedSection[],
  name = "fixture_test"
): ParsedSnapshot {
  return {
    fixtureName: name,
    input: "const x = 1;",
    sections,
    diagnostics: [],
  };
}

// ---------------------------------------------------------------------------
// COMP-01: Structural segment mismatch reporting (REPT-01, SEG-02)
// ---------------------------------------------------------------------------

describe("compareFixture — COMP-01: structural segment mismatch reporting", () => {
  it("returns MISSING_SEGMENT with identity when SWC has 2 segments and OXC has 1", () => {
    const swcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }),
    ];
    const oxcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
    ];

    const swcSnap = makeSnapshot(swcSections);
    const oxcSnap = makeSnapshot(oxcSections);

    const failures = compareFixture(swcSnap, oxcSnap);

    // No SEGMENT_COUNT_MISMATCH — only typed structural failures
    expect(failures.some((f) => f.category === FailureCategory.SEGMENT_COUNT_MISMATCH)).toBe(false);
    expect(failures).toHaveLength(1);
    expect(failures[0].category).toBe(FailureCategory.MISSING_SEGMENT);
    expect(failures[0].expected).toEqual({ ctxName: "useTask$", loc: [101, 200] });
    expect(failures[0].actual).toBeUndefined();
  });

  it("does NOT return per-segment metadata failures when segment count differs", () => {
    const swcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }),
    ];
    const oxcSections = [
      // OXC has metadata mismatch too, but should not be reported — only structural failures
      makeSyntheticSection({ ctxName: "differentCtx$", hash: "different1234", loc: [999, 9999] }),
    ];

    const swcSnap = makeSnapshot(swcSections);
    const oxcSnap = makeSnapshot(oxcSections);

    const failures = compareFixture(swcSnap, oxcSnap);

    // Only structural failures — no metadata failures (hash_mismatch, display_name_mismatch, etc.)
    const metadataCategories = new Set([
      FailureCategory.HASH_MISMATCH,
      FailureCategory.DISPLAY_NAME_MISMATCH,
      FailureCategory.CANONICAL_FILENAME_MISMATCH,
      FailureCategory.WRONG_CAPTURES,
      FailureCategory.WRONG_CAPTURE_NAMES,
      FailureCategory.WRONG_CTX_KIND,
      FailureCategory.WRONG_CTX_NAME,
      FailureCategory.WRONG_PARENT,
      FailureCategory.WRONG_ENTRY,
      FailureCategory.WRONG_LOC,
      FailureCategory.WRONG_PARAM_NAMES,
      FailureCategory.WRONG_EXTENSION,
      FailureCategory.WRONG_ORIGIN,
      FailureCategory.WRONG_PATH,
    ]);
    const metadataFailures = failures.filter((f) => metadataCategories.has(f.category));
    expect(metadataFailures).toHaveLength(0);
  });

  it("returns EXTRA_SEGMENT with identity when OXC has more segments than SWC", () => {
    const swcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
    ];
    const oxcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }),
    ];

    const swcSnap = makeSnapshot(swcSections);
    const oxcSnap = makeSnapshot(oxcSections);

    const failures = compareFixture(swcSnap, oxcSnap);

    expect(failures).toHaveLength(1);
    expect(failures[0].category).toBe(FailureCategory.EXTRA_SEGMENT);
    expect(failures[0].expected).toBeUndefined();
    expect(failures[0].actual).toEqual({ ctxName: "useTask$", loc: [101, 200] });
  });

  it("returns multiple MISSING_SEGMENT failures when OXC has 0 segments and SWC has 3", () => {
    const swcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }),
      makeSyntheticSection({ ctxName: "useVisibleTask$", loc: [201, 300] }),
    ];

    const swcSnap = makeSnapshot(swcSections);
    const oxcSnap = makeSnapshot([]);

    const failures = compareFixture(swcSnap, oxcSnap);

    const missingFailures = failures.filter((f) => f.category === FailureCategory.MISSING_SEGMENT);
    expect(missingFailures).toHaveLength(3);
    expect(missingFailures[0].expected).toEqual({ ctxName: "component$", loc: [0, 100] });
    expect(missingFailures[1].expected).toEqual({ ctxName: "useTask$", loc: [101, 200] });
    expect(missingFailures[2].expected).toEqual({ ctxName: "useVisibleTask$", loc: [201, 300] });
    for (const f of missingFailures) {
      expect(f.actual).toBeUndefined();
    }
  });

  it("MISSING_SEGMENT failure carries field='segment'", () => {
    const swcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }),
    ];
    const oxcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
    ];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const missingFailures = failures.filter((f) => f.category === FailureCategory.MISSING_SEGMENT);
    for (const f of missingFailures) {
      expect(f.field).toBe("segment");
    }
  });

  it("EXTRA_SEGMENT failure carries field='segment'", () => {
    const swcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
    ];
    const oxcSections = [
      makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }),
    ];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const extraFailures = failures.filter((f) => f.category === FailureCategory.EXTRA_SEGMENT);
    for (const f of extraFailures) {
      expect(f.field).toBe("segment");
    }
  });

  it("no SEGMENT_COUNT_MISMATCH failure is ever emitted (all mismatch scenarios)", () => {
    // Scenario A: SWC > OXC
    const scenarioA = compareFixture(
      makeSnapshot([
        makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
        makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }),
      ]),
      makeSnapshot([makeSyntheticSection({ ctxName: "component$", loc: [0, 100] })])
    );
    expect(scenarioA.some((f) => f.category === FailureCategory.SEGMENT_COUNT_MISMATCH)).toBe(false);

    // Scenario B: OXC > SWC
    const scenarioB = compareFixture(
      makeSnapshot([makeSyntheticSection({ ctxName: "component$", loc: [0, 100] })]),
      makeSnapshot([
        makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }),
        makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }),
      ])
    );
    expect(scenarioB.some((f) => f.category === FailureCategory.SEGMENT_COUNT_MISMATCH)).toBe(false);

    // Scenario C: OXC has 0 segments
    const scenarioC = compareFixture(
      makeSnapshot([makeSyntheticSection({ ctxName: "component$", loc: [0, 100] })]),
      makeSnapshot([])
    );
    expect(scenarioC.some((f) => f.category === FailureCategory.SEGMENT_COUNT_MISMATCH)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// COMP-02: Metadata field checks — independent, no short-circuit
// ---------------------------------------------------------------------------

describe("compareFixture — COMP-02: metadata fields checked independently", () => {
  it("reports hash_mismatch only when hash differs", () => {
    const swcSections = [makeSyntheticSection({ hash: "abc12345678" })];
    const oxcSections = [makeSyntheticSection({ hash: "zzz99999999" })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const hashFailures = failures.filter((f) => f.category === FailureCategory.HASH_MISMATCH);
    expect(hashFailures).toHaveLength(1);
    // No other field failures since all other fields match
    const otherFieldFailures = failures.filter(
      (f) =>
        f.category !== FailureCategory.HASH_MISMATCH &&
        f.category !== FailureCategory.CODE_DIFF
    );
    expect(otherFieldFailures).toHaveLength(0);
  });

  it("reports ALL differing field categories (no short-circuit) when multiple fields differ", () => {
    const swcSections = [
      makeSyntheticSection({
        hash: "abc12345678",
        displayName: "oldName",
        ctxKind: "function",
        extension: "tsx",
      }),
    ];
    const oxcSections = [
      makeSyntheticSection({
        hash: "zzz99999999",
        displayName: "newName",
        ctxKind: "eventHandler",
        extension: "ts",
      }),
    ];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const categories = failures.map((f) => f.category);
    expect(categories).toContain(FailureCategory.HASH_MISMATCH);
    expect(categories).toContain(FailureCategory.DISPLAY_NAME_MISMATCH);
    expect(categories).toContain(FailureCategory.WRONG_CTX_KIND);
    expect(categories).toContain(FailureCategory.WRONG_EXTENSION);
  });

  it("reports no failures for identical metadata (all 14 fields match)", () => {
    const section = makeSyntheticSection();
    const failures = compareFixture(
      makeSnapshot([section]),
      makeSnapshot([{ ...section }])
    );
    // Only potential failure: CODE_DIFF if code differs — both empty here
    const metaFailures = failures.filter((f) => f.category !== FailureCategory.CODE_DIFF);
    expect(metaFailures).toHaveLength(0);
  });

  it("checks captures boolean field independently", () => {
    const swcSections = [makeSyntheticSection({ captures: false })];
    const oxcSections = [makeSyntheticSection({ captures: true })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const captureFailures = failures.filter((f) => f.category === FailureCategory.WRONG_CAPTURES);
    expect(captureFailures).toHaveLength(1);
    expect(captureFailures[0].expected).toBe(false);
    expect(captureFailures[0].actual).toBe(true);
  });

  it("checks canonicalFilename field independently", () => {
    const swcSections = [makeSyntheticSection({ canonicalFilename: "test_abc12345678" })];
    const oxcSections = [makeSyntheticSection({ canonicalFilename: "test_zzz99999999" })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const canonicalFailures = failures.filter(
      (f) => f.category === FailureCategory.CANONICAL_FILENAME_MISMATCH
    );
    expect(canonicalFailures).toHaveLength(1);
  });

  it("checks ctxName field independently", () => {
    const swcSections = [makeSyntheticSection({ ctxName: "component$" })];
    const oxcSections = [makeSyntheticSection({ ctxName: "useTask$" })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const ctxNameFailures = failures.filter((f) => f.category === FailureCategory.WRONG_CTX_NAME);
    expect(ctxNameFailures).toHaveLength(1);
  });

  it("checks parent field independently", () => {
    const swcSections = [makeSyntheticSection({ parent: null })];
    const oxcSections = [makeSyntheticSection({ parent: "parent_abc12345678" })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const parentFailures = failures.filter((f) => f.category === FailureCategory.WRONG_PARENT);
    expect(parentFailures).toHaveLength(1);
    expect(parentFailures[0].expected).toBeNull();
    expect(parentFailures[0].actual).toBe("parent_abc12345678");
  });

  it("checks entry field independently", () => {
    const swcSections = [makeSyntheticSection({ entry: null })];
    const oxcSections = [makeSyntheticSection({ entry: "some-bundle-key" })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const entryFailures = failures.filter((f) => f.category === FailureCategory.WRONG_ENTRY);
    expect(entryFailures).toHaveLength(1);
  });

  it("checks origin field independently", () => {
    const swcSections = [makeSyntheticSection({ origin: "test.tsx" })];
    const oxcSections = [makeSyntheticSection({ origin: "other.tsx" })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const originFailures = failures.filter((f) => f.category === FailureCategory.WRONG_ORIGIN);
    expect(originFailures).toHaveLength(1);
  });

  it("checks path field independently", () => {
    const swcSections = [makeSyntheticSection({ path: "" })];
    const oxcSections = [makeSyntheticSection({ path: "some/subdir" })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const pathFailures = failures.filter((f) => f.category === FailureCategory.WRONG_PATH);
    expect(pathFailures).toHaveLength(1);
  });

  it("checks paramNames array field independently", () => {
    const swcSections = [makeSyntheticSection({ paramNames: ["a", "b"] })];
    const oxcSections = [makeSyntheticSection({ paramNames: ["a", "c"] })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const paramFailures = failures.filter((f) => f.category === FailureCategory.WRONG_PARAM_NAMES);
    expect(paramFailures).toHaveLength(1);
  });

  it("checks captureNames array field independently", () => {
    const swcSections = [makeSyntheticSection({ captureNames: ["x"] })];
    const oxcSections = [makeSyntheticSection({ captureNames: ["y"] })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const captureNamesFailures = failures.filter(
      (f) => f.category === FailureCategory.WRONG_CAPTURE_NAMES
    );
    expect(captureNamesFailures).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// COMP-03: Code comparison
// ---------------------------------------------------------------------------

describe("compareFixture — COMP-03: normalized code comparison", () => {
  it("returns code_diff when SWC code is 'const a = 1;' and OXC code is 'const b = 1;'", () => {
    const swcSections = [
      makeSyntheticSection({}, { code: "const a = 1;" }),
    ];
    const oxcSections = [
      makeSyntheticSection({}, { code: "const b = 1;" }),
    ];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const codeDiffs = failures.filter((f) => f.category === FailureCategory.CODE_DIFF);
    expect(codeDiffs).toHaveLength(1);
  });

  it("returns no code_diff when both code blocks are identical after normalization", () => {
    const code = "const a = 1;";
    const swcSections = [makeSyntheticSection({}, { code })];
    const oxcSections = [makeSyntheticSection({}, { code })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const codeDiffs = failures.filter((f) => f.category === FailureCategory.CODE_DIFF);
    expect(codeDiffs).toHaveLength(0);
  });

  it("returns no code_diff when both code blocks are empty", () => {
    const swcSections = [makeSyntheticSection({}, { code: "" })];
    const oxcSections = [makeSyntheticSection({}, { code: "" })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const codeDiffs = failures.filter((f) => f.category === FailureCategory.CODE_DIFF);
    expect(codeDiffs).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// COMP-06: Typed failure categories
// ---------------------------------------------------------------------------

describe("compareFixture — COMP-06: all failures carry typed FailureCategory values", () => {
  it("every failure in returned array has a category from FailureCategory enum", () => {
    const validCategories = new Set(Object.values(FailureCategory));

    const swcSections = [
      makeSyntheticSection({
        hash: "abc12345678",
        displayName: "oldName",
        loc: [0, 100],
      }),
    ];
    const oxcSections = [
      makeSyntheticSection({
        hash: "zzz99999999",
        displayName: "newName",
        loc: [0, 200],
      }),
    ];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    expect(failures.length).toBeGreaterThan(0);
    for (const failure of failures) {
      expect(validCategories.has(failure.category)).toBe(true);
    }
  });

  it("structural mismatch failures (MISSING_SEGMENT/EXTRA_SEGMENT) carry typed categories", () => {
    const swcSnap = makeSnapshot([makeSyntheticSection(), makeSyntheticSection()]);
    const oxcSnap = makeSnapshot([makeSyntheticSection()]);
    const validCategories = new Set(Object.values(FailureCategory));

    const failures = compareFixture(swcSnap, oxcSnap);

    expect(failures.length).toBeGreaterThan(0);
    for (const failure of failures) {
      expect(validCategories.has(failure.category)).toBe(true);
    }
  });
});

// ---------------------------------------------------------------------------
// COMP-07: loc comparison — wrong_loc, never suppressed
// ---------------------------------------------------------------------------

describe("compareFixture — COMP-07: loc reported as WRONG_LOC", () => {
  it("returns wrong_loc when loc differs between SWC [0,100] and OXC [0,200]", () => {
    const swcSections = [makeSyntheticSection({ loc: [0, 100] })];
    const oxcSections = [makeSyntheticSection({ loc: [0, 200] })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const locFailures = failures.filter((f) => f.category === FailureCategory.WRONG_LOC);
    expect(locFailures).toHaveLength(1);
    expect(locFailures[0].expected).toEqual([0, 100]);
    expect(locFailures[0].actual).toEqual([0, 200]);
  });

  it("loc failures appear alongside other metadata failures (not suppressed)", () => {
    const swcSections = [
      makeSyntheticSection({ loc: [0, 100], hash: "abc12345678" }),
    ];
    const oxcSections = [
      makeSyntheticSection({ loc: [0, 200], hash: "zzz99999999" }),
    ];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const locFailures = failures.filter((f) => f.category === FailureCategory.WRONG_LOC);
    const hashFailures = failures.filter((f) => f.category === FailureCategory.HASH_MISMATCH);
    // Both must appear — loc does not suppress hash, and vice versa
    expect(locFailures).toHaveLength(1);
    expect(hashFailures).toHaveLength(1);
  });

  it("does not return wrong_loc when loc matches", () => {
    const swcSections = [makeSyntheticSection({ loc: [0, 100] })];
    const oxcSections = [makeSyntheticSection({ loc: [0, 100] })];

    const failures = compareFixture(makeSnapshot(swcSections), makeSnapshot(oxcSections));

    const locFailures = failures.filter((f) => f.category === FailureCategory.WRONG_LOC);
    expect(locFailures).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Self-comparison: identical SWC snapshots on both sides → zero failures
// ---------------------------------------------------------------------------

describe("compareFixture — self-comparison returns zero failures", () => {
  it("returns zero failures for identical single-segment snapshots", () => {
    const section = makeSyntheticSection(
      { hash: "abc12345678", loc: [10, 200] },
      { code: "const x = 1;" }
    );
    const snap = makeSnapshot([section]);

    const failures = compareFixture(snap, snap);

    expect(failures).toHaveLength(0);
  });

  it("returns zero failures for multi-segment identical snapshots", () => {
    const s1 = makeSyntheticSection({ ctxName: "component$", loc: [0, 100] }, { code: "const a = 1;" });
    const s2 = makeSyntheticSection({ ctxName: "useTask$", loc: [101, 200] }, { code: "const b = 2;" });
    const snap = makeSnapshot([s1, s2]);

    const failures = compareFixture(snap, snap);

    expect(failures).toHaveLength(0);
  });

  it("returns zero failures for empty segment list", () => {
    const snap = makeSnapshot([]);

    const failures = compareFixture(snap, snap);

    expect(failures).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// COMP-04: Parent module comparison
// ---------------------------------------------------------------------------

function makeParentSection(code: string): ParsedSection {
  return {
    headerName: "test.tsx",
    isEntryPoint: false,
    code,
    sourceMap: null,
    metadata: null, // null = parent section
  };
}

describe("compareFixture — COMP-04: parent module comparison", () => {
  it("returns code_diff with field=parentModule when parent code differs", () => {
    const swcParent = makeParentSection("const x = 1;");
    const oxcParent = makeParentSection("const y = 1;");

    const swcSnap = makeSnapshot([swcParent]);
    const oxcSnap = makeSnapshot([oxcParent]);

    const failures = compareFixture(swcSnap, oxcSnap);

    const parentFailures = failures.filter(
      (f) => f.category === FailureCategory.CODE_DIFF && f.field === "parentModule"
    );
    expect(parentFailures).toHaveLength(1);
  });

  it("returns no parent failure when both parent modules are identical", () => {
    const code = "const x = 1;";
    const swcParent = makeParentSection(code);
    const oxcParent = makeParentSection(code);

    const swcSnap = makeSnapshot([swcParent]);
    const oxcSnap = makeSnapshot([oxcParent]);

    const failures = compareFixture(swcSnap, oxcSnap);

    const parentFailures = failures.filter(
      (f) => f.category === FailureCategory.CODE_DIFF && f.field === "parentModule"
    );
    expect(parentFailures).toHaveLength(0);
  });

  it("returns code_diff with field=parentModule when SWC has parent section but OXC does not", () => {
    const swcParent = makeParentSection("const x = 1;");

    const swcSnap = makeSnapshot([swcParent]);
    const oxcSnap = makeSnapshot([]); // no sections at all

    const failures = compareFixture(swcSnap, oxcSnap);

    const parentFailures = failures.filter(
      (f) => f.category === FailureCategory.CODE_DIFF && f.field === "parentModule"
    );
    expect(parentFailures).toHaveLength(1);
  });

  it("returns code_diff with field=parentModule when OXC has parent section but SWC does not", () => {
    const oxcParent = makeParentSection("const x = 1;");

    const swcSnap = makeSnapshot([]); // no sections at all
    const oxcSnap = makeSnapshot([oxcParent]);

    const failures = compareFixture(swcSnap, oxcSnap);

    const parentFailures = failures.filter(
      (f) => f.category === FailureCategory.CODE_DIFF && f.field === "parentModule"
    );
    expect(parentFailures).toHaveLength(1);
  });

  it("returns no parent failure when neither side has a parent section", () => {
    const swcSnap = makeSnapshot([]);
    const oxcSnap = makeSnapshot([]);

    const failures = compareFixture(swcSnap, oxcSnap);

    const parentFailures = failures.filter(
      (f) => f.category === FailureCategory.CODE_DIFF && f.field === "parentModule"
    );
    expect(parentFailures).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// COMP-05: Diagnostics comparison — structural, sorted
// ---------------------------------------------------------------------------

describe("compareFixture — COMP-05: diagnostics comparison", () => {
  it("returns diagnostics_mismatch when message differs", () => {
    const swcSnap: ParsedSnapshot = {
      ...makeSnapshot([]),
      diagnostics: [{ category: "Error", message: "foo" }],
    };
    const oxcSnap: ParsedSnapshot = {
      ...makeSnapshot([]),
      diagnostics: [{ category: "Error", message: "bar" }],
    };

    const failures = compareFixture(swcSnap, oxcSnap);

    const diagFailures = failures.filter(
      (f) => f.category === FailureCategory.DIAGNOSTICS_MISMATCH
    );
    expect(diagFailures).toHaveLength(1);
  });

  it("returns diagnostics_mismatch when counts differ (SWC=2, OXC=1)", () => {
    const swcSnap: ParsedSnapshot = {
      ...makeSnapshot([]),
      diagnostics: [
        { category: "Error", message: "foo" },
        { category: "Error", message: "bar" },
      ],
    };
    const oxcSnap: ParsedSnapshot = {
      ...makeSnapshot([]),
      diagnostics: [{ category: "Error", message: "foo" }],
    };

    const failures = compareFixture(swcSnap, oxcSnap);

    const diagFailures = failures.filter(
      (f) => f.category === FailureCategory.DIAGNOSTICS_MISMATCH
    );
    expect(diagFailures).toHaveLength(1);
  });

  it("returns no diagnostics_mismatch when diagnostics are identical", () => {
    const diag = [{ category: "Error", message: "foo", file: "test.tsx", loc: [10, 5] }];
    const swcSnap: ParsedSnapshot = { ...makeSnapshot([]), diagnostics: diag };
    const oxcSnap: ParsedSnapshot = { ...makeSnapshot([]), diagnostics: diag };

    const failures = compareFixture(swcSnap, oxcSnap);

    const diagFailures = failures.filter(
      (f) => f.category === FailureCategory.DIAGNOSTICS_MISMATCH
    );
    expect(diagFailures).toHaveLength(0);
  });

  it("sorts diagnostics by DIAGNOSTICS_SORT_KEY before comparing (order-independent)", () => {
    // Two diagnostics in different order — should still match
    const diag1 = { category: "Error", message: "first", file: "a.tsx", loc: [1, 0] };
    const diag2 = { category: "Error", message: "second", file: "b.tsx", loc: [2, 0] };

    const swcSnap: ParsedSnapshot = {
      ...makeSnapshot([]),
      diagnostics: [diag1, diag2],
    };
    const oxcSnap: ParsedSnapshot = {
      ...makeSnapshot([]),
      diagnostics: [diag2, diag1], // reversed order
    };

    const failures = compareFixture(swcSnap, oxcSnap);

    const diagFailures = failures.filter(
      (f) => f.category === FailureCategory.DIAGNOSTICS_MISMATCH
    );
    expect(diagFailures).toHaveLength(0);
  });

  it("returns no diagnostics_mismatch when both diagnostic arrays are empty", () => {
    const swcSnap: ParsedSnapshot = { ...makeSnapshot([]), diagnostics: [] };
    const oxcSnap: ParsedSnapshot = { ...makeSnapshot([]), diagnostics: [] };

    const failures = compareFixture(swcSnap, oxcSnap);

    const diagFailures = failures.filter(
      (f) => f.category === FailureCategory.DIAGNOSTICS_MISMATCH
    );
    expect(diagFailures).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Self-comparison smoke test: real SWC snap files produce zero failures
// ---------------------------------------------------------------------------

const SWC_SNAPSHOTS_DIR = path.resolve(
  new URL("..", import.meta.url).pathname,
  "swc-snapshots"
);

describe("compareFixture — real SWC snapshot self-comparison (smoke test)", () => {
  const REPRESENTATIVE_FIXTURES = [
    "example_1",
    "component_level_self_referential_qrl",
    "destructure_args_colon_props",
  ];

  for (const fixtureName of REPRESENTATIVE_FIXTURES) {
    it(`returns zero failures when comparing ${fixtureName} against itself`, () => {
      const snapPath = path.join(SWC_SNAPSHOTS_DIR, `${fixtureName}.snap`);
      const parsed = parseSnapFile(snapPath);

      const failures = compareFixture(parsed, parsed);

      expect(failures).toHaveLength(0);
    });
  }
});
