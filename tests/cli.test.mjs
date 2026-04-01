/**
 * CLI integration tests: covers CLI-01 through CLI-04 requirements.
 * Tests spawn the CLI as a subprocess via execFileSync.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import * as path from "node:path";

const ROOT = path.resolve(fileURLToPath(import.meta.url), "../../");
const CLI = path.join(ROOT, "src/cli.ts");

/**
 * Run the CLI with given arguments.
 * Returns { stdout, stderr, exitCode }.
 */
function runCLI(args = []) {
  try {
    const stdout = execFileSync(
      process.execPath,
      ["--import", "tsx/esm", CLI, ...args],
      { cwd: ROOT, encoding: "utf8" }
    );
    return { stdout, stderr: "", exitCode: 0 };
  } catch (err) {
    return {
      stdout: err.stdout ?? "",
      stderr: err.stderr ?? "",
      exitCode: err.status ?? 1,
    };
  }
}

// --- CLI-01: fixture glob filtering ---
describe("CLI-01: --fixture glob filtering", () => {
  test("--fixture nonexistent produces 0 fixtures with exit 0", () => {
    const { stdout, exitCode } = runCLI(["--fixture", "nonexistent", "--json"]);
    const json = JSON.parse(stdout);
    assert.equal(json.fixtures.length, 0, "should have 0 fixtures");
    assert.equal(json.summary.total, 0, "summary.total should be 0");
    assert.equal(exitCode, 0, "exit code should be 0");
  });

  test("--fixture example_1 produces exactly 1 fixture named 'example_1'", () => {
    const { stdout, exitCode } = runCLI(["--fixture", "example_1", "--json"]);
    const json = JSON.parse(stdout);
    assert.equal(json.fixtures.length, 1, "should have 1 fixture");
    assert.equal(json.fixtures[0].name, "example_1", "fixture name should match");
    assert.equal(exitCode, 0, "exit code should be 0");
  });
});

// --- CLI-02: --category flag ---
describe("CLI-02: --category flag validation", () => {
  test("--category hash_mismatch runs without error and outputs valid JSON", () => {
    const { stdout, exitCode } = runCLI(["--category", "hash_mismatch", "--json"]);
    assert.equal(exitCode, 0, "exit code should be 0 for valid category");
    const json = JSON.parse(stdout);
    assert.ok(Array.isArray(json.fixtures), "fixtures should be an array");
  });

  test("--category invalid_value exits 2 with error message", () => {
    const { exitCode, stderr } = runCLI(["--category", "not_a_real_category", "--json"]);
    assert.equal(exitCode, 2, "exit code should be 2 for invalid category");
    assert.ok(
      stderr.includes("not_a_real_category") || stderr.includes("unknown category"),
      "stderr should mention the invalid category"
    );
  });
});

// --- CLI-03: --json flag and HarnessOutput schema ---
describe("CLI-03: --json output schema", () => {
  test("--json produces valid JSON with correct structure", () => {
    const { stdout, exitCode } = runCLI(["--json"]);
    assert.equal(exitCode, 0, "exit code should be 0");
    const json = JSON.parse(stdout);
    assert.ok(Array.isArray(json.fixtures), "fixtures should be an array");
    assert.ok(typeof json.summary === "object", "summary should be an object");
  });

  test("summary has correct counts: total 201, passed 201, failed 0", () => {
    const { stdout } = runCLI(["--json"]);
    const json = JSON.parse(stdout);
    assert.equal(json.summary.total, 201, "total should be 201");
    assert.equal(json.summary.passed, 201, "passed should be 201");
    assert.equal(json.summary.failed, 0, "failed should be 0");
  });

  test("byCategory has exactly 19 keys, all values 0", () => {
    const { stdout } = runCLI(["--json"]);
    const json = JSON.parse(stdout);
    const keys = Object.keys(json.summary.byCategory);
    assert.equal(keys.length, 19, "byCategory should have 19 keys");
    for (const [key, value] of Object.entries(json.summary.byCategory)) {
      assert.equal(value, 0, `byCategory.${key} should be 0`);
    }
  });

  test("each fixture has name, pass:true, failures:[]", () => {
    const { stdout } = runCLI(["--json"]);
    const json = JSON.parse(stdout);
    for (const fixture of json.fixtures) {
      assert.ok(typeof fixture.name === "string", "fixture.name should be a string");
      assert.equal(fixture.pass, true, `${fixture.name} should pass`);
      assert.deepEqual(fixture.failures, [], `${fixture.name} should have no failures`);
    }
  });
});

// --- CLI-04: exit codes ---
describe("CLI-04: exit codes", () => {
  test("normal run exits 0", () => {
    const { exitCode } = runCLI(["--json"]);
    assert.equal(exitCode, 0, "normal run should exit 0");
  });

  test("normal run without --json exits 0 and prints human summary", () => {
    const { stdout, exitCode } = runCLI([]);
    assert.equal(exitCode, 0, "exit code should be 0");
    assert.ok(
      stdout.includes("201 fixtures") || stdout.includes("Parsed 201"),
      "human output should mention fixture count"
    );
  });
});
