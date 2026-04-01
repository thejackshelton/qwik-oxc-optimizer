/**
 * Roundtrip validation test: confirms all 201 .snap files parse without errors.
 * Covers PARSE-04 requirement.
 *
 * Run: bun test tests/roundtrip.test.ts
 */

import { test, describe, expect } from "bun:test";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { parseSnapFile } from "../src/parser.ts";
import { CORPUS_SIZE } from "../src/contract.ts";

const ROOT = path.resolve(fileURLToPath(import.meta.url), "../../");
const SNAP_DIR = path.join(ROOT, "swc-snapshots");

const snapFiles = fs
  .readdirSync(SNAP_DIR)
  .filter((f) => f.endsWith(".snap"))
  .sort();

// Sanity: corpus size must match the frozen contract constant
describe("PARSE-04: corpus roundtrip", () => {
  test(`corpus contains ${CORPUS_SIZE} snap files`, () => {
    expect(snapFiles.length).toBe(CORPUS_SIZE);
  });

  for (const file of snapFiles) {
    const fixtureName = path.basename(file, ".snap");

    test(`[${fixtureName}] parses without error`, () => {
      const fullPath = path.join(SNAP_DIR, file);
      let result: ReturnType<typeof parseSnapFile>;
      expect(() => {
        result = parseSnapFile(fullPath);
      }).not.toThrow();

      // fixtureName matches basename
      expect(result!.fixtureName).toBe(fixtureName);

      // sections is an array
      expect(Array.isArray(result!.sections)).toBeTruthy();

      // diagnostics is an array
      expect(Array.isArray(result!.diagnostics)).toBeTruthy();

      // section count sanity: 0-30 per research findings
      expect(
        result!.sections.length >= 0 && result!.sections.length <= 30
      ).toBeTruthy();

      // Per-section metadata checks
      for (const section of result!.sections) {
        if (section.metadata !== null) {
          expect(typeof section.metadata.origin).toBe("string");
          expect(typeof section.metadata.name).toBe("string");
          expect(typeof section.metadata.hash).toBe("string");
          expect(typeof section.metadata.displayName).toBe("string");
        }
      }
    });
  }
});
