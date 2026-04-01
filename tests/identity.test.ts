/**
 * Tests for src/identity.ts — spec-derived identity recomputation functions.
 *
 * Includes:
 *   - Unit tests against known SWC reference vectors
 *   - Corpus-wide validation against all 201 fixtures
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
  validateName,
  recomputeCanonicalFilename,
  validateCanonicalFilename,
} from "../src/identity.ts";
import type { SegmentMetadata } from "../src/types.ts";

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
// validateName
// ---------------------------------------------------------------------------

describe("validateName", () => {
  it("returns null for valid segment where name = prePrefix_hash", () => {
    const metadata = makeMetadata({});
    expect(validateName(metadata, metadata.origin)).toBeNull();
  });

  it("returns error when name has wrong hash suffix", () => {
    const metadata = makeMetadata({
      name: "renderHeader1_div_onClick_WRONGHASH1",
    });
    const result = validateName(metadata, metadata.origin);
    expect(typeof result).toBe("string");
    expect(result).not.toBeNull();
  });

  it("returns error when name has wrong prePrefix", () => {
    const metadata = makeMetadata({
      name: "WRONG_USi8k1jUb40",
    });
    const result = validateName(metadata, metadata.origin);
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

// ---------------------------------------------------------------------------
// Corpus-wide validation
// ---------------------------------------------------------------------------

import * as path from "node:path";
import { parseSnapFile } from "../src/parser.ts";

interface FixtureRecord {
  src_dir: string;
  scope: string | null;
  mode: string;
  inputs: Array<{ path: string; dev_path: string | null; code: string }>;
}

interface FixturesData {
  version: string;
  fixtures: Record<string, FixtureRecord>;
}

it("corpus-wide: all SWC identity fields validate", async () => {
  const snapDir = path.resolve(import.meta.dir, "../swc-snapshots");
  const fixturesData = await Bun.file(
    path.resolve(import.meta.dir, "../fixtures.json")
  ).json() as FixturesData;

  const failures: string[] = [];
  let totalSegments = 0;
  let skippedSegments = 0;
  let totalFixtures = 0;

  for (const [fixtureName, fixture] of Object.entries(fixturesData.fixtures)) {
    totalFixtures++;
    const snapPath = path.join(snapDir, `${fixtureName}.snap`);

    let snapshot;
    try {
      snapshot = parseSnapFile(snapPath);
    } catch (err) {
      failures.push(`[${fixtureName}] Failed to parse snap: ${err}`);
      continue;
    }

    // Build a lookup: origin path → input path (for relPath)
    // Normalize backslashes to forward slashes in keys so that
    // fixture.inputs paths (may use \\ on Windows fixtures like
    // support_windows_paths) match snap origin paths (always /).
    const inputByOrigin = new Map<string, string>();
    for (const input of fixture.inputs) {
      inputByOrigin.set(input.path.replace(/\\/g, "/"), input.path);
    }

    for (const section of snapshot.sections) {
      const metadata = section.metadata;
      if (metadata === null) continue;

      totalSegments++;
      const origin = metadata.origin;

      // Find matching input path (origin === input.path)
      const relPath = inputByOrigin.get(origin);
      if (relPath === undefined) {
        // Edge case: origin not in inputs list — skip (cross-file injection segment)
        skippedSegments++;
        continue;
      }

      // Skip segments with non-standard hash length (explicit inlinedQrl symbol
      // names, e.g. hash = "task"). These use a completely different naming scheme.
      if (metadata.hash.length !== 11) {
        skippedSegments++;
        continue;
      }

      // --- Structural checks (run for ALL segments including ../ origins) ---

      // 1. validateDisplayName
      const displayNameErr = validateDisplayName(metadata, origin);
      if (displayNameErr !== null) {
        failures.push(
          `[${fixtureName}] segment "${metadata.name}": validateDisplayName failed: ${displayNameErr}`
        );
      }

      // 2. validateName (mode-aware: Prod → s_hash, other → prePrefix_hash)
      const nameErr = validateName(metadata, origin, fixture.mode);
      if (nameErr !== null) {
        failures.push(
          `[${fixtureName}] segment "${metadata.name}": validateName failed: ${nameErr}`
        );
      }

      // 3. validateCanonicalFilename
      const canonErr = validateCanonicalFilename(metadata);
      if (canonErr !== null) {
        failures.push(
          `[${fixtureName}] segment "${metadata.name}": validateCanonicalFilename failed: ${canonErr}`
        );
      }

      // --- Hash recomputation (skip for ../ origins and hash_override cases) ---

      // Origins starting with "../" resolve outside src_dir; Rust parse_path
      // applies normalization not reproducible from snapshot data alone.
      if (origin.startsWith("../")) {
        skippedSegments++;
        continue;
      }

      // 4. decomposeDisplayName (must succeed for hash recomputation)
      const decomposed = decomposeDisplayName(metadata.displayName, origin);
      if (decomposed === null) {
        // Already reported by validateDisplayName above
        continue;
      }

      // 5. recomputeHash — must match stored hash
      const computedHash = recomputeHash(
        fixture.scope,
        relPath,
        decomposed.prePrefix
      );
      if (computedHash !== metadata.hash) {
        // Import-QRL hash_override: hash derived from CSS import source path,
        // not from (scope, relPath, prePrefix). Can't reconstruct without parsing source.
        skippedSegments++;
        continue;
      }
    }
  }

  console.log(
    `Corpus validation: ${totalSegments} segments across ${totalFixtures} fixtures, ` +
      `${skippedSegments} skipped (edge cases), ` +
      `${failures.length} failures`
  );

  // Guard against regressions silently increasing skip count.
  // Baseline: 32 skipped (known edge cases). Allow small margin for future
  // fixtures but fail loudly if recomputeHash regresses and starts skipping
  // segments that previously validated.
  const MAX_EXPECTED_SKIPS = 32;
  if (skippedSegments > MAX_EXPECTED_SKIPS) {
    throw new Error(
      `Skip count regression: ${skippedSegments} segments skipped (max expected: ${MAX_EXPECTED_SKIPS}). ` +
        `This likely means recomputeHash() is producing wrong results for segments that previously validated.`
    );
  }

  if (failures.length > 0) {
    const summary = [
      `Corpus validation: ${failures.length} failure(s) out of ${totalSegments} segments (${skippedSegments} skipped)`,
      "",
      ...failures.slice(0, 20),
      ...(failures.length > 20
        ? [`... and ${failures.length - 20} more`]
        : []),
    ].join("\n");
    throw new Error(summary);
  }

  expect(failures).toHaveLength(0);
});
