/**
 * Unit tests for src/normalizer.ts
 *
 * Covers:
 *   NORM-01 — Parser already strips source maps (normalizer does NOT re-strip)
 *   NORM-02 — normalizeCode formats code through oxfmt 0.32.0
 *   NORM-03 — Normalization is idempotent
 *   NORM-04 — Single normalizeCode function, no side-specific variants
 *   NORM-05 — loc metadata fields are untouched by normalization
 *
 * Run: bun test tests/normalizer.test.ts
 */

import { test, describe, expect } from "bun:test";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { parseSnapFile } from "../src/parser.ts";
import { normalizeCode, assertOxfmtVersion, assertNormalizerIdempotent } from "../src/normalizer.ts";

// Resolve project root relative to this test file
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");

/** Helper: absolute path to a snap file */
function snapPath(name: string) {
  return path.join(projectRoot, "swc-snapshots", name);
}

// ---------------------------------------------------------------------------
// NORM-02: normalizeCode formats code through oxfmt 0.32.0
// ---------------------------------------------------------------------------

describe("NORM-02: normalizeCode formats via oxfmt", () => {
  test("assertOxfmtVersion() does not throw when oxfmt 0.32.0 is installed", () => {
    expect(() => assertOxfmtVersion()).not.toThrow();
  });

  test("normalizeCode formats known unformatted TypeScript (spaces around =)", () => {
    const input = "const x=1;";
    const result = normalizeCode(input, "test.ts");
    // oxfmt should add space around = in assignments
    expect(result).toContain("const x = 1");
  });

  test("normalizeCode with .tsx extension handles JSX without parse errors", () => {
    const jsx = 'export const A = () => <div onClick={handler}/>;';
    expect(() => normalizeCode(jsx, "test.tsx")).not.toThrow();
  });

  test("normalizeCode with .tsx extension returns a non-empty string for JSX", () => {
    const jsx = 'export const A = () => <div onClick={handler}/>;';
    const result = normalizeCode(jsx, "test.tsx");
    expect(typeof result).toBe("string");
    expect(result.length > 0).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// NORM-03: Normalization is idempotent
// ---------------------------------------------------------------------------

describe("NORM-03: Idempotency", () => {
  test("assertNormalizerIdempotent() does not throw", () => {
    expect(() => assertNormalizerIdempotent()).not.toThrow();
  });

  test("double-format produces identical output for a simple const", () => {
    const code = "const x=1;";
    const once = normalizeCode(code, "test.ts");
    const twice = normalizeCode(once, "test.ts");
    expect(twice).toBe(once);
  });

  test("double-format produces identical output for an arrow function", () => {
    const code = "const fn = (a:string,b:number)=>a+b;";
    const once = normalizeCode(code, "test.ts");
    const twice = normalizeCode(once, "test.ts");
    expect(twice).toBe(once);
  });

  test("double-format produces identical output for a JSX component", () => {
    const code = "export const Cmp = (props:{name:string}) => <div>{props.name}</div>;";
    const once = normalizeCode(code, "test.tsx");
    const twice = normalizeCode(once, "test.tsx");
    expect(twice).toBe(once);
  });
});

// ---------------------------------------------------------------------------
// NORM-01: Parser already strips source maps from code
// ---------------------------------------------------------------------------

describe("NORM-01: Parser strips source maps before normalizer sees code", () => {
  test("example_1.snap: a section with sourceMap !== null exists", () => {
    const snap = parseSnapFile(snapPath("example_1.snap"));
    const withSourceMap = snap.sections.find(s => s.sourceMap !== null);
    expect(withSourceMap).toBeTruthy();
  });

  test("example_1.snap: section.code does NOT contain 'Some(\"' (source map line stripped)", () => {
    const snap = parseSnapFile(snapPath("example_1.snap"));
    const withSourceMap = snap.sections.find(s => s.sourceMap !== null);
    expect(withSourceMap).toBeTruthy();
    expect(withSourceMap!.code).not.toContain('Some("');
  });

  test("example_1.snap: section.sourceMap is a non-empty string for a source-map section", () => {
    const snap = parseSnapFile(snapPath("example_1.snap"));
    const withSourceMap = snap.sections.find(s => s.sourceMap !== null);
    expect(withSourceMap).toBeTruthy();
    expect(typeof withSourceMap!.sourceMap).toBe("string");
    expect(withSourceMap!.sourceMap!.length > 0).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// NORM-05: loc metadata is untouched by normalization
// ---------------------------------------------------------------------------

describe("NORM-05: loc metadata is unchanged after normalization", () => {
  test("metadata.loc values are unchanged after normalizing section.code", () => {
    const snap = parseSnapFile(snapPath("example_1.snap"));
    const segment = snap.sections.find(s => s.metadata !== null);
    expect(segment).toBeTruthy();

    const locBefore = [...segment!.metadata!.loc] as [number, number];
    // Normalize the code — this should NOT affect metadata
    normalizeCode(segment!.code, "test.tsx");

    expect(segment!.metadata!.loc[0]).toBe(locBefore[0]);
    expect(segment!.metadata!.loc[1]).toBe(locBefore[1]);
  });

  test("metadata object reference is the same after normalization (no mutation)", () => {
    const snap = parseSnapFile(snapPath("example_1.snap"));
    const segment = snap.sections.find(s => s.metadata !== null);
    expect(segment).toBeTruthy();

    const metaBefore = segment!.metadata;
    normalizeCode(segment!.code, "test.tsx");
    // metadata reference is unchanged (normalizer never touches metadata)
    expect(segment!.metadata).toBe(metaBefore);
  });
});

// ---------------------------------------------------------------------------
// Empty/whitespace short-circuit (no oxfmt invocation)
// ---------------------------------------------------------------------------

describe("Empty/whitespace code short-circuits oxfmt", () => {
  test("normalizeCode(\"\", \"test.ts\") returns empty string", () => {
    expect(normalizeCode("", "test.ts")).toBe("");
  });

  test("normalizeCode(\"  \\n  \", \"test.ts\") returns the input unchanged", () => {
    const ws = "  \n  ";
    expect(normalizeCode(ws, "test.ts")).toBe(ws);
  });
});

// ---------------------------------------------------------------------------
// NORM-04: Single normalizeCode function, no side-specific variants
// ---------------------------------------------------------------------------

describe("NORM-04: Single symmetric normalizeCode export", () => {
  test("normalizer module exports exactly 4 named exports", async () => {
    const normalizer = await import("../src/normalizer.ts");
    const exportedKeys = Object.keys(normalizer).sort();
    expect(exportedKeys).toEqual([
      "assertNormalizerIdempotent",
      "assertOxfmtVersion",
      "normalizeCode",
      "warmNormalizationCache",
    ]);
  });

  test("no 'normalizeSWC' or 'normalizeOXC' variant exists", async () => {
    const normalizer = await import("../src/normalizer.ts");
    expect((normalizer as Record<string, unknown>)["normalizeSWC"]).toBeUndefined();
    expect((normalizer as Record<string, unknown>)["normalizeOXC"]).toBeUndefined();
  });
});
