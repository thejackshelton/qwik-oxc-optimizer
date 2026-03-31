#!/usr/bin/env node
/**
 * verify-corpus.mjs — Corpus freeze verification script
 *
 * Checks:
 *   CORP-01: inputs/ has exactly 201 .tsx files
 *   CORP-02: swc-snapshots/ has exactly 201 .snap files
 *   CORP-03/04 (conditional): fixtures.json valid structure and cross-references
 *   CORP-05 (conditional): src/contract.ts has all 19 FailureCategory values and OXFMT_VERSION matches package.json
 *
 * Exits 0 if all required checks pass, 1 if any required check fails.
 * Conditional checks are skipped (SKIP) when their prerequisite files don't exist yet.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");

const EXPECTED_COUNT = 201;
let hasFailure = false;

function pass(msg) {
  console.log(`[PASS] ${msg}`);
}

function fail(msg) {
  console.log(`[FAIL] ${msg}`);
  hasFailure = true;
}

function skip(msg) {
  console.log(`[SKIP] ${msg}`);
}

// ─── CORP-01: inputs/ .tsx file count ────────────────────────────────────────

const inputsDir = path.join(ROOT, "inputs");
let inputsFiles = [];
if (!fs.existsSync(inputsDir)) {
  fail(`inputs/ directory does not exist`);
} else {
  inputsFiles = fs.readdirSync(inputsDir).filter((f) => f.endsWith(".tsx"));
  if (inputsFiles.length === EXPECTED_COUNT) {
    pass(`inputs/ has ${EXPECTED_COUNT} .tsx files`);
  } else {
    fail(
      `inputs/ has ${inputsFiles.length} .tsx files (expected ${EXPECTED_COUNT})`
    );
  }
}

// ─── CORP-02: swc-snapshots/ .snap file count ────────────────────────────────

const snapDir = path.join(ROOT, "swc-snapshots");
let snapFiles = [];
if (!fs.existsSync(snapDir)) {
  fail(`swc-snapshots/ directory does not exist`);
} else {
  snapFiles = fs.readdirSync(snapDir).filter((f) => f.endsWith(".snap"));
  if (snapFiles.length === EXPECTED_COUNT) {
    pass(`swc-snapshots/ has ${EXPECTED_COUNT} .snap files`);
  } else {
    fail(
      `swc-snapshots/ has ${snapFiles.length} .snap files (expected ${EXPECTED_COUNT})`
    );
  }
}

// ─── CORP-03/04: fixtures.json validation (conditional) ──────────────────────

const fixturesJsonPath = path.join(ROOT, "fixtures.json");
if (!fs.existsSync(fixturesJsonPath)) {
  skip("fixtures.json not found — CORP-03/04 checks skipped");
} else {
  let fixtures;
  try {
    fixtures = JSON.parse(fs.readFileSync(fixturesJsonPath, "utf8"));
    pass("fixtures.json is valid JSON");
  } catch (e) {
    fail(`fixtures.json is not valid JSON: ${e.message}`);
    fixtures = null;
  }

  if (fixtures !== null) {
    // Check version field
    if (fixtures.version === 1) {
      pass("fixtures.json version field equals 1");
    } else {
      fail(
        `fixtures.json version field is ${JSON.stringify(fixtures.version)} (expected 1)`
      );
    }

    // Check fixture count
    const fixtureKeys = Object.keys(fixtures.fixtures ?? {});
    if (fixtureKeys.length === EXPECTED_COUNT) {
      pass(`fixtures.json has ${EXPECTED_COUNT} fixture entries`);
    } else {
      fail(
        `fixtures.json has ${fixtureKeys.length} fixture entries (expected ${EXPECTED_COUNT})`
      );
    }

    // Check each fixture has non-empty inputs with non-empty code
    let inputsOk = true;
    for (const [name, config] of Object.entries(fixtures.fixtures ?? {})) {
      if (!Array.isArray(config.inputs) || config.inputs.length === 0) {
        fail(`fixture "${name}" has empty or missing inputs array`);
        inputsOk = false;
      } else {
        for (const input of config.inputs) {
          if (typeof input.code !== "string" || input.code.length === 0) {
            fail(`fixture "${name}" has input with empty code string`);
            inputsOk = false;
          }
        }
      }
    }
    if (inputsOk) {
      pass("all fixture inputs have non-empty code strings");
    }

    // CORP-04: relative_paths must have exactly 2 inputs
    const relPaths = fixtures.fixtures?.["relative_paths"];
    if (!relPaths) {
      fail('fixtures.json is missing "relative_paths" fixture');
    } else if (relPaths.inputs?.length === 2) {
      pass('relative_paths fixture has exactly 2 inputs');
    } else {
      fail(
        `relative_paths fixture has ${relPaths.inputs?.length ?? 0} inputs (expected 2)`
      );
    }

    // Cross-validate: every fixture key should have a matching .snap file
    const snapNames = new Set(snapFiles.map((f) => f.replace(/\.snap$/, "")));
    let snapCrossOk = true;
    for (const name of fixtureKeys) {
      if (!snapNames.has(name)) {
        fail(`fixture "${name}" has no matching .snap file in swc-snapshots/`);
        snapCrossOk = false;
      }
    }
    if (snapCrossOk) {
      pass("every fixture key has a matching .snap file in swc-snapshots/");
    }

    // Cross-validate: single-input fixtures should have a matching .tsx in inputs/
    const inputNames = new Set(inputsFiles.map((f) => f.replace(/\.tsx$/, "")));
    let inputCrossOk = true;
    for (const [name, config] of Object.entries(fixtures.fixtures ?? {})) {
      if (config.inputs?.length === 1) {
        if (!inputNames.has(name)) {
          fail(
            `single-input fixture "${name}" has no matching .tsx file in inputs/`
          );
          inputCrossOk = false;
        }
      }
    }
    if (inputCrossOk) {
      pass(
        "all single-input fixtures have a matching .tsx file in inputs/"
      );
    }
  }
}

// ─── CORP-05: src/contract.ts validation (conditional) ───────────────────────

const contractPath = path.join(ROOT, "src", "contract.ts");
if (!fs.existsSync(contractPath)) {
  skip("src/contract.ts not found — CORP-05 checks skipped");
} else {
  const contractSource = fs.readFileSync(contractPath, "utf8");

  // All 19 required FailureCategory values
  const requiredCategories = [
    "segment_count_mismatch",
    "missing_segment",
    "extra_segment",
    "hash_mismatch",
    "display_name_mismatch",
    "canonical_filename_mismatch",
    "wrong_captures",
    "wrong_capture_names",
    "wrong_ctx_kind",
    "wrong_ctx_name",
    "wrong_parent",
    "wrong_entry",
    "wrong_loc",
    "wrong_param_names",
    "wrong_extension",
    "wrong_origin",
    "wrong_path",
    "code_diff",
    "diagnostics_mismatch",
  ];

  let categoriesOk = true;
  for (const cat of requiredCategories) {
    if (!contractSource.includes(`"${cat}"`)) {
      fail(`src/contract.ts is missing FailureCategory value: "${cat}"`);
      categoriesOk = false;
    }
  }
  if (categoriesOk) {
    pass(`src/contract.ts has all ${requiredCategories.length} FailureCategory values`);
  }

  // OXFMT_VERSION must match package.json devDependencies.oxfmt
  const packageJsonPath = path.join(ROOT, "package.json");
  if (!fs.existsSync(packageJsonPath)) {
    skip("package.json not found — OXFMT_VERSION cross-check skipped");
  } else {
    let pkg;
    try {
      pkg = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
    } catch (e) {
      fail(`package.json is not valid JSON: ${e.message}`);
      pkg = null;
    }
    if (pkg !== null) {
      const oxfmtVersion = pkg.devDependencies?.oxfmt;
      if (!oxfmtVersion) {
        fail("package.json does not have devDependencies.oxfmt");
      } else {
        // Extract OXFMT_VERSION value from contract.ts
        const versionMatch = contractSource.match(
          /OXFMT_VERSION\s*=\s*"([^"]+)"/
        );
        if (!versionMatch) {
          fail("src/contract.ts does not define OXFMT_VERSION");
        } else {
          const contractVersion = versionMatch[1];
          if (contractVersion === oxfmtVersion) {
            pass(
              `OXFMT_VERSION "${contractVersion}" matches package.json devDependencies.oxfmt`
            );
          } else {
            fail(
              `OXFMT_VERSION "${contractVersion}" does not match package.json devDependencies.oxfmt "${oxfmtVersion}"`
            );
          }
        }
      }
    }
  }
}

// ─── Result ──────────────────────────────────────────────────────────────────

if (hasFailure) {
  console.log("\nCORPUS VERIFICATION FAILED");
  process.exit(1);
} else {
  console.log("\nCORPUS VERIFICATION PASSED");
  process.exit(0);
}
