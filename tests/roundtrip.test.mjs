/**
 * Roundtrip validation test: confirms all 201 .snap files parse without errors.
 * Covers PARSE-04 requirement.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";
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
    assert.equal(
      snapFiles.length,
      CORPUS_SIZE,
      `Expected ${CORPUS_SIZE} .snap files, found ${snapFiles.length}`
    );
  });

  for (const file of snapFiles) {
    const fixtureName = path.basename(file, ".snap");

    test(`[${fixtureName}] parses without error`, () => {
      const fullPath = path.join(SNAP_DIR, file);
      let result;
      assert.doesNotThrow(() => {
        result = parseSnapFile(fullPath);
      }, `parseSnapFile should not throw for ${file}`);

      // fixtureName matches basename
      assert.equal(
        result.fixtureName,
        fixtureName,
        "fixtureName should match basename without .snap"
      );

      // sections is an array
      assert.ok(
        Array.isArray(result.sections),
        "sections should be an array"
      );

      // diagnostics is an array
      assert.ok(
        Array.isArray(result.diagnostics),
        "diagnostics should be an array"
      );

      // section count sanity: 0-30 per research findings
      assert.ok(
        result.sections.length >= 0 && result.sections.length <= 30,
        `section count ${result.sections.length} out of expected range [0, 30]`
      );

      // Per-section metadata checks
      for (const section of result.sections) {
        if (section.metadata !== null) {
          assert.ok(
            typeof section.metadata.origin === "string",
            `${fixtureName}: section.metadata.origin should be a string`
          );
          assert.ok(
            typeof section.metadata.name === "string",
            `${fixtureName}: section.metadata.name should be a string`
          );
          assert.ok(
            typeof section.metadata.hash === "string",
            `${fixtureName}: section.metadata.hash should be a string`
          );
          assert.ok(
            typeof section.metadata.displayName === "string",
            `${fixtureName}: section.metadata.displayName should be a string`
          );
        }
      }
    });
  }
});
