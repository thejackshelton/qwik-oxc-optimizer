import * as fs from "node:fs";
import * as path from "node:path";
import { parseSnapFile } from "../src/parser.js";
import { compareFixture } from "../src/comparator.js";

const SWC_DIR = path.resolve("swc-snapshots");
const OXC_DIR = path.resolve("oxc-snapshots");

const swcFiles = fs.readdirSync(SWC_DIR).filter(f => f.endsWith(".snap")).sort();
let count = 0;
const extraImportTypes = new Map<string, number>();

for (const file of swcFiles) {
  const oxcPath = path.join(OXC_DIR, file);
  if (!fs.existsSync(oxcPath)) continue;
  const swcSnap = parseSnapFile(path.join(SWC_DIR, file));
  const oxcSnap = parseSnapFile(oxcPath);
  const failures = compareFixture(swcSnap, oxcSnap);
  for (const f of failures) {
    if (f.category !== "code_diff" || f.field !== "parentModule") continue;
    const expected = String(f.expected ?? "");
    const actual = String(f.actual ?? "");
    const swcImports = (expected.match(/^import\s.+$/gm) || []).map(s => s.trim());
    const oxcImports = (actual.match(/^import\s.+$/gm) || []).map(s => s.trim());
    if (oxcImports.length > swcImports.length) {
      count++;
      const swcSet = new Set(swcImports);
      const extras = oxcImports.filter(i => !swcSet.has(i));
      for (const e of extras) {
        extraImportTypes.set(e, (extraImportTypes.get(e) || 0) + 1);
      }
      if (count <= 3) {
        console.log(`\n--- ${file.replace('.snap','')} ---`);
        console.log("SWC:", swcImports);
        console.log("OXC:", oxcImports);
        console.log("EXTRA:", extras);
      }
    }
  }
}
console.log(`\nTotal parent modules with extra OXC imports: ${count}`);
console.log("\nMost common extra imports:");
[...extraImportTypes.entries()]
  .sort((a, b) => b[1] - a[1])
  .slice(0, 15)
  .forEach(([imp, c]) => console.log(`  ${c}x: ${imp}`));
