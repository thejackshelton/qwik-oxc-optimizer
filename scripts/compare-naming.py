#!/usr/bin/env python3
"""Compare naming fields between SWC and OXC snapshots side by side.

Usage:
  python scripts/compare-naming.py                    # all tests
  python scripts/compare-naming.py example_1          # single test
  python scripts/compare-naming.py --diff-only        # only show mismatches
  python scripts/compare-naming.py --json             # raw JSON output
"""

import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OXC_SNAPS = ROOT / "crates/qwik-optimizer-oxc/tests/snapshots"
SWC_SNAPS = ROOT / "crates/swc-optimizer/core/src/snapshots"

NAMING_FIELDS = ["displayName", "name", "parent", "canonicalFilename", "ctxName", "ctxKind"]


def extract_metadata_blocks(snap_text: str) -> list:
    """Extract all JSON metadata blocks from a snapshot file."""
    blocks = []
    # Match /* { ... } */ blocks containing "name":
    pattern = re.compile(r'/\*\s*\n(\{[^}]*(?:\{[^}]*\}[^}]*)*\})\s*\n\*/', re.DOTALL)
    for match in pattern.finditer(snap_text):
        try:
            data = json.loads(match.group(1))
            if "name" in data:
                blocks.append(data)
        except json.JSONDecodeError:
            continue
    return blocks


def pick_naming(meta: dict) -> dict:
    """Extract only the naming-relevant fields."""
    return {f: meta.get(f) for f in NAMING_FIELDS}


def compare_test(test_name: str) -> dict:
    """Compare naming fields for a single test case."""
    oxc_path = OXC_SNAPS / f"{test_name}.snap"
    swc_path = SWC_SNAPS / f"qwik_core__test__{test_name}.snap"

    if not oxc_path.exists():
        return {"test": test_name, "error": "OXC snapshot missing"}
    if not swc_path.exists():
        return {"test": test_name, "error": "SWC snapshot missing"}

    oxc_blocks = extract_metadata_blocks(oxc_path.read_text())
    swc_blocks = extract_metadata_blocks(swc_path.read_text())

    # Index by canonical name (with hash replaced)
    def index_by_name(blocks):
        result = {}
        for b in blocks:
            key = b.get("name", "unknown")
            result[key] = b
        return result

    oxc_idx = index_by_name(oxc_blocks)
    swc_idx = index_by_name(swc_blocks)

    all_names = sorted(set(list(oxc_idx.keys()) + list(swc_idx.keys())))

    segments = []
    has_diff = False

    for name in all_names:
        oxc_meta = oxc_idx.get(name)
        swc_meta = swc_idx.get(name)

        if oxc_meta and swc_meta:
            oxc_naming = pick_naming(oxc_meta)
            swc_naming = pick_naming(swc_meta)
            match = oxc_naming == swc_naming
            if not match:
                has_diff = True
            segment = {
                "segment": name,
                "match": match,
                "fields": {}
            }
            for f in NAMING_FIELDS:
                oxc_val = oxc_naming[f]
                swc_val = swc_naming[f]
                entry = {"swc": swc_val, "oxc": oxc_val}
                if oxc_val != swc_val:
                    entry["diff"] = True
                segment["fields"][f] = entry
            segments.append(segment)
        elif oxc_meta and not swc_meta:
            has_diff = True
            segments.append({
                "segment": name,
                "match": False,
                "note": "OXC only (extra segment)",
                "fields": {f: {"swc": None, "oxc": oxc_meta.get(f)} for f in NAMING_FIELDS}
            })
        else:
            has_diff = True
            segments.append({
                "segment": name,
                "match": False,
                "note": "SWC only (missing from OXC)",
                "fields": {f: {"swc": swc_meta.get(f), "oxc": None} for f in NAMING_FIELDS}
            })

    return {
        "test": test_name,
        "match": not has_diff,
        "oxc_segments": len(oxc_blocks),
        "swc_segments": len(swc_blocks),
        "segments": segments
    }


def print_human(results: list[dict], diff_only: bool):
    """Pretty-print results for human consumption."""
    total = len(results)
    matched = sum(1 for r in results if r.get("match"))
    diffed = total - matched
    errors = [r for r in results if "error" in r]

    print(f"\n{'='*70}")
    print(f" Naming Comparison: {matched}/{total} match, {diffed} diffs, {len(errors)} errors")
    print(f"{'='*70}\n")

    for r in results:
        if "error" in r:
            print(f"  {r['test']}: {r['error']}")
            continue

        if diff_only and r["match"]:
            continue

        icon = "PASS" if r["match"] else "DIFF"
        seg_info = f"({r['oxc_segments']} oxc / {r['swc_segments']} swc segments)"
        print(f"  [{icon}] {r['test']} {seg_info}")

        if not r["match"]:
            for seg in r["segments"]:
                if seg["match"]:
                    continue

                note = f" -- {seg.get('note', '')}" if "note" in seg else ""
                print(f"    segment: {seg['segment']}{note}")

                for field, vals in seg["fields"].items():
                    if vals.get("diff") or vals.get("swc") != vals.get("oxc"):
                        swc_v = json.dumps(vals["swc"])
                        oxc_v = json.dumps(vals["oxc"])
                        print(f"      {field}:")
                        print(f"        swc: {swc_v}")
                        print(f"        oxc: {oxc_v}")
                print()

    print(f"{'='*70}")
    print(f" Summary: {matched} pass, {diffed} diff, {len(errors)} error")

    # Categorize diffs
    if diffed > 0:
        ordering_only = []
        naming_diffs = []
        missing_segments = []

        for r in results:
            if r.get("match") or "error" in r:
                continue
            if r["oxc_segments"] != r["swc_segments"]:
                missing_segments.append(r["test"])
            else:
                # Check if it's ordering-only (same set of names, different order)
                oxc_names = {s["segment"] for s in r["segments"]}
                swc_only = any("note" in s and "SWC only" in s.get("note", "") for s in r["segments"])
                oxc_only = any("note" in s and "OXC only" in s.get("note", "") for s in r["segments"])
                if swc_only or oxc_only:
                    ordering_only.append(r["test"])
                else:
                    naming_diffs.append(r["test"])

        if ordering_only:
            print(f"\n  Ordering diffs ({len(ordering_only)}): {', '.join(ordering_only)}")
        if naming_diffs:
            print(f"  Naming diffs ({len(naming_diffs)}): {', '.join(naming_diffs)}")
        if missing_segments:
            print(f"  Missing segments ({len(missing_segments)}): {', '.join(missing_segments)}")

    print(f"{'='*70}\n")


def main():
    args = sys.argv[1:]
    diff_only = "--diff-only" in args
    json_output = "--json" in args
    args = [a for a in args if not a.startswith("--")]

    if args:
        test_names = args
    else:
        # Find all OXC snapshot names
        test_names = sorted(
            p.stem for p in OXC_SNAPS.glob("*.snap")
            if not p.name.endswith(".snap.new")
        )

    results = []
    for name in test_names:
        result = compare_test(name)
        if result:
            results.append(result)

    if json_output:
        output = results if not diff_only else [r for r in results if not r.get("match")]
        print(json.dumps(output, indent=2))
    else:
        print_human(results, diff_only)


if __name__ == "__main__":
    main()
