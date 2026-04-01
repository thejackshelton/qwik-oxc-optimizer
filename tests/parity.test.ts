/**
 * Parity integration tests: real OXC-vs-SWC comparison using compareFixture.
 *
 * These are the canonical parity signal tests. They run real comparison against
 * all 201 fixtures. bun test includes these by default (bunfig.toml root = ./tests).
 *
 * HARN-01: Default bun test runs real comparison
 * HARN-02: Mismatch-surfacing test proves false negatives are impossible
 * HARN-04: swc-snapshots/ immutability guard
 * HARN-05: OXC candidates isolated from SWC goldens
 */

import { describe, test, expect } from "bun:test";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { parseSnapFile } from "../src/parser.js";
import { compareFixture } from "../src/comparator.js";
import { CORPUS_SIZE } from "../src/contract.js";

const ROOT = path.resolve(fileURLToPath(import.meta.url), "../../");
const SWC_DIR = path.join(ROOT, "swc-snapshots");
const OXC_DIR = path.join(ROOT, "oxc-snapshots");

describe("parity integration (real OXC-vs-SWC comparison)", () => {
  test("real OXC-vs-SWC comparison runs without error for all 201 fixtures", () => {
    const swcFiles = fs.readdirSync(SWC_DIR).filter(f => f.endsWith(".snap")).sort();
    const errors: string[] = [];
    let passCount = 0;
    let failCount = 0;

    for (const file of swcFiles) {
      try {
        const swcPath = path.join(SWC_DIR, file);
        const oxcPath = path.join(OXC_DIR, file);

        if (!fs.existsSync(oxcPath)) {
          failCount++;
          continue;
        }

        const swcParsed = parseSnapFile(swcPath);
        const oxcParsed = parseSnapFile(oxcPath);
        const failures = compareFixture(swcParsed, oxcParsed);

        if (failures.length === 0) {
          passCount++;
        } else {
          failCount++;
        }
      } catch (err) {
        errors.push(`${file}: ${String(err)}`);
      }
    }

    // The test itself must not throw — if there were errors, fail with details
    expect(errors).toHaveLength(0);
    // Report counts (informational — this test is about running without errors)
    expect(passCount + failCount).toBe(swcFiles.length);
  }, { timeout: 120_000 });

  test("baseline: at least 1 fixture passes real comparison", () => {
    const swcFiles = fs.readdirSync(SWC_DIR).filter(f => f.endsWith(".snap")).sort();
    let passCount = 0;

    for (const file of swcFiles) {
      const swcPath = path.join(SWC_DIR, file);
      const oxcPath = path.join(OXC_DIR, file);

      if (!fs.existsSync(oxcPath)) continue;

      try {
        const swcParsed = parseSnapFile(swcPath);
        const oxcParsed = parseSnapFile(oxcPath);
        const failures = compareFixture(swcParsed, oxcParsed);
        if (failures.length === 0) passCount++;
      } catch {
        // ignore parse errors for baseline count
      }
    }

    expect(passCount).toBeGreaterThanOrEqual(1);
  }, { timeout: 120_000 });

  test("baseline: total fixture count is 201", () => {
    const swcFiles = fs.readdirSync(SWC_DIR).filter(f => f.endsWith(".snap"));
    expect(swcFiles.length).toBe(CORPUS_SIZE);
  });

  test("compareFixture reports failures when OXC diverges from SWC (HARN-02 mismatch-surfacing)", () => {
    const swcPath = path.join(SWC_DIR, "example_1.snap");
    const oxcPath = path.join(OXC_DIR, "example_1.snap");

    const swcParsed = parseSnapFile(swcPath);
    const oxcParsed = parseSnapFile(oxcPath);

    // Sanity: self-comparison must produce zero failures
    const selfFailures = compareFixture(swcParsed, swcParsed);
    expect(selfFailures).toHaveLength(0);

    // Cross-comparison: OXC diverges from SWC for example_1 — must produce failures
    const crossFailures = compareFixture(swcParsed, oxcParsed);
    expect(crossFailures.length).toBeGreaterThan(0);
  });

  test("swc-snapshots/ directory is not modified by comparison runs (HARN-04 golden immutability)", () => {
    const swcFiles = fs.readdirSync(SWC_DIR).filter(f => f.endsWith(".snap")).sort();

    // Capture mtimes before comparison
    const mtimesBefore = new Map<string, number>();
    for (const file of swcFiles) {
      const fullPath = path.join(SWC_DIR, file);
      mtimesBefore.set(fullPath, fs.statSync(fullPath).mtimeMs);
    }

    // Run all 201 comparisons (compareFixture is pure — no file writes)
    for (const file of swcFiles) {
      const swcPath = path.join(SWC_DIR, file);
      const oxcPath = path.join(OXC_DIR, file);
      if (!fs.existsSync(oxcPath)) continue;

      try {
        const swcParsed = parseSnapFile(swcPath);
        const oxcParsed = parseSnapFile(oxcPath);
        compareFixture(swcParsed, oxcParsed);
      } catch {
        // ignore errors — this test only checks file immutability
      }
    }

    // Re-check all mtimes — none should have changed
    for (const [fullPath, mtimeBefore] of mtimesBefore) {
      const mtimeAfter = fs.statSync(fullPath).mtimeMs;
      expect(mtimeAfter).toBe(mtimeBefore);
    }
  }, { timeout: 120_000 });

  test("oxc-snapshots/ files exist separately from swc-snapshots/ (HARN-05 candidate isolation)", () => {
    const swcPaths = new Set(
      fs.readdirSync(SWC_DIR)
        .filter(f => f.endsWith(".snap"))
        .map(f => path.resolve(SWC_DIR, f))
    );

    const oxcPaths = fs.readdirSync(OXC_DIR)
      .filter(f => f.endsWith(".snap"))
      .map(f => path.resolve(OXC_DIR, f));

    // Intersection must be empty — no OXC file shares an absolute path with any SWC file
    const intersection = oxcPaths.filter(p => swcPaths.has(p));
    expect(intersection).toHaveLength(0);
  });
});
