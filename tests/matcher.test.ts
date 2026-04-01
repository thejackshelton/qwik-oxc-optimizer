import { describe, it, expect } from "bun:test";
import { matchSegments } from "../src/matcher.ts";
import type { ParsedSection, SegmentMetadata } from "../src/types.ts";

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
    headerName: "test_section",
    isEntryPoint: false,
    code: "",
    sourceMap: null,
    metadata: makeMeta(overrides),
    ...sectionOverrides,
  };
}

function makeParentSection(): ParsedSection {
  return {
    headerName: "parent_section",
    isEntryPoint: false,
    code: "export const Parent = () => <div/>;",
    sourceMap: null,
    metadata: null,
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("matchSegments", () => {
  it("pairs equal-count segments by order with high confidence", () => {
    const swc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [60, 120] }),
      makeSyntheticSection({ ctxName: "useSignal$", loc: [130, 200] }),
    ];
    const oxc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [60, 120] }),
      makeSyntheticSection({ ctxName: "useSignal$", loc: [130, 200] }),
    ];

    const result = matchSegments(swc, oxc);

    expect(result.matches).toHaveLength(3);
    expect(result.matches.every((m: { confidence: string }) => m.confidence === "high")).toBe(true);
    expect(result.unmatched_swc).toHaveLength(0);
    expect(result.unmatched_oxc).toHaveLength(0);
    expect(result.ambiguous).toBe(false);
  });

  it("reports low confidence when ctxName differs", () => {
    const swc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [60, 120] }),
    ];
    const oxc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
      // Second segment has different ctxName
      makeSyntheticSection({ ctxName: "useResource$", loc: [60, 120] }),
    ];

    const result = matchSegments(swc, oxc);

    expect(result.matches).toHaveLength(2);
    expect(result.matches[0].confidence).toBe("high");
    expect(result.matches[1].confidence).toBe("low");
    expect(result.ambiguous).toBe(true);
  });

  it("reports low confidence when loc differs", () => {
    const swc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [60, 120] }),
    ];
    const oxc = [
      // First segment has different loc[0]
      makeSyntheticSection({ ctxName: "component$", loc: [99, 50] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [60, 120] }),
    ];

    const result = matchSegments(swc, oxc);

    expect(result.matches).toHaveLength(2);
    expect(result.matches[0].confidence).toBe("low");
    expect(result.matches[1].confidence).toBe("high");
    expect(result.ambiguous).toBe(true);
  });

  it("handles SWC having more segments", () => {
    const swc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [60, 120] }),
      makeSyntheticSection({ ctxName: "useSignal$", loc: [130, 200] }),
    ];
    const oxc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
    ];

    const result = matchSegments(swc, oxc);

    expect(result.matches).toHaveLength(1);
    expect(result.unmatched_swc).toHaveLength(2);
    expect(result.unmatched_oxc).toHaveLength(0);
    expect(result.ambiguous).toBe(true);
  });

  it("handles OXC having more segments", () => {
    const swc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
    ];
    const oxc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
      makeSyntheticSection({ ctxName: "useTask$", loc: [60, 120] }),
      makeSyntheticSection({ ctxName: "useSignal$", loc: [130, 200] }),
    ];

    const result = matchSegments(swc, oxc);

    expect(result.matches).toHaveLength(1);
    expect(result.unmatched_swc).toHaveLength(0);
    expect(result.unmatched_oxc).toHaveLength(2);
    expect(result.ambiguous).toBe(true);
  });

  it("reports low confidence when loc[1] (end offset) differs", () => {
    const swc = [
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
    ];
    const oxc = [
      // Same loc[0] but different loc[1]
      makeSyntheticSection({ ctxName: "component$", loc: [10, 999] }),
    ];

    const result = matchSegments(swc, oxc);

    expect(result.matches).toHaveLength(1);
    expect(result.matches[0].confidence).toBe("low");
    expect(result.matches[0].reason).toContain("loc[1]");
    expect(result.ambiguous).toBe(true);
  });

  it("handles both empty", () => {
    const result = matchSegments([], []);

    expect(result.matches).toHaveLength(0);
    expect(result.unmatched_swc).toHaveLength(0);
    expect(result.unmatched_oxc).toHaveLength(0);
    expect(result.ambiguous).toBe(false);
  });

  it("filters out parent sections (metadata=null)", () => {
    const swc = [
      makeParentSection(),
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
      makeParentSection(),
    ];
    const oxc = [
      makeParentSection(),
      makeSyntheticSection({ ctxName: "component$", loc: [10, 50] }),
    ];

    const result = matchSegments(swc, oxc);

    // Only the metadata-bearing sections count
    expect(result.matches).toHaveLength(1);
    expect(result.matches[0].confidence).toBe("high");
    expect(result.unmatched_swc).toHaveLength(0);
    expect(result.unmatched_oxc).toHaveLength(0);
    expect(result.ambiguous).toBe(false);
  });

  it("does not use derived fields for matching — high confidence when only derived fields differ", () => {
    // The two pairs have completely different derived fields but matching ctxName + loc.
    // Matcher must return high confidence since it must not consider derived fields.
    const swc = [
      makeSyntheticSection({
        displayName: "SwcDisplay",
        hash: "swchash1111",
        canonicalFilename: "SwcDisplay_swchash1111",
        name: "SwcDisplay_swchash1111",
        parent: "SwcParent_xxxxxxxxxxx",
        ctxName: "component$",
        loc: [0, 100],
      }),
    ];
    const oxc = [
      makeSyntheticSection({
        displayName: "OxcDisplay",
        hash: "oxchash9999",
        canonicalFilename: "OxcDisplay_oxchash9999",
        name: "OxcDisplay_oxchash9999",
        parent: "OxcParent_yyyyyyyyyyy",
        ctxName: "component$",
        loc: [0, 100],
      }),
    ];

    const result = matchSegments(swc, oxc);

    expect(result.matches).toHaveLength(1);
    expect(result.matches[0].confidence).toBe("high");
    expect(result.ambiguous).toBe(false);
  });
});
