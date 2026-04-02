#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const DEFAULT_TARGETS = [
  "crates/swc-optimizer/core/src/snapshots",
  "crates/qwik-optimizer-oxc/tests/snapshots",
];

const SECTION_HEADER_RE = /^==+\s*(.*?)\s*==+(?:\s*\(.*\))?\s*$/;
const INPUT_SECTION = "INPUT";
const DIAGNOSTICS_KEYWORD = "DIAGNOSTICS";

const SUPPORTED_EXTENSIONS = new Set([
  ".js",
  ".jsx",
  ".mjs",
  ".cjs",
  ".ts",
  ".tsx",
  ".mts",
  ".cts",
]);

function printHelp() {
  console.log(`Format code sections inside optimizer snapshot files using oxfmt.

Usage:
  node scripts/format-snapshot-code-blocks.mjs [--check] [paths...]

Options:
  --check         Check mode. Exits with code 1 if any snapshot would change.
  --help          Show this help message.

Environment:
  OXFMT_CMD       Formatter command used for stdin formatting.
                  Default: ./node_modules/.bin/oxfmt (if installed), else npx --yes oxfmt

Default paths:
  ${DEFAULT_TARGETS.join("\n  ")}
`);
}

function shellQuote(value) {
  return `'${String(value).replace(/'/g, `'"'"'`)}'`;
}

function runFormatter(formatterCmd, code, stdinFilepath) {
  const cmd = `${formatterCmd} --stdin-filepath ${shellQuote(stdinFilepath)}`;
  const res = spawnSync(cmd, {
    shell: true,
    input: code,
    encoding: "utf8",
    maxBuffer: 50 * 1024 * 1024,
  });

  if (res.error) {
    return { ok: false, error: res.error.message };
  }

  if (res.status !== 0) {
    const details = (res.stderr || res.stdout || "unknown formatter error").trim();
    return { ok: false, error: details || `formatter exited with code ${res.status}` };
  }

  return { ok: true, code: res.stdout };
}

function walkSnapshots(dirPath, out) {
  for (const entry of fs.readdirSync(dirPath, { withFileTypes: true })) {
    const abs = path.join(dirPath, entry.name);
    if (entry.isDirectory()) {
      walkSnapshots(abs, out);
      continue;
    }
    if (entry.isFile() && abs.endsWith(".snap")) {
      out.push(abs);
    }
  }
}

function collectSnapshotFiles(pathsArg) {
  const files = [];
  const seen = new Set();

  for (const inputPath of pathsArg) {
    const abs = path.resolve(process.cwd(), inputPath);
    if (!fs.existsSync(abs)) {
      console.warn(`[warn] path does not exist, skipping: ${inputPath}`);
      continue;
    }

    const stat = fs.statSync(abs);
    if (stat.isDirectory()) {
      const dirFiles = [];
      walkSnapshots(abs, dirFiles);
      for (const file of dirFiles) {
        if (!seen.has(file)) {
          files.push(file);
          seen.add(file);
        }
      }
      continue;
    }

    if (stat.isFile() && abs.endsWith(".snap") && !seen.has(abs)) {
      files.push(abs);
      seen.add(abs);
    }
  }

  files.sort();
  return files;
}

function resolveFormatterCmd() {
  if (process.env.OXFMT_CMD) {
    return process.env.OXFMT_CMD;
  }

  const localBin = path.resolve(process.cwd(), "node_modules/.bin/oxfmt");
  if (fs.existsSync(localBin)) {
    return shellQuote(localBin);
  }

  return "npx --yes oxfmt";
}

function isDiagnosticsSection(name) {
  return name.toUpperCase().includes(DIAGNOSTICS_KEYWORD);
}

function isCodeSection(name) {
  return !isDiagnosticsSection(name);
}

function detectMetadataStart(sectionLines) {
  for (let i = 0; i < sectionLines.length; i++) {
    if (sectionLines[i].trim() !== "/*") {
      continue;
    }

    const next = (sectionLines[i + 1] || "").trimStart();
    if (next.startsWith("{") || next.startsWith('"origin"')) {
      return i;
    }
  }

  return -1;
}

function chooseInputFilepath(snapshotPath, inputCode) {
  const base = path.basename(snapshotPath, ".snap");

  // Cases like router/react inline snapshots are sourced from .mjs fixtures.
  if (base.includes("qwik_router") || base.includes("qwik_react")) {
    return "snapshot-input.mjs";
  }

  // Heuristic for likely JS/MJS-only snippets.
  const trimmed = inputCode.trimStart();
  if (/^export\s+\{/.test(trimmed) || /^module\.exports\s*=/.test(trimmed)) {
    return "snapshot-input.js";
  }

  return "snapshot-input.tsx";
}

function chooseSectionFilepath(snapshotPath, sectionName, inputCode) {
  if (sectionName === INPUT_SECTION) {
    return chooseInputFilepath(snapshotPath, inputCode);
  }

  const normalized = sectionName.replace(/\\/g, "/");
  const ext = path.extname(normalized).toLowerCase();
  if (SUPPORTED_EXTENSIONS.has(ext)) {
    return normalized;
  }

  return "snapshot-section.tsx";
}

function processSnapshot(filePath, formatterCmd) {
  const original = fs.readFileSync(filePath, "utf8");
  const originalEndsWithNewline = original.endsWith("\n");
  const rawLines = original.split("\n");
  if (originalEndsWithNewline) {
    rawLines.pop();
  }

  const out = [];
  const warnings = [];
  let sectionsChanged = 0;

  for (let i = 0; i < rawLines.length; ) {
    const line = rawLines[i];
    const headerMatch = line.match(SECTION_HEADER_RE);

    if (!headerMatch) {
      out.push(line);
      i += 1;
      continue;
    }

    out.push(line);
    i += 1;

    const sectionName = headerMatch[1].trim();
    const bodyStart = i;
    while (i < rawLines.length && !SECTION_HEADER_RE.test(rawLines[i])) {
      i += 1;
    }

    const bodyLines = rawLines.slice(bodyStart, i);
    if (!isCodeSection(sectionName)) {
      out.push(...bodyLines);
      continue;
    }

    let codeLines = bodyLines;
    let tailLines = [];

    if (sectionName !== INPUT_SECTION) {
      const metaStart = detectMetadataStart(bodyLines);
      if (metaStart >= 0) {
        codeLines = bodyLines.slice(0, metaStart);
        tailLines = bodyLines.slice(metaStart);
      }
    }

    const code = codeLines.join("\n");
    if (code.trim().length === 0) {
      out.push(...bodyLines);
      continue;
    }

    const stdinFilepath = chooseSectionFilepath(filePath, sectionName, code);
    const fmtRes = runFormatter(formatterCmd, `${code}\n`, stdinFilepath);

    if (!fmtRes.ok) {
      warnings.push(`[warn] ${path.relative(process.cwd(), filePath)} :: ${sectionName} :: ${fmtRes.error}`);
      out.push(...bodyLines);
      continue;
    }

    let formatted = fmtRes.code;
    if (formatted.endsWith("\n")) {
      formatted = formatted.slice(0, -1);
    }

    // Preserve trailing blank lines from the original code block
    let trailingBlanks = 0;
    for (let j = codeLines.length - 1; j >= 0; j--) {
      if (codeLines[j].trim() === "") {
        trailingBlanks++;
      } else {
        break;
      }
    }

    const formattedLines = formatted.length > 0 ? formatted.split("\n") : [];

    // Re-append trailing blank lines that the formatter may have stripped
    for (let j = 0; j < trailingBlanks; j++) {
      formattedLines.push("");
    }
    const changed =
      formattedLines.length !== codeLines.length ||
      formattedLines.some((value, idx) => value !== codeLines[idx]);

    if (changed) {
      sectionsChanged += 1;
    }

    out.push(...formattedLines, ...tailLines);
  }

  let next = out.join("\n");
  if (originalEndsWithNewline) {
    next += "\n";
  }

  const fileChanged = next !== original;

  return {
    fileChanged,
    sectionsChanged,
    next,
    warnings,
  };
}

function main() {
  const args = process.argv.slice(2);
  const checkMode = args.includes("--check");
  const helpMode = args.includes("--help") || args.includes("-h");

  if (helpMode) {
    printHelp();
    process.exit(0);
  }

  const positional = args.filter((arg) => !arg.startsWith("--"));
  const targets = positional.length > 0 ? positional : DEFAULT_TARGETS;
  const formatterCmd = resolveFormatterCmd();

  const files = collectSnapshotFiles(targets);
  if (files.length === 0) {
    console.error("No .snap files found for the provided paths.");
    process.exit(2);
  }

  let changedFiles = 0;
  let changedSections = 0;
  const allWarnings = [];

  for (const file of files) {
    const res = processSnapshot(file, formatterCmd);
    if (res.warnings.length > 0) {
      allWarnings.push(...res.warnings);
    }

    if (!res.fileChanged) {
      continue;
    }

    changedFiles += 1;
    changedSections += res.sectionsChanged;

    if (!checkMode) {
      fs.writeFileSync(file, res.next, "utf8");
    }
  }

  const modeLabel = checkMode ? "check" : "write";
  console.log(
    `[${modeLabel}] scanned ${files.length} snapshot files, changed ${changedFiles} files (${changedSections} sections)`
  );

  if (allWarnings.length > 0) {
    for (const warning of allWarnings) {
      console.warn(warning);
    }
  }

  if (checkMode && changedFiles > 0) {
    process.exit(1);
  }
}

main();
