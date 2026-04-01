/**
 * CLI integration tests: covers CLI-01 through CLI-04 requirements.
 * Tests spawn the CLI as a subprocess via execFileSync.
 *
 * Run: bun test tests/cli.test.ts
 */

import { test, describe, expect } from "bun:test";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import * as path from "node:path";

const ROOT = path.resolve(fileURLToPath(import.meta.url), "../../");
const CLI = path.join(ROOT, "src/cli.ts");
const SWC_FLAG = ["--swc-snapshots", "swc-snapshots/"];

/**
 * Run the CLI with given arguments.
 * Returns { stdout, stderr, exitCode }.
 */
function runCLI(args: string[] = []) {
  try {
    const stdout = execFileSync(
      "bun",
      [CLI, ...args],
      { cwd: ROOT, encoding: "utf8" }
    );
    return { stdout, stderr: "", exitCode: 0 };
  } catch (err: any) {
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
    const { stdout, exitCode } = runCLI([...SWC_FLAG, "--fixture", "nonexistent", "--json"]);
    const json = JSON.parse(stdout);
    expect(json.fixtures.length).toBe(0);
    expect(json.summary.total).toBe(0);
    expect(exitCode).toBe(0);
  });

  test("--fixture example_1 produces exactly 1 fixture named 'example_1'", () => {
    const { stdout, exitCode } = runCLI([...SWC_FLAG, "--fixture", "example_1", "--json"]);
    const json = JSON.parse(stdout);
    expect(json.fixtures.length).toBe(1);
    expect(json.fixtures[0].name).toBe("example_1");
    expect(exitCode).toBe(0);
  });
});

// --- CLI-02: --category flag ---
describe("CLI-02: --category flag validation", () => {
  test("--category hash_mismatch runs without error and outputs valid JSON", () => {
    const { stdout, exitCode } = runCLI([...SWC_FLAG, "--category", "hash_mismatch", "--json"]);
    expect(exitCode).toBe(0);
    const json = JSON.parse(stdout);
    expect(Array.isArray(json.fixtures)).toBeTruthy();
  });

  test("--category invalid_value exits 2 with error message", () => {
    const { exitCode, stderr } = runCLI([...SWC_FLAG, "--category", "not_a_real_category", "--json"]);
    expect(exitCode).toBe(2);
    expect(
      stderr.includes("not_a_real_category") || stderr.includes("unknown category")
    ).toBeTruthy();
  });
});

// --- CLI-03: --json flag and HarnessOutput schema ---
describe("CLI-03: --json output schema", () => {
  test("--json produces valid JSON with correct structure", () => {
    const { stdout, exitCode } = runCLI([...SWC_FLAG, "--json"]);
    expect(exitCode).toBe(0);
    const json = JSON.parse(stdout);
    expect(Array.isArray(json.fixtures)).toBeTruthy();
    expect(typeof json.summary).toBe("object");
  });

  test("summary has correct counts: total 201, passed 201, failed 0", () => {
    const { stdout } = runCLI([...SWC_FLAG, "--json"]);
    const json = JSON.parse(stdout);
    expect(json.summary.total).toBe(201);
    expect(json.summary.passed).toBe(201);
    expect(json.summary.failed).toBe(0);
  });

  test("byCategory has exactly 19 keys, all values 0", () => {
    const { stdout } = runCLI([...SWC_FLAG, "--json"]);
    const json = JSON.parse(stdout);
    const keys = Object.keys(json.summary.byCategory);
    expect(keys.length).toBe(19);
    for (const [key, value] of Object.entries(json.summary.byCategory)) {
      expect(value).toBe(0);
    }
  });

  test("each fixture has name, pass:true, failures:[]", () => {
    const { stdout } = runCLI([...SWC_FLAG, "--json"]);
    const json = JSON.parse(stdout);
    for (const fixture of json.fixtures) {
      expect(typeof fixture.name).toBe("string");
      expect(fixture.pass).toBe(true);
      expect(fixture.failures).toEqual([]);
    }
  });
});

// --- CLI-05: REPT-03 — JSON stability and schema conformance ---
describe("CLI-05: --json stability and HarnessOutput schema (REPT-03)", () => {
  test("--json output is byte-identical across two runs (machine stability)", () => {
    const { stdout: run1 } = runCLI([...SWC_FLAG, "--json"]);
    const { stdout: run2 } = runCLI([...SWC_FLAG, "--json"]);
    expect(run1).toBe(run2);
  });

  test("--json output parses as valid JSON with all HarnessOutput schema fields", () => {
    const { stdout } = runCLI([...SWC_FLAG, "--json"]);
    const json = JSON.parse(stdout) as Record<string, unknown>;

    // fixtures array present
    expect(Array.isArray(json["fixtures"])).toBe(true);
    const fixtures = json["fixtures"] as Array<Record<string, unknown>>;

    // each fixture has name (string), pass (boolean), failures (array)
    for (const fixture of fixtures) {
      expect(typeof fixture["name"]).toBe("string");
      expect(typeof fixture["pass"]).toBe("boolean");
      expect(Array.isArray(fixture["failures"])).toBe(true);
    }

    // summary has total, passed, failed (numbers), byCategory (object)
    expect(typeof json["summary"]).toBe("object");
    const summary = json["summary"] as Record<string, unknown>;
    expect(typeof summary["total"]).toBe("number");
    expect(typeof summary["passed"]).toBe("number");
    expect(typeof summary["failed"]).toBe("number");
    expect(typeof summary["byCategory"]).toBe("object");
    expect(summary["byCategory"]).not.toBeNull();

    // byCategory has exactly 19 keys
    const byCategory = summary["byCategory"] as Record<string, unknown>;
    expect(Object.keys(byCategory).length).toBe(19);
  });

  test("--json fixtures are sorted by name ascending (REPT-03 deterministic order)", () => {
    const { stdout } = runCLI([...SWC_FLAG, "--json"]);
    const json = JSON.parse(stdout) as { fixtures: Array<{ name: string }> };
    const names = json.fixtures.map((f) => f.name);
    const sorted = [...names].sort((a, b) => a.localeCompare(b));
    expect(names).toEqual(sorted);
  });
});

// --- CLI-04: exit codes ---
describe("CLI-04: exit codes", () => {
  test("normal run exits 0", () => {
    const { exitCode } = runCLI([...SWC_FLAG, "--json"]);
    expect(exitCode).toBe(0);
  });

  test("missing --swc-snapshots exits 2 with usage message", () => {
    const { exitCode, stderr } = runCLI(["--json"]);
    expect(exitCode).toBe(2);
    expect(stderr).toContain("--swc-snapshots");
  });

  test("normal run without --json exits 0 and prints human summary", () => {
    const { stdout, exitCode } = runCLI([...SWC_FLAG]);
    expect(exitCode).toBe(0);
    expect(
      stdout.includes("201 fixtures") || stdout.includes("Parsed 201")
    ).toBeTruthy();
  });
});
