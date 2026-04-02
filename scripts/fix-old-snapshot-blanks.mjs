#!/usr/bin/env node

/**
 * Fix blank line formatting in old (SWC) snapshots to match new (OXC) format.
 *
 * Changes applied:
 * 1. Add blank line before section headers (between code/metadata end and next == / === header)
 * 2. Add blank line before metadata blocks (between last code line and /*)
 * 3. Do NOT add a blank line between frontmatter --- and === INPUT ===
 * 4. Remove trailing blank lines at end of file
 */

import fs from "node:fs";
import path from "node:path";

const SNAP_DIR = "crates/swc-optimizer/core/src/snapshots";

const SECTION_HEADER_RE = /^={2,}\s+.*\s+={2,}/;
const METADATA_START_RE = /^\/\*$/;

function processSnapshot(filePath) {
  const original = fs.readFileSync(filePath, "utf8");
  const lines = original.split("\n");

  // Strip trailing empty line if file ended with \n
  if (lines.length > 0 && lines[lines.length - 1] === "") {
    lines.pop();
  }

  // Rebuild with proper blank lines
  const out = [];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    // Before a section header, ensure blank line (but not right after frontmatter ---)
    if (SECTION_HEADER_RE.test(line) && out.length > 0) {
      if (out[out.length - 1] !== "" && out[out.length - 1] !== "---") {
        out.push("");
      }
      out.push(line);
      continue;
    }

    // Before a metadata block /*, ensure blank line
    if (METADATA_START_RE.test(line) && out.length > 0) {
      if (out[out.length - 1] !== "") {
        out.push("");
      }
      out.push(line);
      continue;
    }

    out.push(line);
  }

  // Remove trailing blank lines
  while (out.length > 0 && out[out.length - 1] === "") {
    out.pop();
  }

  // Ensure file ends with newline
  const result = out.join("\n") + "\n";

  if (result !== original) {
    fs.writeFileSync(filePath, result, "utf8");
    return true;
  }
  return false;
}

const dir = path.resolve(process.cwd(), SNAP_DIR);
const files = fs
  .readdirSync(dir)
  .filter((f) => f.endsWith(".snap"))
  .map((f) => path.join(dir, f))
  .sort();

let changed = 0;
for (const file of files) {
  if (processSnapshot(file)) {
    changed++;
  }
}

console.log(`Processed ${files.length} snapshots, updated ${changed}`);
