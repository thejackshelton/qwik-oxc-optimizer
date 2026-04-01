#!/usr/bin/env node

/**
 * sync-upstream-snapshots.mjs
 *
 * Orchestrates the full upstream snapshot sync pipeline:
 *   1. Discover upstream qwik_core__test__*.snap files
 *   2. Copy + rename (strip prefix) into swc-snapshots/
 *   3. Prune local .snap files no longer in upstream
 *   4. Update CORPUS_SIZE / EXPECTED_COUNT / fixtureCount guard if count changed
 *   5. Rebuild fixtures.json via build-fixtures-json.mjs
 *   6. Verify corpus via verify-corpus.mjs
 *   7. Print summary
 *
 * Usage:
 *   node scripts/sync-upstream-snapshots.mjs [--qwik-dir <path>] [--dry-run]
 *
 * Options:
 *   --qwik-dir <path>   Path to upstream qwik repository root.
 *                       Falls back to QWIK_DIR env var, then ../qwik sibling.
 *   --dry-run           Print what would change without writing any files.
 *
 * Exit codes:
 *   0  success
 *   1  sync failure (verify failed, missing upstream dir, copy/prune error)
 *   2  usage error (invalid --qwik-dir, upstream snapshots dir not found)
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, "..");

// ---------------------------------------------------------------------------
// Parse CLI flags
// ---------------------------------------------------------------------------

const args = process.argv.slice(2);
const DRY_RUN = args.includes("--dry-run");

function resolveQwikDir() {
  const argIdx = args.indexOf("--qwik-dir");
  if (argIdx !== -1) {
    const val = args[argIdx + 1];
    if (!val || val.startsWith("-")) {
      console.error("[ERROR] --qwik-dir requires a path argument");
      process.exit(2);
    }
    return path.resolve(val);
  }
  if (process.env.QWIK_DIR) {
    return path.resolve(process.env.QWIK_DIR);
  }
  return path.resolve(PROJECT_ROOT, "../qwik");
}

const QWIK_DIR = resolveQwikDir();
const UPSTREAM_SNAP_DIR = path.join(
  QWIK_DIR,
  "packages/optimizer/core/src/snapshots"
);
const LOCAL_SNAP_DIR = path.join(PROJECT_ROOT, "swc-snapshots");
const CONTRACT_TS = path.join(PROJECT_ROOT, "src", "contract.ts");
const VERIFY_CORPUS_MJS = path.join(PROJECT_ROOT, "scripts", "verify-corpus.mjs");
const BUILD_FIXTURES_MJS = path.join(PROJECT_ROOT, "scripts", "build-fixtures-json.mjs");

const UPSTREAM_PREFIX = "qwik_core__test__";

// ---------------------------------------------------------------------------
// Logging helpers
// ---------------------------------------------------------------------------

function log(msg) {
  console.log(msg);
}

function logPass(msg) {
  console.log(`[PASS] ${msg}`);
}

function logFail(msg) {
  console.error(`[FAIL] ${msg}`);
}

// ---------------------------------------------------------------------------
// Step 1: Validate upstream snapshots directory
// ---------------------------------------------------------------------------

if (!fs.existsSync(UPSTREAM_SNAP_DIR)) {
  logFail(`Upstream snapshots directory not found: ${UPSTREAM_SNAP_DIR}`);
  logFail(`  Resolved qwik-dir: ${QWIK_DIR}`);
  logFail(`  Pass the correct path with: --qwik-dir /path/to/qwik`);
  logFail(`  Or set the QWIK_DIR environment variable.`);
  process.exit(2);
}

log(`Upstream snapshots dir: ${UPSTREAM_SNAP_DIR}`);
log(`Local snapshots dir:    ${LOCAL_SNAP_DIR}`);
if (DRY_RUN) {
  log(`Mode: DRY RUN (no files will be written)`);
}
log("");

// ---------------------------------------------------------------------------
// Step 2: Discover upstream snapshots
// ---------------------------------------------------------------------------

const upstreamFiles = fs
  .readdirSync(UPSTREAM_SNAP_DIR)
  .filter((f) => f.endsWith(".snap"));

// Validate all upstream files have the expected prefix
const invalid = upstreamFiles.filter((f) => !f.startsWith(UPSTREAM_PREFIX));
if (invalid.length > 0) {
  logFail(
    `Found ${invalid.length} upstream .snap file(s) without expected prefix "${UPSTREAM_PREFIX}":`
  );
  for (const f of invalid) {
    logFail(`  ${f}`);
  }
  logFail(`Aborting — unexpected upstream file naming convention.`);
  process.exit(1);
}

log(`Upstream: found ${upstreamFiles.length} .snap files`);

// Build map: localName -> upstreamName
const upstreamMap = new Map();
for (const upstreamFile of upstreamFiles) {
  const localName = upstreamFile.slice(UPSTREAM_PREFIX.length);
  upstreamMap.set(localName, upstreamFile);
}

// ---------------------------------------------------------------------------
// Step 3: Copy + rename upstream snapshots to local swc-snapshots/
// ---------------------------------------------------------------------------

let copied = 0;
let unchanged = 0;

if (!fs.existsSync(LOCAL_SNAP_DIR)) {
  if (DRY_RUN) {
    log(`[DRY] Would create directory: ${LOCAL_SNAP_DIR}`);
  } else {
    fs.mkdirSync(LOCAL_SNAP_DIR, { recursive: true });
  }
}

for (const [localName, upstreamName] of upstreamMap) {
  const srcPath = path.join(UPSTREAM_SNAP_DIR, upstreamName);
  const destPath = path.join(LOCAL_SNAP_DIR, localName);

  const srcContent = fs.readFileSync(srcPath, "utf8");

  if (DRY_RUN) {
    const exists = fs.existsSync(destPath);
    const destContent = exists ? fs.readFileSync(destPath, "utf8") : null;
    if (!exists || destContent !== srcContent) {
      log(`[DRY] Would copy: ${upstreamName} -> swc-snapshots/${localName}`);
      copied++;
    } else {
      unchanged++;
    }
  } else {
    const exists = fs.existsSync(destPath);
    const destContent = exists ? fs.readFileSync(destPath, "utf8") : null;
    if (!exists || destContent !== srcContent) {
      fs.writeFileSync(destPath, srcContent, "utf8");
      copied++;
    } else {
      unchanged++;
    }
  }
}

log(`Copied/updated: ${copied} files, unchanged: ${unchanged} files`);

// ---------------------------------------------------------------------------
// Step 4: Prune local .snap files no longer in upstream
// ---------------------------------------------------------------------------

const localFiles = fs.existsSync(LOCAL_SNAP_DIR)
  ? fs.readdirSync(LOCAL_SNAP_DIR).filter((f) => f.endsWith(".snap"))
  : [];

let pruned = 0;
for (const localFile of localFiles) {
  if (!upstreamMap.has(localFile)) {
    if (DRY_RUN) {
      log(`[DRY] Would prune: swc-snapshots/${localFile} (no upstream counterpart)`);
    } else {
      fs.unlinkSync(path.join(LOCAL_SNAP_DIR, localFile));
    }
    pruned++;
  }
}

log(`Pruned: ${pruned} files no longer in upstream`);

// ---------------------------------------------------------------------------
// Step 5: Update CORPUS_SIZE / EXPECTED_COUNT / fixtureCount guard
// ---------------------------------------------------------------------------

const newCount = upstreamMap.size;

// Read current CORPUS_SIZE from contract.ts
let currentCount = null;
if (fs.existsSync(CONTRACT_TS)) {
  const contractSource = fs.readFileSync(CONTRACT_TS, "utf8");
  const m = contractSource.match(/export const CORPUS_SIZE = (\d+);/);
  if (m) {
    currentCount = parseInt(m[1], 10);
  }
}

if (currentCount !== null && currentCount !== newCount) {
  log(`\nCount changed: ${currentCount} -> ${newCount}`);

  if (DRY_RUN) {
    log(`[DRY] Would update CORPUS_SIZE in src/contract.ts: ${currentCount} -> ${newCount}`);
    log(`[DRY] Would update EXPECTED_COUNT in scripts/verify-corpus.mjs: ${currentCount} -> ${newCount}`);
    log(`[DRY] Would update fixtureCount guard in scripts/build-fixtures-json.mjs: ${currentCount} -> ${newCount}`);
  } else {
    // Update contract.ts
    let contractSource = fs.readFileSync(CONTRACT_TS, "utf8");
    const contractBefore = contractSource;
    contractSource = contractSource.replace(
      /export const CORPUS_SIZE = \d+;/,
      `export const CORPUS_SIZE = ${newCount};`
    );
    if (contractSource === contractBefore) {
      logFail(`CORPUS_SIZE pattern not found in src/contract.ts — file may be stale`);
      process.exit(1);
    }
    fs.writeFileSync(CONTRACT_TS, contractSource, "utf8");
    logPass(`Updated CORPUS_SIZE in src/contract.ts`);

    // Update verify-corpus.mjs
    let verifySource = fs.readFileSync(VERIFY_CORPUS_MJS, "utf8");
    const verifyBefore = verifySource;
    verifySource = verifySource.replace(
      /const EXPECTED_COUNT = \d+;/,
      `const EXPECTED_COUNT = ${newCount};`
    );
    if (verifySource === verifyBefore) {
      logFail(`EXPECTED_COUNT pattern not found in scripts/verify-corpus.mjs — file may be stale`);
      process.exit(1);
    }
    fs.writeFileSync(VERIFY_CORPUS_MJS, verifySource, "utf8");
    logPass(`Updated EXPECTED_COUNT in scripts/verify-corpus.mjs`);

    // Update build-fixtures-json.mjs fixtureCount guard
    const buildFixturesMjs = path.join(PROJECT_ROOT, "scripts", "build-fixtures-json.mjs");
    let buildSource = fs.readFileSync(buildFixturesMjs, "utf8");
    const buildBefore = buildSource;
    buildSource = buildSource.replace(
      /fixtureCount !== \d+/,
      `fixtureCount !== ${newCount}`
    );
    if (buildSource === buildBefore) {
      logFail(`fixtureCount guard pattern not found in scripts/build-fixtures-json.mjs — file may be stale`);
      process.exit(1);
    }
    fs.writeFileSync(buildFixturesMjs, buildSource, "utf8");
    logPass(`Updated fixtureCount guard in scripts/build-fixtures-json.mjs`);
  }
} else if (currentCount === newCount) {
  log(`\nCount unchanged: ${newCount} fixtures`);

  // Even when contract.ts is current, verify the other files haven't drifted
  if (!DRY_RUN) {
    let driftFixed = false;

    const verifySource = fs.readFileSync(VERIFY_CORPUS_MJS, "utf8");
    const verifyMatch = verifySource.match(/const EXPECTED_COUNT = (\d+);/);
    if (verifyMatch && parseInt(verifyMatch[1], 10) !== newCount) {
      fs.writeFileSync(VERIFY_CORPUS_MJS, verifySource.replace(/const EXPECTED_COUNT = \d+;/, `const EXPECTED_COUNT = ${newCount};`), "utf8");
      logPass(`Fixed drifted EXPECTED_COUNT in scripts/verify-corpus.mjs: ${verifyMatch[1]} -> ${newCount}`);
      driftFixed = true;
    }

    const buildFixturesMjs = path.join(PROJECT_ROOT, "scripts", "build-fixtures-json.mjs");
    const buildSource = fs.readFileSync(buildFixturesMjs, "utf8");
    const buildMatch = buildSource.match(/fixtureCount !== (\d+)/);
    if (buildMatch && parseInt(buildMatch[1], 10) !== newCount) {
      fs.writeFileSync(buildFixturesMjs, buildSource.replace(/fixtureCount !== \d+/, `fixtureCount !== ${newCount}`), "utf8");
      logPass(`Fixed drifted fixtureCount guard in scripts/build-fixtures-json.mjs: ${buildMatch[1]} -> ${newCount}`);
      driftFixed = true;
    }

    if (!driftFixed) {
      log(`All count literals in sync.`);
    }
  }
} else {
  log(`\nCount: ${newCount} fixtures (src/contract.ts not found, skipping CORPUS_SIZE update)`);
}

// ---------------------------------------------------------------------------
// Step 6: Rebuild fixtures.json
// ---------------------------------------------------------------------------

if (DRY_RUN) {
  log(`\n[DRY] Would run: node scripts/build-fixtures-json.mjs --qwik-dir ${QWIK_DIR}`);
} else {
  log(`\nRebuilding fixtures.json...`);
  try {
    execFileSync(process.execPath, [BUILD_FIXTURES_MJS, "--qwik-dir", QWIK_DIR], {
      stdio: "inherit",
      cwd: PROJECT_ROOT,
    });
    logPass(`fixtures.json rebuilt successfully`);
  } catch (err) {
    logFail(`build-fixtures-json.mjs failed (exit ${err.status ?? "?"})`);
    process.exit(1);
  }
}

// ---------------------------------------------------------------------------
// Step 7: Verify corpus
// ---------------------------------------------------------------------------

if (DRY_RUN) {
  log(`\n[DRY] Would run: node scripts/verify-corpus.mjs`);
} else {
  log(`\nVerifying corpus...`);
  try {
    execFileSync(process.execPath, [VERIFY_CORPUS_MJS], {
      stdio: "inherit",
      cwd: PROJECT_ROOT,
    });
    logPass(`verify-corpus.mjs passed`);
  } catch (err) {
    logFail(`verify-corpus.mjs failed (exit ${err.status ?? "?"})`);
    process.exit(1);
  }
}

// ---------------------------------------------------------------------------
// Step 8: Summary
// ---------------------------------------------------------------------------

log(`\n--- Sync Summary ---`);
log(`  Files copied/updated: ${copied}`);
log(`  Files unchanged:      ${unchanged}`);
log(`  Files pruned:         ${pruned}`);
if (currentCount !== null && currentCount !== newCount) {
  log(`  Count:                ${currentCount} -> ${newCount} (updated)`);
} else {
  log(`  Count:                ${newCount} (no change)`);
}
if (DRY_RUN) {
  log(`  Mode:                 DRY RUN — no files were written`);
} else {
  log(`  Fixtures rebuilt:     yes`);
  log(`  Corpus verified:      PASS`);
}
log(`--------------------`);
log(`Sync complete.`);
