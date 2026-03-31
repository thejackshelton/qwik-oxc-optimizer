#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const DEFAULT_TARGET = "crates/swc-optimizer/core/src/test.rs";

function printHelp() {
  console.log(`Format inline TestInput code blocks in old optimizer test.rs.

Usage:
  node scripts/format-old-test-rs-inputs.mjs [--check] [path]

Options:
  --check   Check mode. Exits with code 1 if file would change.
  --help    Show this help message.
`);
}

function shellQuote(value) {
  return `'${String(value).replace(/'/g, `'"'"'`)}'`;
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

function extensionFromFilename(filename) {
  const normalized = filename.replace(/\\/g, "/");
  const ext = path.extname(normalized).toLowerCase();
  if (ext === ".mjs") return "mjs";
  if (ext === ".cjs") return "cjs";
  if (ext === ".jsx") return "jsx";
  if (ext === ".js") return "js";
  if (ext === ".ts") return "ts";
  return "tsx";
}

function detectStdinFilepath(source, afterBlockPos) {
  const tail = source.slice(afterBlockPos, Math.min(source.length, afterBlockPos + 500));
  const filenameMatch = tail.match(/filename:\s*"([^"]+)"/);
  const ext = filenameMatch ? extensionFromFilename(filenameMatch[1]) : "tsx";
  return `snapshot-input.${ext}`;
}

function formatOldTestInputs(source, formatterCmd) {
  let out = "";
  let cursor = 0;
  let formattedBlocks = 0;
  const warnings = [];

  while (cursor < source.length) {
    const keyPos = source.indexOf("code:", cursor);
    if (keyPos === -1) {
      out += source.slice(cursor);
      break;
    }

    out += source.slice(cursor, keyPos);

    const rawStart = source.slice(keyPos).match(/^code:\s*r(#+)"/);
    if (!rawStart) {
      out += source.slice(keyPos, keyPos + 5);
      cursor = keyPos + 5;
      continue;
    }

    const hashes = rawStart[1];
    const literalStart = keyPos + rawStart[0].length;
    const closeToken = `"${hashes}`;
    const closePos = source.indexOf(closeToken, literalStart);
    if (closePos === -1) {
      warnings.push("[warn] unclosed raw string literal after code:");
      out += source.slice(keyPos);
      break;
    }

    const prefix = source.slice(keyPos, literalStart);
    const codeBody = source.slice(literalStart, closePos);
    const suffix = closeToken;

    const stdinFilepath = detectStdinFilepath(source, closePos + closeToken.length);
    const fmtRes = runFormatter(formatterCmd, codeBody, stdinFilepath);

    if (!fmtRes.ok) {
      warnings.push(`[warn] formatter failed for block near index ${keyPos}: ${fmtRes.error}`);
      out += prefix + codeBody + suffix;
      cursor = closePos + closeToken.length;
      continue;
    }

    out += prefix + fmtRes.code + suffix;
    cursor = closePos + closeToken.length;
    formattedBlocks += 1;
  }

  return { next: out, formattedBlocks, warnings };
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
  const target = positional[0] || DEFAULT_TARGET;
  const absTarget = path.resolve(process.cwd(), target);

  if (!fs.existsSync(absTarget)) {
    console.error(`Target not found: ${target}`);
    process.exit(2);
  }

  const formatterCmd = resolveFormatterCmd();
  const original = fs.readFileSync(absTarget, "utf8");
  const res = formatOldTestInputs(original, formatterCmd);
  const changed = res.next !== original;

  console.log(
    `[${checkMode ? "check" : "write"}] ${target}: formatted ${res.formattedBlocks} blocks, changed=${changed}`
  );
  for (const warning of res.warnings) {
    console.warn(warning);
  }

  if (!checkMode && changed) {
    fs.writeFileSync(absTarget, res.next, "utf8");
  }

  if (checkMode && changed) {
    process.exit(1);
  }
}

main();
