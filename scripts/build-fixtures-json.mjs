#!/usr/bin/env node

/**
 * build-fixtures-json.mjs
 *
 * Extracts per-fixture TransformModulesOptions from SWC test.rs and produces
 * fixtures.json — the canonical manifest of all 201 fixtures.
 *
 * Usage: node scripts/build-fixtures-json.mjs [--qwik-dir <path>]
 *
 * Options:
 *   --qwik-dir <path>   Path to the root of the upstream qwik repository.
 *                       Falls back to QWIK_DIR env var, then ../qwik sibling directory.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, "..");

// ---------------------------------------------------------------------------
// Resolve the upstream qwik repository root directory
// ---------------------------------------------------------------------------

function resolveQwikDir() {
  // 1. --qwik-dir <path> CLI flag
  const argIdx = process.argv.indexOf("--qwik-dir");
  if (argIdx !== -1 && process.argv[argIdx + 1]) {
    return path.resolve(process.argv[argIdx + 1]);
  }

  // 2. QWIK_DIR environment variable
  if (process.env.QWIK_DIR) {
    return path.resolve(process.env.QWIK_DIR);
  }

  // 3. Fallback: ../qwik sibling directory
  return path.resolve(PROJECT_ROOT, "../qwik");
}

const QWIK_DIR = resolveQwikDir();
const SWC_TEST_RS = path.join(
  QWIK_DIR,
  "packages/optimizer/core/src/test.rs"
);

if (!fs.existsSync(SWC_TEST_RS)) {
  console.error(
    `[ERROR] Could not find test.rs at: ${SWC_TEST_RS}`
  );
  console.error(
    `  Resolved qwik-dir: ${QWIK_DIR}`
  );
  console.error(
    `  Pass the correct path with: --qwik-dir /path/to/qwik`
  );
  console.error(
    `  Or set the QWIK_DIR environment variable.`
  );
  process.exit(2);
}

const INPUTS_DIR = path.join(PROJECT_ROOT, "inputs");
const OUTPUT_FILE = path.join(PROJECT_ROOT, "fixtures.json");

// ---------------------------------------------------------------------------
// Default values matching TestInput::default() in test.rs lines 7222-7245
// ---------------------------------------------------------------------------
const DEFAULTS = {
  src_dir: "/user/qwik/src/",
  root_dir: null,
  entry_strategy: "Segment",
  minify: "Simplify",
  transpile_ts: false,
  transpile_jsx: false,
  preserve_filenames: false,
  explicit_extensions: false,
  mode: "Test",
  scope: null,
  core_module: null,
  strip_exports: null,
  strip_ctx_name: null,
  strip_event_handlers: false,
  reg_ctx_name: null,
  is_server: null,
  // filename is the TestInput.filename field → input path
  filename: "test.tsx",
  // dev_path is the TestInput.dev_path → input dev_path
  dev_path: null,
};

// ---------------------------------------------------------------------------
// Value parsers for Rust literals
// ---------------------------------------------------------------------------

/** Parse EntryStrategy::X -> "X" */
function parseEntryStrategy(val) {
  const m = val.match(/EntryStrategy::(\w+)/);
  return m ? m[1] : null;
}

/** Parse EmitMode::X -> "X" */
function parseEmitMode(val) {
  const m = val.match(/EmitMode::(\w+)/);
  return m ? m[1] : null;
}

/** Parse MinifyMode::X -> "X" */
function parseMinifyMode(val) {
  const m = val.match(/MinifyMode::(\w+)/);
  return m ? m[1] : null;
}

/** Parse Some("x") -> "x", Some(true) -> true, Some(false) -> false, None -> null */
function parseSomeOption(val) {
  const trimmed = val.trim();
  if (trimmed === "None") return null;
  const someStr = trimmed.match(/^Some\("([^"]*)"\)/);
  if (someStr) return someStr[1];
  const someBool = trimmed.match(/^Some\((true|false)\)/);
  if (someBool) return someBool[1] === "true";
  const somePath = trimmed.match(/^Some\("([^"]*)"\s*\.into\(\)\)/);
  if (somePath) return somePath[1];
  // Some("/path/to/app/".into()) pattern
  const someInto = trimmed.match(/^Some\(([^)]+)\.into\(\)\)/);
  if (someInto) {
    const inner = someInto[1].trim();
    const strVal = inner.match(/^"([^"]*)"/);
    if (strVal) return strVal[1];
  }
  return null;
}

/** Parse vec!["a", "b"] -> ["a", "b"] or Some(vec![...]) -> ["a", "b"] */
function parseVecOption(val) {
  const trimmed = val.trim();
  if (trimmed === "None") return null;
  // Some(vec!["x".into(), ...])
  const vecContent = trimmed.match(/vec!\[([^\]]*)\]/);
  if (!vecContent) return null;
  const items = vecContent[1]
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean)
    .map((s) => {
      // "x".into() or "x"
      const strMatch = s.match(/"([^"]*)"/);
      return strMatch ? strMatch[1] : s;
    });
  return items.length > 0 ? items : null;
}

/** Parse a string literal "foo" -> "foo" or "foo".to_string() -> "foo" */
function parseStringLiteral(val) {
  const m = val.match(/"([^"]*)"/);
  return m ? m[1] : null;
}

/** Parse true/false -> boolean */
function parseBool(val) {
  if (val.trim() === "true") return true;
  if (val.trim() === "false") return false;
  return null;
}

// ---------------------------------------------------------------------------
// Extract a field's value from the search region using lookahead
// ---------------------------------------------------------------------------

/**
 * Extract the value text for a named field in a Rust struct literal.
 * Finds `fieldName: <value>` and returns the value up to the next comma/newline
 * that's not inside brackets or parens.
 */
function extractFieldValue(region, fieldName) {
  // Match field name followed by colon, handling leading whitespace/newline
  const pattern = new RegExp(`\\b${fieldName}\\s*:\\s*`, "m");
  const m = pattern.exec(region);
  if (!m) return null;

  const start = m.index + m[0].length;
  let depth = 0;
  let i = start;
  const end = Math.min(region.length, start + 2000);

  while (i < end) {
    const ch = region[i];
    if (ch === "(" || ch === "[" || ch === "{") {
      depth++;
    } else if (ch === ")" || ch === "]" || ch === "}") {
      if (depth === 0) break;
      depth--;
    } else if (ch === "," && depth === 0) {
      break;
    } else if (ch === "\n" && depth === 0) {
      // Check if next non-whitespace is a continuation
      const rest = region.slice(i + 1).trimStart();
      if (rest.startsWith("..")) break; // spread operator
    }
    i++;
  }

  return region.slice(start, i).trim();
}

// ---------------------------------------------------------------------------
// Parse all non-default overrides from a TestInput struct literal
// ---------------------------------------------------------------------------

function parseTestInputOverrides(region) {
  const overrides = {};

  // entry_strategy
  const esVal = extractFieldValue(region, "entry_strategy");
  if (esVal) {
    const parsed = parseEntryStrategy(esVal);
    if (parsed && parsed !== DEFAULTS.entry_strategy) {
      overrides.entry_strategy = parsed;
    }
  }

  // minify
  const minVal = extractFieldValue(region, "minify");
  if (minVal) {
    const parsed = parseMinifyMode(minVal);
    if (parsed && parsed !== DEFAULTS.minify) {
      overrides.minify = parsed;
    }
  }

  // mode
  const modeVal = extractFieldValue(region, "mode");
  if (modeVal) {
    const parsed = parseEmitMode(modeVal);
    if (parsed && parsed !== DEFAULTS.mode) {
      overrides.mode = parsed;
    }
  }

  // transpile_ts
  const tsVal = extractFieldValue(region, "transpile_ts");
  if (tsVal !== null) {
    const parsed = parseBool(tsVal);
    if (parsed !== null && parsed !== DEFAULTS.transpile_ts) {
      overrides.transpile_ts = parsed;
    }
  }

  // transpile_jsx
  const jsxVal = extractFieldValue(region, "transpile_jsx");
  if (jsxVal !== null) {
    const parsed = parseBool(jsxVal);
    if (parsed !== null && parsed !== DEFAULTS.transpile_jsx) {
      overrides.transpile_jsx = parsed;
    }
  }

  // preserve_filenames
  const pfVal = extractFieldValue(region, "preserve_filenames");
  if (pfVal !== null) {
    const parsed = parseBool(pfVal);
    if (parsed !== null && parsed !== DEFAULTS.preserve_filenames) {
      overrides.preserve_filenames = parsed;
    }
  }

  // explicit_extensions
  const eeVal = extractFieldValue(region, "explicit_extensions");
  if (eeVal !== null) {
    const parsed = parseBool(eeVal);
    if (parsed !== null && parsed !== DEFAULTS.explicit_extensions) {
      overrides.explicit_extensions = parsed;
    }
  }

  // strip_event_handlers
  const sehVal = extractFieldValue(region, "strip_event_handlers");
  if (sehVal !== null) {
    const parsed = parseBool(sehVal);
    if (parsed !== null && parsed !== DEFAULTS.strip_event_handlers) {
      overrides.strip_event_handlers = parsed;
    }
  }

  // scope
  const scopeVal = extractFieldValue(region, "scope");
  if (scopeVal !== null) {
    const parsed = parseSomeOption(scopeVal);
    if (parsed !== null) {
      overrides.scope = parsed;
    }
  }

  // core_module
  const cmVal = extractFieldValue(region, "core_module");
  if (cmVal !== null) {
    const parsed = parseSomeOption(cmVal);
    if (parsed !== null) {
      overrides.core_module = parsed;
    }
  }

  // is_server
  const isServerVal = extractFieldValue(region, "is_server");
  if (isServerVal !== null) {
    const parsed = parseSomeOption(isServerVal);
    if (parsed !== null) {
      overrides.is_server = parsed;
    }
  }

  // strip_exports — vec pattern inside Some(...)
  const seVal = extractFieldValue(region, "strip_exports");
  if (seVal !== null && seVal.trim() !== "None") {
    const parsed = parseVecOption(seVal);
    if (parsed !== null) {
      overrides.strip_exports = parsed;
    }
  }

  // strip_ctx_name
  const scnVal = extractFieldValue(region, "strip_ctx_name");
  if (scnVal !== null && scnVal.trim() !== "None") {
    const parsed = parseVecOption(scnVal);
    if (parsed !== null) {
      overrides.strip_ctx_name = parsed;
    }
  }

  // reg_ctx_name
  const rcnVal = extractFieldValue(region, "reg_ctx_name");
  if (rcnVal !== null && rcnVal.trim() !== "None") {
    const parsed = parseVecOption(rcnVal);
    if (parsed !== null) {
      overrides.reg_ctx_name = parsed;
    }
  }

  // filename (affects input path)
  const fnameVal = extractFieldValue(region, "filename");
  if (fnameVal !== null) {
    const parsed = parseStringLiteral(fnameVal);
    if (parsed && parsed !== DEFAULTS.filename) {
      overrides.filename = parsed;
    }
  }

  // dev_path
  const dpVal = extractFieldValue(region, "dev_path");
  if (dpVal !== null && dpVal.trim() !== "None") {
    const parsed = parseSomeOption(dpVal);
    if (parsed !== null) {
      overrides.dev_path = parsed;
    }
  }

  // src_dir (only relevant in non-standard fixtures)
  const srcDirVal = extractFieldValue(region, "src_dir");
  if (srcDirVal !== null) {
    const parsed = parseStringLiteral(srcDirVal);
    if (parsed && parsed !== DEFAULTS.src_dir) {
      overrides.src_dir = parsed;
    }
  }

  // root_dir (only relevant in non-standard fixtures)
  const rootDirVal = extractFieldValue(region, "root_dir");
  if (rootDirVal !== null && rootDirVal.trim() !== "None") {
    const parsed = parseSomeOption(rootDirVal);
    if (parsed !== null) {
      overrides.root_dir = parsed;
    }
  }

  return overrides;
}

// ---------------------------------------------------------------------------
// Build a fixture config from overrides + defaults
// ---------------------------------------------------------------------------

function buildFixtureConfig(overrides, code) {
  const cfg = {
    src_dir: overrides.src_dir ?? DEFAULTS.src_dir,
    root_dir: overrides.root_dir ?? DEFAULTS.root_dir,
    source_maps: true, // always true — hardcoded in test_input_fn
    minify: overrides.minify ?? DEFAULTS.minify,
    transpile_ts: overrides.transpile_ts ?? DEFAULTS.transpile_ts,
    transpile_jsx: overrides.transpile_jsx ?? DEFAULTS.transpile_jsx,
    preserve_filenames:
      overrides.preserve_filenames ?? DEFAULTS.preserve_filenames,
    explicit_extensions:
      overrides.explicit_extensions ?? DEFAULTS.explicit_extensions,
    entry_strategy: overrides.entry_strategy ?? DEFAULTS.entry_strategy,
    mode: overrides.mode ?? DEFAULTS.mode,
    scope: overrides.scope ?? DEFAULTS.scope,
    core_module: overrides.core_module ?? DEFAULTS.core_module,
    strip_exports: overrides.strip_exports ?? DEFAULTS.strip_exports,
    strip_ctx_name: overrides.strip_ctx_name ?? DEFAULTS.strip_ctx_name,
    strip_event_handlers:
      overrides.strip_event_handlers ?? DEFAULTS.strip_event_handlers,
    reg_ctx_name: overrides.reg_ctx_name ?? DEFAULTS.reg_ctx_name,
    is_server: overrides.is_server ?? DEFAULTS.is_server,
    inputs: [
      {
        path: overrides.filename ?? DEFAULTS.filename,
        dev_path: overrides.dev_path ?? DEFAULTS.dev_path,
        code,
      },
    ],
  };
  return cfg;
}

// ---------------------------------------------------------------------------
// Main extraction logic
// ---------------------------------------------------------------------------

function main() {
  const source = fs.readFileSync(SWC_TEST_RS, "utf8");
  const inputFiles = new Set(fs.readdirSync(INPUTS_DIR));

  const fixtures = {};
  const nonDefaultSummary = {};

  // ---------------------------------------------------------------------------
  // Special case: relative_paths — multi-input fixture
  // Handled entirely from test.rs data captured here.
  // ---------------------------------------------------------------------------
  const relativePaths = {
    src_dir: "/path/to/app/src/thing",
    root_dir: "/path/to/app/",
    source_maps: true,
    minify: "Simplify",
    transpile_ts: true,
    transpile_jsx: true,
    preserve_filenames: false,
    explicit_extensions: true,
    entry_strategy: "Segment",
    mode: "Test",
    scope: null,
    core_module: null,
    strip_exports: null,
    strip_ctx_name: null,
    strip_event_handlers: false,
    reg_ctx_name: null,
    is_server: null,
    inputs: [
      {
        path: "../../node_modules/dep/dist/lib.mjs",
        dev_path: null,
        // dep code extracted from test.rs lines 3336-3372
        code: extractRelativePathsDepCode(source),
      },
      {
        path: "components/main.tsx",
        dev_path: null,
        code: fs.readFileSync(
          path.join(INPUTS_DIR, "relative_paths.tsx"),
          "utf8"
        ),
      },
    ],
  };

  // ---------------------------------------------------------------------------
  // Collect all test function positions first for proper boundary detection
  // ---------------------------------------------------------------------------
  const fnRegex = /^fn (\w+)\(\) \{/gm;
  const allFunctions = [];
  let fmatch;
  while ((fmatch = fnRegex.exec(source)) !== null) {
    allFunctions.push({ name: fmatch[1], index: fmatch.index });
  }

  let extracted = 0;
  let skipped = 0;
  const skippedNames = [];

  for (let fi = 0; fi < allFunctions.length; fi++) {
    const { name: fnName, index: fnIndex } = allFunctions[fi];

    // Skip utility functions and non-test helpers
    if (fnName === "test_input_fn") continue;

    // Special case: relative_paths is handled separately above
    if (fnName === "relative_paths") {
      fixtures[fnName] = relativePaths;
      extracted++;
      continue;
    }

    // Determine the end of this function body (start of next function or end of file)
    const nextFnIndex =
      fi + 1 < allFunctions.length ? allFunctions[fi + 1].index : source.length;
    const fnBody = source.slice(fnIndex, nextFnIndex);

    // Check if snapshot: false — if so, this test doesn't produce a snapshot
    // Must only check within this function's body (not the next one)
    if (/\bsnapshot\s*:\s*false\b/.test(fnBody)) {
      skipped++;
      skippedNames.push(`${fnName} (snapshot:false)`);
      continue;
    }

    // Check if this test function has a corresponding .tsx input file
    const inputFile = `${fnName}.tsx`;
    if (!inputFiles.has(inputFile)) {
      skipped++;
      skippedNames.push(`${fnName} (no input file)`);
      continue;
    }

    // Find the TestInput struct literal region within the function body
    const structMatch = fnBody.match(/TestInput\s*\{/);
    if (!structMatch) {
      // No TestInput struct, could be a direct transform call — skip for now
      skipped++;
      skippedNames.push(`${fnName} (no TestInput struct)`);
      continue;
    }

    // Extract the struct fields region bounded to the function body only.
    // Find the end of the TestInput struct by locating `..TestInput::default()` or `})`.
    // Use fnBody to stay within bounds.
    const structBodyStart = structMatch.index + structMatch[0].length;
    // Find closing of the struct by finding `..TestInput::default()` spread or `}`
    // We'll use the full function body slice from structBodyStart forward, but
    // cap at a reasonable distance or at the `..TestInput::default()` spread.
    let structEnd = fnBody.length;
    const spreadIdx = fnBody.indexOf("..TestInput::default()", structBodyStart);
    if (spreadIdx !== -1) {
      structEnd = spreadIdx + "..TestInput::default()".length + 10;
    }
    const structRegion = fnBody.slice(structBodyStart, structEnd);

    // Parse overrides
    const overrides = parseTestInputOverrides(structRegion);

    // Read the code from inputs/<name>.tsx
    const code = fs.readFileSync(path.join(INPUTS_DIR, inputFile), "utf8");

    // Build fixture config
    const cfg = buildFixtureConfig(overrides, code);
    fixtures[fnName] = cfg;
    extracted++;

    // Track which non-default fields were used
    const nonDefaults = Object.keys(overrides).filter(
      (k) => k !== "filename" && k !== "dev_path"
    );
    if (nonDefaults.length > 0) {
      for (const field of nonDefaults) {
        if (!nonDefaultSummary[field]) nonDefaultSummary[field] = [];
        nonDefaultSummary[field].push(fnName);
      }
    }
  }

  // ---------------------------------------------------------------------------
  // Write output
  // ---------------------------------------------------------------------------
  const output = {
    version: 1,
    fixtures,
  };

  fs.writeFileSync(OUTPUT_FILE, JSON.stringify(output, null, 2) + "\n", "utf8");

  // ---------------------------------------------------------------------------
  // Summary report
  // ---------------------------------------------------------------------------
  const fixtureCount = Object.keys(fixtures).length;
  console.log(`\nfixtures.json written to: ${OUTPUT_FILE}`);
  console.log(`Total fixtures extracted: ${fixtureCount}`);
  console.log(`Skipped: ${skipped}`);

  if (skippedNames.length > 0) {
    console.log(`\nSkipped functions:`);
    for (const n of skippedNames) {
      console.log(`  - ${n}`);
    }
  }

  const withNonDefaults = new Set(Object.values(nonDefaultSummary).flat()).size;
  console.log(
    `\nFixtures with non-default settings: ${withNonDefaults} of ${fixtureCount}`
  );

  if (Object.keys(nonDefaultSummary).length > 0) {
    console.log(`\nNon-default field usage:`);
    for (const [field, names] of Object.entries(nonDefaultSummary).sort()) {
      console.log(`  ${field} (${names.length}): ${names.slice(0, 5).join(", ")}${names.length > 5 ? ", ..." : ""}`);
    }
  }

  if (fixtureCount !== 201) {
    console.error(
      `\nWARNING: Expected 201 fixtures, got ${fixtureCount}. Check skipped list.`
    );
    process.exit(1);
  }

  console.log(`\nAll ${fixtureCount} fixtures extracted successfully.`);
}

// ---------------------------------------------------------------------------
// Extract the dep code from the relative_paths function in test.rs
// ---------------------------------------------------------------------------
function extractRelativePathsDepCode(source) {
  // Find the relative_paths function
  const fnMatch = /^fn relative_paths\(\) \{/m.exec(source);
  if (!fnMatch) {
    throw new Error("Could not find relative_paths function in test.rs");
  }

  // The dep variable is a raw string: let dep = r#"..."#;
  const fnRegion = source.slice(fnMatch.index, fnMatch.index + 5000);
  const depMatch = fnRegion.match(/let dep = r(#+)"/);
  if (!depMatch) {
    throw new Error("Could not find dep raw string in relative_paths");
  }

  const hashes = depMatch[1];
  const depCodeStart = fnMatch.index + depMatch.index + depMatch[0].length;
  const closeToken = `"${hashes}`;
  const closePos = source.indexOf(closeToken, depCodeStart);
  if (closePos === -1) {
    throw new Error("Could not find closing delimiter for dep raw string");
  }

  return source.slice(depCodeStart, closePos);
}

main();
