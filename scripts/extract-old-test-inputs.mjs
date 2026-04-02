#!/usr/bin/env node

/**
 * Extracts inline code blocks from the old SWC optimizer test.rs
 * and writes them to the new optimizer's tests/input/*.tsx files,
 * preserving the original formatting exactly as-is.
 */

import fs from "node:fs";
import path from "node:path";

const OLD_TEST_RS = "crates/swc-optimizer/core/src/test.rs";
const NEW_INPUT_DIR = "crates/qwik-optimizer-oxc/tests/input";

function main() {
  const args = process.argv.slice(2);
  const dryRun = args.includes("--dry-run");

  const source = fs.readFileSync(OLD_TEST_RS, "utf8");
  const inputDir = path.resolve(NEW_INPUT_DIR);

  // Match each test function and extract its code block
  // Pattern: fn <name>() { ... code: r#"..."# ... }
  const fnRegex = /^fn (\w+)\(\) \{/gm;
  let match;
  let extracted = 0;
  let skipped = 0;
  const results = [];

  while ((match = fnRegex.exec(source)) !== null) {
    const fnName = match[1];
    if (fnName === "test_input_fn") continue;

    const fnStart = match.index + match[0].length;

    // Find the code: r#"..."# block within this test function
    const searchRegion = source.slice(fnStart, fnStart + 10000);
    const codeMatch = searchRegion.match(/code:\s*r(#+)"/);
    if (!codeMatch) {
      skipped++;
      continue;
    }

    const hashes = codeMatch[1];
    const codeStart = fnStart + codeMatch.index + codeMatch[0].length;
    const closeToken = `"${hashes}`;
    const closePos = source.indexOf(closeToken, codeStart);
    if (closePos === -1) {
      console.warn(`[warn] unclosed raw string for ${fnName}`);
      skipped++;
      continue;
    }

    const code = source.slice(codeStart, closePos);

    // Detect the filename extension from the test config
    const configRegion = source.slice(closePos, closePos + 500);
    const filenameMatch = configRegion.match(/filename:\s*"([^"]+)"/);
    let ext = "tsx";
    if (filenameMatch) {
      const fnExt = path.extname(filenameMatch[1]).slice(1).toLowerCase();
      if (["ts", "tsx", "js", "jsx", "mjs", "cjs"].includes(fnExt)) {
        ext = fnExt;
      }
    }

    const outFile = path.join(inputDir, `${fnName}.${ext}`);
    results.push({ fnName, outFile, code, ext });
    extracted++;
  }

  // Check which files exist in the target directory
  const existingFiles = new Set(fs.readdirSync(inputDir));

  let written = 0;
  let noMatch = 0;

  for (const { fnName, outFile, code, ext } of results) {
    const basename = path.basename(outFile);
    if (!existingFiles.has(basename)) {
      // Try with .tsx if the detected extension didn't match
      const altBasename = `${fnName}.tsx`;
      if (existingFiles.has(altBasename)) {
        const altFile = path.join(inputDir, altBasename);
        if (dryRun) {
          console.log(`[dry-run] would write ${altBasename}`);
        } else {
          fs.writeFileSync(altFile, code, "utf8");
        }
        written++;
        continue;
      }
      console.log(`[skip] no matching input file for: ${fnName} (tried ${basename})`);
      noMatch++;
      continue;
    }

    if (dryRun) {
      console.log(`[dry-run] would write ${basename}`);
    } else {
      fs.writeFileSync(outFile, code, "utf8");
    }
    written++;
  }

  console.log(`\nExtracted ${extracted} code blocks from old test.rs`);
  console.log(`Written: ${written}, Skipped: ${skipped}, No match: ${noMatch}`);
  if (dryRun) {
    console.log("(dry-run mode — no files were modified)");
  }
}

main();
