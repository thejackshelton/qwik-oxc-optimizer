#!/usr/bin/env node

/**
 * Replace all segment hashes in snapshot files with a fixed placeholder.
 *
 * Hashes are discovered from `"hash": "..."` in JSON metadata blocks,
 * then every occurrence of each hash in the file is replaced with
 * XXXXXXXXXXXX.
 */

import fs from "node:fs";
import path from "node:path";

const DEFAULT_TARGETS = [
  "crates/swc-optimizer/core/src/snapshots",
  "crates/qwik-optimizer-oxc/tests/snapshots",
];

function printHelp() {
  console.log(`Replace segment hashes in snapshot files with XXXXXXXXXXXX placeholder.

Usage:
  node scripts/replace-hashes-in-snapshots.mjs [--check] [--dry-run] [paths...]

Options:
  --check     Exit with code 1 if any file would change.
  --dry-run   Show what would change without writing.
  --help      Show this help message.

Default paths:
  ${DEFAULT_TARGETS.join("\n  ")}
`);
}

function collectSnapFiles(targets) {
  const files = [];
  for (const target of targets) {
    const abs = path.resolve(process.cwd(), target);
    if (!fs.existsSync(abs)) {
      console.warn(`[warn] path does not exist: ${target}`);
      continue;
    }
    const stat = fs.statSync(abs);
    if (stat.isDirectory()) {
      for (const entry of fs.readdirSync(abs)) {
        if (entry.endsWith(".snap")) {
          files.push(path.join(abs, entry));
        }
      }
    } else if (abs.endsWith(".snap")) {
      files.push(abs);
    }
  }
  files.sort();
  return files;
}

function processSnapshot(content) {
  // Extract hashes from metadata blocks: "hash": "XXXXXXXXXXX"
  const hashRegex = /"hash":\s*"([A-Za-z0-9_-]+)"/g;
  const hashes = [];
  const seen = new Set();
  let match;

  while ((match = hashRegex.exec(content)) !== null) {
    const hash = match[1];
    if (!seen.has(hash)) {
      seen.add(hash);
      hashes.push(hash);
    }
  }

  if (hashes.length === 0) {
    return { content, changed: false, hashCount: 0 };
  }

  // Replace all occurrences with a single fixed placeholder
  let result = content;
  for (const hash of hashes) {
    result = result.split(hash).join("XXXXXXXXXXXX");
  }

  return { content: result, changed: result !== content, hashCount: hashes.length };
}

function main() {
  const args = process.argv.slice(2);
  const checkMode = args.includes("--check");
  const dryRun = args.includes("--dry-run");
  const helpMode = args.includes("--help") || args.includes("-h");

  if (helpMode) {
    printHelp();
    process.exit(0);
  }

  const positional = args.filter((a) => !a.startsWith("--"));
  const targets = positional.length > 0 ? positional : DEFAULT_TARGETS;
  const files = collectSnapFiles(targets);

  if (files.length === 0) {
    console.error("No .snap files found.");
    process.exit(2);
  }

  let changedFiles = 0;
  let totalHashes = 0;

  for (const file of files) {
    const original = fs.readFileSync(file, "utf8");
    const result = processSnapshot(original);
    totalHashes += result.hashCount;

    if (!result.changed) continue;

    changedFiles++;
    const rel = path.relative(process.cwd(), file);

    if (dryRun) {
      console.log(`[dry-run] ${rel}: ${result.hashCount} hashes`);
    } else if (!checkMode) {
      fs.writeFileSync(file, result.content, "utf8");
    }
  }

  const mode = checkMode ? "check" : dryRun ? "dry-run" : "write";
  console.log(
    `[${mode}] ${files.length} files scanned, ${changedFiles} changed, ${totalHashes} hashes replaced`
  );

  if (checkMode && changedFiles > 0) {
    process.exit(1);
  }
}

main();
