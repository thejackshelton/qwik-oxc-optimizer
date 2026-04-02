/**
 * CLI entry point for the qwik-oxc harness.
 *
 * Flags:
 *   --fixture <glob>          Filter by fixture name (basename without .snap). Zero matches = empty output, not error.
 *   --category <name>         Filter failures by FailureCategory value (validated).
 *   --json                    Emit JSON matching HarnessOutput schema instead of human summary.
 *   --swc-snapshots <dir>     Directory of SWC golden .snap files (required).
 *   --oxc-snapshots <dir>     Directory of OXC candidate .snap files (required).
 *
 * Exit codes:
 *   0 — all fixtures pass
 *   1 — any fixture has failures
 *   2 — harness error (uncaught exception, parse failure)
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { parseSnapFile } from "./parser.js";
import type { ParsedSnapshot } from "./types.js";
import { assertOxfmtVersion, warmNormalizationCache } from "./normalizer.js";
import { compareFixture, getStdinFilepath } from "./comparator.js";
import {
  FailureCategory,
  type FailureCategoryValue,
  type HarnessOutput,
} from "./contract.js";
import { formatTerminalReport, formatStableJson, type StepTraceEntry } from "./reporter.js";
import { matchSegments } from "./matcher.js";

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
      "Usage: harness --swc-snapshots <dir> --oxc-snapshots <dir> [--fixture <glob>] [--category <name>] [--json]"
    );
    process.exit(2);
  }
  if (!args.oxcSnapshots) {
    console.error(
      "Error: --oxc-snapshots <dir> is required.\n" +
      "Usage: harness --swc-snapshots <dir> --oxc-snapshots <dir> [--fixture <glob>] [--category <name>] [--json]"
    );
    process.exit(2);
  }
  const SNAP_DIR = path.resolve(args.swcSnapshots);
  const OXC_DIR = path.resolve(args.oxcSnapshots);

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

  // Fail fast if oxfmt version doesn't match frozen contract
  if (snapFiles.length > 0) {
    try {
      assertOxfmtVersion();
    } catch (err) {
      console.error(`Harness error: ${(err as Error).message}`);
      process.exit(2);
    }
  }

  // Parse all snapshots first
  const parsedPairs: Array<{ swc: ParsedSnapshot; oxc: ParsedSnapshot }> = [];
  for (const file of snapFiles) {
    const swcPath = path.join(SNAP_DIR, file);
    try {
      const swcParsed = parseSnapFile(swcPath);
      const oxcPath = path.join(OXC_DIR, file);
      let oxcParsed;
      try {
        oxcParsed = parseSnapFile(oxcPath);
      } catch (err) {
        console.error(`Harness error: failed to parse OXC snapshot ${file}:`, err);
        process.exit(2);
      }
      parsedPairs.push({ swc: swcParsed, oxc: oxcParsed });
    } catch (err) {
      console.error(`Harness error: failed to parse ${file}:`, err);
      process.exit(2);
    }
  }

  // Warm normalization cache — all code blocks normalized concurrently
  const codeEntries: Array<{ code: string; filepath: string }> = [];
  for (const { swc, oxc } of parsedPairs) {
    for (const section of swc.sections) {
      codeEntries.push({ code: section.code, filepath: getStdinFilepath(section.headerName) });
    }
    for (const section of oxc.sections) {
      codeEntries.push({ code: section.code, filepath: getStdinFilepath(section.headerName) });
    }
  }
  await warmNormalizationCache(codeEntries);

  // Compare each fixture (normalizeCode calls are now cache hits)
  const fixtures: HarnessOutput["fixtures"] = [];
  for (const { swc, oxc } of parsedPairs) {
    const failures = compareFixture(swc, oxc);
    fixtures.push({
      name: swc.fixtureName,
      pass: failures.length === 0,
      failures,
    });
  }

  // Apply --category filter: keep only failures matching the specified category
  if (args.category !== null) {
    const categoryFilter = args.category as FailureCategoryValue;
    for (const fixture of fixtures) {
      fixture.failures = fixture.failures.filter((f) => f.category === categoryFilter);
      fixture.pass = fixture.failures.length === 0;
    }
  }

  // Build HarnessOutput
  const byCategory = buildEmptyByCategory();

  // Accumulate byCategory counts from all failures
  for (const fixture of fixtures) {
    for (const failure of fixture.failures) {
      byCategory[failure.category] = (byCategory[failure.category] ?? 0) + 1;
    }
  }

  const passed = fixtures.filter((f) => f.pass).length;
  const failed = fixtures.filter((f) => !f.pass).length;

  const output: HarnessOutput = {
    fixtures,
    summary: {
      total: fixtures.length,
      passed,
      failed,
      byCategory,
    },
  };

  // Determine exit code
  const hasFailures = fixtures.some((f) => !f.pass);
  const exitCode = hasFailures ? 1 : 0;

  // Render output
  if (args.json) {
    console.log(formatStableJson(output));
  } else {
    // Build step-trace annotations for derived-field failure explanations (REPT-05)
    const stepTraces: StepTraceEntry[] = [];
    {
      // Load fixtures.json for scope/relPath data needed by step-trace
      const fixturesJsonPath = path.resolve(path.dirname(SNAP_DIR), "fixtures.json");
      let fixtureConfigs: Record<string, { scope?: string | null; inputs?: Array<{ path: string }> }> = {};
      try {
        fixtureConfigs = JSON.parse(fs.readFileSync(fixturesJsonPath, "utf8")).fixtures ?? {};
      } catch {
        // fixtures.json not available — step-trace will lack scope/relPath context
      }

      for (const fixture of output.fixtures) {
        if (fixture.pass || fixture.failures.length === 0) continue;

        // Re-match segments to get metadata pairings
        const swcParsed = parseSnapFile(path.join(SNAP_DIR, `${fixture.name}.snap`));
        const oxcParsed = parseSnapFile(path.join(OXC_DIR, `${fixture.name}.snap`));
        const matchResult = matchSegments(swcParsed.sections, oxcParsed.sections);

        const config = fixtureConfigs[fixture.name];
        const scope = config?.scope ?? null;
        // Build origin→inputPath lookup for per-segment relPath resolution
        const inputByOrigin = new Map<string, string>();
        if (config?.inputs) {
          for (const input of config.inputs) {
            inputByOrigin.set(input.path.replace(/\\/g, "/"), input.path);
          }
        }

        const entries: StepTraceEntry["failures"] = [];
        for (const failure of fixture.failures) {
          // Find the matching segment pair for this failure's field
          const matchedPair = matchResult.matches.find((m) => {
            const swcMeta = m.swcSection.metadata;
            const oxcMeta = m.oxcSection.metadata;
            if (!swcMeta || !oxcMeta) return false;
            // Match by checking if the failure's expected/actual values correspond
            if (failure.field && failure.field in swcMeta) {
              return (swcMeta as unknown as Record<string, unknown>)[failure.field] === failure.expected;
            }
            return false;
          });

          // Use the segment's own origin for relPath (not the first input)
          const segOrigin = matchedPair?.swcSection.metadata?.origin;
          const relPath = segOrigin ? (inputByOrigin.get(segOrigin) ?? segOrigin) : undefined;

          entries.push({
            failure,
            swcMeta: matchedPair?.swcSection.metadata ?? null,
            oxcMeta: matchedPair?.oxcSection.metadata ?? null,
            scope,
            relPath,
          });
        }

        if (entries.length > 0) {
          stepTraces.push({ fixtureName: fixture.name, failures: entries });
        }
      }
    }
    console.log(formatTerminalReport(output, stepTraces));
  }

  process.exit(exitCode);
}

main().catch((err) => {
  console.error("Harness error:", err);
  process.exit(2);
});
