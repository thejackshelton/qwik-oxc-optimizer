#!/usr/bin/env node

/**
 * Remove source map blocks from old SWC optimizer snapshot files.
 *
 * Source maps appear in two formats:
 *   1. Single-line:  Some("{\"version\":3,...}")
 *   2. Multi-line:   Some(\n  '...',\n);
 *
 * Both formats appear between code output and the /* metadata block,
 * surrounded by blank lines.
 */

import fs from "node:fs";
import path from "node:path";

const DEFAULT_TARGET = "crates/swc-optimizer/core/src/snapshots";

function main() {
  const args = process.argv.slice(2);
  const dryRun = args.includes("--dry-run");
  const checkMode = args.includes("--check");

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
  let removedBlocks = 0;

  for (const file of files) {
    const original = fs.readFileSync(file, "utf8");
    const lines = original.split("\n");
    const out = [];
    let removed = 0;
    let i = 0;

    while (i < lines.length) {
      const line = lines[i];

      // Single-line: Some("...") or Some("{...}")
      if (/^Some\(["']/.test(line.trimStart()) || line.trimStart() === "Some(") {
        // Multi-line Some( block
        if (line.trimStart() === "Some(") {
          // Consume until closing );
          let j = i + 1;
          while (j < lines.length && !lines[j].trimStart().startsWith(");")) {
            j++;
          }
          if (j < lines.length) j++; // skip the ); line

          // Remove surrounding blank lines
          while (out.length > 0 && out[out.length - 1].trim() === "") {
            out.pop();
          }
          while (j < lines.length && lines[j].trim() === "") {
            j++;
          }
          i = j;
          removed++;
          continue;
        }

        // Single-line Some("...")
        let j = i + 1;
        // Remove surrounding blank lines
        while (out.length > 0 && out[out.length - 1].trim() === "") {
          out.pop();
        }
        while (j < lines.length && lines[j].trim() === "") {
          j++;
        }
        i = j;
        removed++;
        continue;
      }

      out.push(line);
      i++;
    }

    if (removed === 0) continue;

    removedBlocks += removed;
    changedFiles++;

    const result = out.join("\n");
    const rel = path.relative(process.cwd(), file);

    if (dryRun) {
      console.log(`[dry-run] ${rel}: removed ${removed} source map blocks`);
    } else if (!checkMode) {
      fs.writeFileSync(file, result, "utf8");
    }
  }

  const mode = checkMode ? "check" : dryRun ? "dry-run" : "write";
  console.log(
    `[${mode}] ${files.length} files scanned, ${changedFiles} changed, ${removedBlocks} source map blocks removed`
  );

  if (checkMode && changedFiles > 0) process.exit(1);
}

main();
