/**
 * CLI entry point for the qwik-oxc harness.
 *
 * Flags:
 *   --fixture <glob>    Filter by fixture name (basename without .snap). Zero matches = empty output, not error.
 *   --category <name>   Filter failures by FailureCategory value (validated). Phase 1 stub — no failures yet.
 *   --json              Emit JSON matching HarnessOutput schema instead of human summary.
 *
 * Exit codes:
 *   0 — all fixtures pass
 *   1 — any fixture has failures
 *   2 — harness error (uncaught exception, parse failure)
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { parseSnapFile } from "./parser.js";
import {
  FailureCategory,
  type FailureCategoryValue,
  type HarnessOutput,
} from "./contract.js";

/**
 * Parse CLI arguments into a typed options object.
 */
function parseArgs(argv: string[]): {
  fixture: string | null;
  category: string | null;
  json: boolean;
  swcSnapshots: string | null;
  oxcSnapshots: string | null;
} {
  let fixture: string | null = null;
  let category: string | null = null;
  let json = false;
  let swcSnapshots: string | null = null;
  let oxcSnapshots: string | null = null;

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!;
    if (arg === "--fixture" && argv[i + 1] !== undefined) {
      fixture = argv[++i]!;
    } else if (arg === "--category" && argv[i + 1] !== undefined) {
      category = argv[++i]!;
    } else if (arg === "--json") {
      json = true;
    } else if (arg === "--swc-snapshots" && argv[i + 1] !== undefined) {
      swcSnapshots = argv[++i]!;
    } else if (arg === "--oxc-snapshots" && argv[i + 1] !== undefined) {
      oxcSnapshots = argv[++i]!;
    }
  }

  return { fixture, category, json, swcSnapshots, oxcSnapshots };
}

/**
 * Build an empty byCategory record with all 19 FailureCategory keys set to 0.
 */
function buildEmptyByCategory(): Record<FailureCategoryValue, number> {
  const result = {} as Record<FailureCategoryValue, number>;
  for (const value of Object.values(FailureCategory) as FailureCategoryValue[]) {
    result[value] = 0;
  }
  return result;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));

  if (!args.swcSnapshots) {
    console.error(
      "Error: --swc-snapshots <dir> is required.\n" +
      "Usage: harness --swc-snapshots <dir> [--oxc-snapshots <dir>] [--fixture <glob>] [--category <name>] [--json]"
    );
    process.exit(2);
  }
  const SNAP_DIR = path.resolve(args.swcSnapshots);

  // Validate --category if provided
  if (args.category !== null) {
    const validCategories = Object.values(FailureCategory) as string[];
    if (!validCategories.includes(args.category)) {
      console.error(
        `Error: unknown category "${args.category}". Valid values:\n  ${validCategories.join("\n  ")}`
      );
      process.exit(2);
    }
  }

  // Discover all .snap files in swc-snapshots/, sorted alphabetically
  let snapFiles: string[];
  try {
    snapFiles = fs
      .readdirSync(SNAP_DIR)
      .filter((f) => f.endsWith(".snap"))
      .sort();
  } catch (err) {
    console.error("Harness error: could not read swc-snapshots/", err);
    process.exit(2);
  }

  // Apply --fixture glob filter
  if (args.fixture !== null) {
    const glob = args.fixture;
    snapFiles = snapFiles.filter((f) => {
      const name = path.basename(f, ".snap");
      return path.matchesGlob(name, glob);
    });
  }

  // Parse each fixture
  const fixtures: HarnessOutput["fixtures"] = [];
  for (const file of snapFiles) {
    const fullPath = path.join(SNAP_DIR, file);
    try {
      const parsed = parseSnapFile(fullPath);
      fixtures.push({
        name: parsed.fixtureName,
        pass: true,
        failures: [],
      });
    } catch (err) {
      console.error(`Harness error: failed to parse ${file}:`, err);
      process.exit(2);
    }
  }

  // Build HarnessOutput
  const byCategory = buildEmptyByCategory();

  const output: HarnessOutput = {
    fixtures,
    summary: {
      total: fixtures.length,
      passed: fixtures.length,
      failed: 0,
      byCategory,
    },
  };

  // Determine exit code
  const hasFailures = fixtures.some((f) => !f.pass);
  const exitCode = hasFailures ? 1 : 0;

  // Render output
  if (args.json) {
    console.log(JSON.stringify(output, null, 2));
  } else {
    console.log(`Parsed ${fixtures.length} fixtures. 0 failures.`);
    for (const fixture of fixtures) {
      const status = fixture.pass ? "PASS" : "FAIL";
      console.log(`  [${status}] ${fixture.name}`);
    }
  }

  process.exit(exitCode);
}

main().catch((err) => {
  console.error("Harness error:", err);
  process.exit(2);
});
