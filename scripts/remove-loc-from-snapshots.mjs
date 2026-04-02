#!/usr/bin/env node

/**
 * Remove "loc" fields from JSON metadata blocks in old SWC optimizer snapshots.
 *
 * The loc field always appears as:
 *   "loc": [
 *     NN,
 *     NN
 *   ]
 * optionally followed by a comma. It may also be preceded by a trailing comma
 * on the previous field when loc is the last entry.
 */

import fs from "node:fs";
import path from "node:path";

const DEFAULT_TARGET = "crates/swc-optimizer/core/src/snapshots";

function main() {
  const args = process.argv.slice(2);
  const dryRun = args.includes("--dry-run");
  const positional = args.filter((a) => !a.startsWith("--"));
  const target = positional[0] || DEFAULT_TARGET;
  const abs = path.resolve(process.cwd(), target);

  const files = [];
  if (fs.statSync(abs).isDirectory()) {
    for (const entry of fs.readdirSync(abs)) {
      if (entry.endsWith(".snap")) files.push(path.join(abs, entry));
    }
  } else {
    files.push(abs);
  }
  files.sort();

  let changedFiles = 0;
  let removedFields = 0;

  for (const file of files) {
    const original = fs.readFileSync(file, "utf8");

    // Remove loc when it's the last field (preceded by comma on prev line):
    //   ,\n  "loc": [\n    N,\n    N\n  ]
    // Remove loc when it's not the last field (followed by comma):
    //   "loc": [\n    N,\n    N\n  ],\n
    let result = original;
    let count = 0;

    // Pattern 1: loc is the last field — remove the preceding comma too
    result = result.replace(/,\n(\s*"loc":\s*\[\n\s*\d+,\n\s*\d+\n\s*\])/g, () => {
      count++;
      return "";
    });

    // Pattern 2: loc is not the last field — has trailing comma
    result = result.replace(/\s*"loc":\s*\[\n\s*\d+,\n\s*\d+\n\s*\],?\n/g, () => {
      count++;
      return "\n";
    });

    if (count > 0) {
      removedFields += count;
      changedFiles++;
      if (!dryRun) {
        fs.writeFileSync(file, result, "utf8");
      }
    }
  }

  const mode = dryRun ? "dry-run" : "write";
  console.log(
    `[${mode}] ${files.length} files scanned, ${changedFiles} changed, ${removedFields} loc fields removed`
  );
}

main();
