/**
 * Tests for src/identity.ts — spec-derived identity recomputation functions.
 *
 * Covers:
 *   - escapeSymbol: non-alphanumeric → underscore, squash, trim
 *   - decomposeDisplayName: extract file prefix and pre-prefix
 *   - recomputeHash: SipHash-1-3 with seed (0,0) over relPath + prePrefix
 *   - validateDisplayName: structural check on stored displayName
 *   - recomputeCanonicalFilename: {displayName}_{hashSuffix}
 *   - validateCanonicalFilename: compare stored vs recomputed
 */

import { describe, it, expect } from "bun:test";
import {
  escapeSymbol,
  decomposeDisplayName,
  recomputeHash,
  validateDisplayName,
  recomputeCanonicalFilename,
  validateCanonicalFilename,
} from "../src/identity.js";
import type { SegmentMetadata } from "../src/types.js";

// ---------------------------------------------------------------------------
// escapeSymbol
// ---------------------------------------------------------------------------

describe("escapeSymbol", () => {
  it("replaces non-alphanumeric chars with underscores", () => {
    expect(escapeSymbol("my-component.handler")).toBe("my_component_handler");
  });

  it("squashes consecutive underscores", () => {
    expect(escapeSymbol("my--double-dash")).toBe("my_double_dash");
  });

  it("trims leading underscores from dashes", () => {
    expect(escapeSymbol("---foo")).toBe("foo");
  });

  it("trims leading underscores", () => {
    expect(escapeSymbol("__bar")).toBe("bar");
  });

  it("does NOT alter digit prefix — caller adds underscore if needed", () => {
    expect(escapeSymbol("123click")).toBe("123click");
  });

  it("handles already-clean identifiers unchanged", () => {
    expect(escapeSymbol("renderHeader1")).toBe("renderHeader1");
  });

  it("handles dot extension", () => {
    // test.tsx → test_tsx
    expect(escapeSymbol("test.tsx")).toBe("test_tsx");
  });
});

// ---------------------------------------------------------------------------
// decomposeDisplayName
// ---------------------------------------------------------------------------

describe("decomposeDisplayName", () => {
  it("extracts file prefix and pre-prefix for typical segment", () => {
    const result = decomposeDisplayName(
      "test.tsx_renderHeader1_div_onClick",
      "test.tsx"
    );
    expect(result).not.toBeNull();
    expect(result!.fileNamePrefix).toBe("test.tsx");
    expect(result!.prePrefix).toBe("renderHeader1_div_onClick");
  });

  it("extracts for nested path origin", () => {
    const result = decomposeDisplayName(
      "apps.tsx_Greeter_component",
      "components/apps/apps.tsx"
    );
    expect(result).not.toBeNull();
    expect(result!.fileNamePrefix).toBe("apps.tsx");
    expect(result!.prePrefix).toBe("Greeter_component");
  });

  it("returns null when displayName does not start with expected prefix", () => {
    const result = decomposeDisplayName("wrong_prefix", "test.tsx");
    expect(result).toBeNull();
  });

  it("uses raw filename (not escape_sym'd) as prefix", () => {
    // test.tsx prefix in displayName is 'test.tsx_' not 'test_tsx_'
    const result = decomposeDisplayName(
      "test.tsx_renderHeader1",
      "test.tsx"
    );
    expect(result).not.toBeNull();
    expect(result!.fileNamePrefix).toBe("test.tsx");
    expect(result!.prePrefix).toBe("renderHeader1");
  });
});

// ---------------------------------------------------------------------------
// recomputeHash — Known SWC reference vectors
// ---------------------------------------------------------------------------

describe("recomputeHash", () => {
  it("matches SWC reference vector 1 (renderHeader1_div_onClick)", () => {
    expect(
      recomputeHash(null, "test.tsx", "renderHeader1_div_onClick")
    ).toBe("USi8k1jUb40");
  });

  it("matches SWC reference vector 2 (renderHeader1)", () => {
    expect(recomputeHash(null, "test.tsx", "renderHeader1")).toBe(
      "jMxQsjbyDss"
    );
  });

  it("matches SWC reference vector 3 (Greeter_component, nested path)", () => {
    expect(
      recomputeHash(null, "components/apps/apps.tsx", "Greeter_component")
    ).toBe("0jjOvx068y0");
  });

  it("produces 11-char string", () => {
    const h = recomputeHash(null, "test.tsx", "renderHeader1");
    expect(h).toHaveLength(11);
  });

  it("normalizes backslashes in relPath (Windows paths)", () => {
    // Same result whether forward or back slashes
    expect(
      recomputeHash(null, "components\\apps\\apps.tsx", "Greeter_component")
    ).toBe("0jjOvx068y0");
  });
});

// ---------------------------------------------------------------------------
// recomputeCanonicalFilename
// ---------------------------------------------------------------------------

describe("recomputeCanonicalFilename", () => {
  it("returns displayName + last _ token from symbolName", () => {
    expect(
      recomputeCanonicalFilename(
        "test.tsx_renderHeader1_div_onClick",
        "renderHeader1_div_onClick_USi8k1jUb40"
      )
    ).toBe("test.tsx_renderHeader1_div_onClick_USi8k1jUb40");
  });
});

// ---------------------------------------------------------------------------
// validateDisplayName
// ---------------------------------------------------------------------------

function makeMetadata(overrides: Partial<SegmentMetadata>): SegmentMetadata {
  return {
    origin: "test.tsx",
    name: "renderHeader1_div_onClick_USi8k1jUb40",
    entry: null,
    displayName: "test.tsx_renderHeader1_div_onClick",
    hash: "USi8k1jUb40",
    canonicalFilename: "test.tsx_renderHeader1_div_onClick_USi8k1jUb40",
    path: "",
    extension: "tsx",
    parent: null,
    ctxKind: "function",
    ctxName: "$",
    captures: false,
    loc: [0, 100],
    ...overrides,
  };
}

describe("validateDisplayName", () => {
  it("returns null for valid example_1 segment", () => {
    const metadata = makeMetadata({});
    expect(validateDisplayName(metadata, metadata.origin)).toBeNull();
  });

  it("returns null for support_windows_paths style segment", () => {
    const metadata = makeMetadata({
      origin: "components/apps/apps.tsx",
      displayName: "apps.tsx_Greeter_component",
      hash: "0jjOvx068y0",
      name: "Greeter_component_0jjOvx068y0",
      canonicalFilename: "apps.tsx_Greeter_component_0jjOvx068y0",
    });
    expect(validateDisplayName(metadata, metadata.origin)).toBeNull();
  });

  it("returns error string when displayName has wrong file prefix", () => {
    const metadata = makeMetadata({
      displayName: "wrong.tsx_renderHeader1_div_onClick",
    });
    const result = validateDisplayName(metadata, metadata.origin);
    expect(typeof result).toBe("string");
    expect(result).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// validateCanonicalFilename
// ---------------------------------------------------------------------------

describe("validateCanonicalFilename", () => {
  it("returns null for valid example_1 segment", () => {
    const metadata = makeMetadata({});
    expect(validateCanonicalFilename(metadata)).toBeNull();
  });

  it("returns error string when canonicalFilename is tampered", () => {
    const metadata = makeMetadata({
      canonicalFilename: "test.tsx_renderHeader1_div_onClick_WRONGHASH1",
    });
    const result = validateCanonicalFilename(metadata);
    expect(typeof result).toBe("string");
    expect(result).not.toBeNull();
  });

  it("returns null for nested path segment", () => {
    const metadata = makeMetadata({
      origin: "components/apps/apps.tsx",
      displayName: "apps.tsx_Greeter_component",
      name: "Greeter_component_0jjOvx068y0",
      canonicalFilename: "apps.tsx_Greeter_component_0jjOvx068y0",
      hash: "0jjOvx068y0",
    });
    expect(validateCanonicalFilename(metadata)).toBeNull();
  });
});
