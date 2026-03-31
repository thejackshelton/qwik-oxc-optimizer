#!/usr/bin/env node

/**
 * Copy body content (everything after frontmatter) from old (SWC) snapshots
 * into corresponding new (OXC) snapshots, preserving the new frontmatter.
 */

import fs from "node:fs";
import path from "node:path";

const OLD_DIR = "crates/swc-optimizer/core/src/snapshots";
const NEW_DIR = "crates/qwik-optimizer-oxc/tests/snapshots";

const OLD_PREFIX = "qwik_core__test__";

function splitFrontmatter(content) {
  // Frontmatter is between first --- and second ---
  const lines = content.split("\n");
  let fmEnd = -1;
  let dashCount = 0;
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].trimEnd() === "---") {
      dashCount++;
      if (dashCount === 2) {
        fmEnd = i;
        break;
      }
    }
  }
  if (fmEnd < 0) {
    return { frontmatter: "", body: content };
  }
  const frontmatter = lines.slice(0, fmEnd + 1).join("\n");
  const body = lines.slice(fmEnd + 1).join("\n");
  return { frontmatter, body };
}

const oldDir = path.resolve(process.cwd(), OLD_DIR);
const newDir = path.resolve(process.cwd(), NEW_DIR);

const oldFiles = fs
  .readdirSync(oldDir)
  .filter((f) => f.endsWith(".snap") && f.startsWith(OLD_PREFIX));

let copied = 0;
let skipped = 0;

for (const oldFile of oldFiles) {
  const testName = oldFile.slice(OLD_PREFIX.length, -".snap".length);
  const newFile = testName + ".snap";
  const newPath = path.join(newDir, newFile);

  if (!fs.existsSync(newPath)) {
    skipped++;
    continue;
  }

  const oldContent = fs.readFileSync(path.join(oldDir, oldFile), "utf8");
  const newContent = fs.readFileSync(newPath, "utf8");

  const oldParts = splitFrontmatter(oldContent);
  const newParts = splitFrontmatter(newContent);

  const result = newParts.frontmatter + "\n" + oldParts.body;

  if (result !== newContent) {
    fs.writeFileSync(newPath, result, "utf8");
    copied++;
  }
}

console.log(
  `Copied ${copied} snapshot bodies, skipped ${skipped} (no matching new file)`
);
