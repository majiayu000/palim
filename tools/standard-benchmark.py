#!/usr/bin/env python3
"""Typed RFC 6902 pipeline comparison; preserves each run in its own directory.

Build first: cargo build --release --example standard_bench --locked
Verify only: python3 tools/standard-benchmark.py --dry-run
Sample: python3 tools/standard-benchmark.py
"""
import argparse
import copy
import datetime
import hashlib
import json
import os
import platform
import re
import signal
import subprocess
import time
from pathlib import Path

from benchmark import inputs

ROOT = Path(__file__).resolve().parents[1]
ENGINES = ("json-patch", "plain", "optimized", "guarded", "optimized-guarded")
TASK_ENGINES = {
    "pipeline": ("json-patch", "plain", "optimized"),
    "export": ("plain",),
    "apply": ("json-patch", "plain"),
    "inverse": ("plain",),
}


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=True, indent=2) + "\n")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fixtures():
    # Reuse the exact historical workload definitions, not a smaller document
    # or a different operation count for one of the compared engines.
    selected = {
        "small-config-edit", "rotate-2000", "keyed-rotate-edit-2000",
        "disjoint-2000", "long-text-ascii",
    }
    cases = [{"name": c["name"], "left": c["left"], "right": c["right"],
              "tasks": ["pipeline", "export"]}
             for c in inputs() if c["name"] in selected]
    source = [f"entry-{i:08d}" for i in range(500)]
    cases.append({"name": "halfswap-500", "left": source,
                  "right": source[250:] + source[:250], "tasks": ["pipeline", "export"]})
    base = "".join(f"line {i:04d} Unicode 🦀 中文 stable context\n" for i in range(5000))
    for name, right in [
        ("unicode-first", base.replace("line 0000", "首行修改🚀", 1)),
        ("unicode-middle", base.replace("line 2500", "中间修改🚀", 1)),
        ("unicode-last", base.replace("line 4999", "末行修改🚀", 1)),
        ("unicode-three-edits", base.replace("line 0000", "首行修改🚀", 1)
         .replace("line 2500", "中间修改🚀", 1).replace("line 4999", "末行修改🚀", 1)),
        ("unicode-disjoint", "αβγδεζηθ🚀" * 12500),
    ]:
        cases.append({"name": name, "left": base, "right": right,
                      "tasks": ["pipeline", "export"]})
    for n in (2000, 20000):
        source = list(range(n))
        multiplier = 997 if n == 2000 else 9973
        for name, right in [("shuffle", [i * multiplier % n for i in source]),
                            ("reverse", source[::-1])]:
            cases.append({"name": f"{name}-{n}", "left": source, "right": right,
                          "tasks": ["pipeline", "export"] if n == 2000 else ["export"],
                          "verify_only": n == 20000})
    for name, period in [("duplicates-high-2000", 8), ("duplicates-low-2000", 1000)]:
        source = [i % period for i in range(2000)]
        right = source[37:] + source[:37]
        for i in range(0, 2000, 100):
            right[i] = 3000 + i
        cases.append({"name": name, "left": source, "right": right,
                      "tasks": ["pipeline", "export"]})
    source = [{"id": i % 32, "value": i} for i in range(2000)]
    right = copy.deepcopy(source[997:] + source[:997])
    right[1000]["value"] = -1
    cases.append({"name": "ambiguous-ids-2000", "left": source, "right": right,
                  "tasks": ["pipeline", "export"]})
    source = {"items": [{"nested": {"score": i, "keep": "unchanged"}} for i in range(2000)],
              "untouched": ["unchanged payload"] * 5000}
    right = copy.deepcopy(source)
    operations = []
    guards = []
    for i in range(2000):
        right["items"][i]["nested"]["score"] = -1
        change = {"op": "replace", "path": f"/items/{i}/nested/score", "value": -1}
        operations.append(change)
        guards.extend([{"op": "test", "path": change["path"], "value": i}, change])
    for name, patch, inverse in [("batch-replace-2000", operations, False),
                                 ("batch-guarded-4000", guards, False),
                                 ("batch-inverse-2000", operations, True)]:
        cases.append({"name": name, "left": source, "right": right, "patch": patch,
                      "apply_inverse": inverse,
                      "tasks": ["apply"] if inverse else ["apply", "inverse"]})
    return cases


def run(engine, fixture, task, dry_run):
    cmd = [str(ROOT / "target/release/examples/standard_bench"), engine, str(fixture), "--task", task]
    if dry_run:
        cmd.append("--dry-run")
    timer = ["/usr/bin/time", "-l" if platform.system() == "Darwin" else "-v"]
    started = time.monotonic()
    process = subprocess.Popen(timer + cmd, cwd=ROOT, text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)
    try:
        stdout, stderr = process.communicate(timeout=180)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate()
        return {"name": fixture.stem, "engine": engine, "task": task,
                "status": "skipped-resource-budget", "measured": False,
                "reason": "180-second process budget exceeded; no completed timing or correctness claim",
                "dry_run": dry_run, "timeout_seconds": 180}
    if process.returncode:
        raise RuntimeError(f"{engine}/{fixture.stem}: exit {process.returncode}\n{stderr[-4000:]}")
    result = json.loads(stdout.strip().splitlines()[-1])
    if not result.get("ok"):
        raise RuntimeError(f"incorrect patch: {engine}/{fixture.stem}")
    pattern = (r"(\d+)\s+maximum resident set size" if platform.system() == "Darwin"
               else r"Maximum resident set size \(kbytes\):\s*(\d+)")
    match = re.search(pattern, stderr)
    result["rss_mib"] = (int(match[1]) / (1024 ** 2 if platform.system() == "Darwin" else 1024)
                         if match else None)
    result["process_seconds"] = time.monotonic() - started
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="verify all engines without sampling")
    parser.add_argument("--task", choices=TASK_ENGINES, default="pipeline")
    parser.add_argument("--case", action="append", dest="names", help="select exact fixture name; repeat to select several")
    parser.add_argument("--engine", action="append", choices=ENGINES, help="select output generator/applicator")
    parser.add_argument("--output", type=Path, help="new output directory; existing directories are never overwritten")
    args = parser.parse_args()
    label = "standard-benchmark-dry-run" if args.dry_run else "standard-benchmark"
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    out = args.output or ROOT / "results" / f"{label}-{args.task}-{stamp}"
    if not out.is_absolute():
        out = ROOT / out
    available = fixtures()
    if args.names:
        missing = set(args.names) - {c["name"] for c in available}
        if missing:
            parser.error(f"unknown fixture names: {sorted(missing)}")
    cases = [c for c in available if args.task in c["tasks"]
             and (not args.names or c["name"] in args.names)]
    if not cases:
        parser.error("no selected fixture supports this task")
    engines = tuple(dict.fromkeys(args.engine or TASK_ENGINES[args.task]))
    out.mkdir(parents=True, exist_ok=False)
    fixture_paths = []
    hashes = {}
    for case in cases:
        file = out / "fixtures" / f"{case['name']}.json"
        dump(file, case)
        fixture_paths.append(file)
        hashes[case["name"]] = sha256(file)
    environment = {
        "recorded_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(), "machine": platform.machine(),
        "rustc": subprocess.check_output(["rustc", "-Vv"], text=True).strip(),
        "dry_run": args.dry_run, "fixture_sha256": hashes,
        "task": args.task, "engines": engines,
        "cargo_lock_sha256": sha256(ROOT / "Cargo.lock"),
        "source_sha256": {str(file.relative_to(ROOT)): sha256(file)
                          for file in sorted((ROOT / "src").glob("*.rs"))},
        "runner_sha256": sha256(ROOT / "examples/standard_bench.rs"),
        "script_sha256": sha256(Path(__file__).resolve()),
        "binary_sha256": sha256(ROOT / "target/release/examples/standard_bench"),
        "method": {
            "pipeline": "identical bytes; parse both -> typed diff -> serialize standard Patch",
            "export": "precomputed Delta -> RFC export -> serialize Patch; no parse or diff",
            "apply": "identical supplied operations; clone/apply baseline -> serialize result; preparation excluded",
            "inverse": "baseline and supplied operations -> inverse -> serialize Patch; no application timing",
        }[args.task],
        "sampling": "20 warmups, >=25ms calibration (cap 16384 iterations), 10 raw samples per process",
        "repetitions": 1 if args.dry_run else 3,
        "rss_method": "whole process peak; includes runtime, verification, warmup and samples",
        "engine_semantics": {
            "json-patch": "typed json_patch::diff; positional arrays; no added guards",
            "plain": "DiffPatcher::diff_json_patch; ID matching; factorize/rationalize/tests=false",
            "optimized": "same DiffPatcher; factorize/rationalize=true; tests=false",
            "guarded": "same DiffPatcher; factorize/rationalize=false; tests=true; guard cost is explicit",
            "optimized-guarded": "same DiffPatcher; factorize/rationalize/tests=true",
        },
        "budget": "20k reorder fixtures are correctness-only; any 180s process overrun is skipped, not failed",
    }
    if platform.system() == "Darwin":
        environment["cpu"] = subprocess.check_output(
            ["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
    dump(out / "environment.json", environment)
    results = []
    over_budget = set()
    for repeat in range(environment["repetitions"]):
        for fixture_index, fixture in enumerate(fixture_paths):
            case = cases[fixture_index]
            if case.get("verify_only") and repeat > 0:
                continue
            # Rotate order between inputs and independent process repetitions;
            # all actual executions stay sequential.
            offset = (fixture_index + repeat) % len(engines)
            order = engines[offset:] + engines[:offset]
            for engine in order:
                if (fixture.stem, engine) in over_budget:
                    continue
                supported = (engine in TASK_ENGINES[args.task] or
                             (args.task == "pipeline" and engine in ("guarded", "optimized-guarded")))
                if not supported:
                    record = {"name": fixture.stem, "engine": engine, "task": args.task,
                              "status": "skipped-unsupported-task", "measured": False,
                              "reason": "engine has no equivalent API for this task"}
                else:
                    record = run(engine, fixture, args.task, args.dry_run or case.get("verify_only", False))
                if case.get("verify_only"):
                    record["sampling_skipped"] = "20k reorder correctness-only budget; not a measured failure"
                if record.get("status") == "skipped-resource-budget":
                    over_budget.add((fixture.stem, engine))
                record["repetition"] = repeat + 1
                results.append(record)
                dump(out / "comparison.json", results)
                if record.get("ok"):
                    print(f"{fixture.stem} {engine}: verified; "
                          f"ops={record['operations']} bytes={record['patch_bytes']}", flush=True)
                else:
                    print(f"{fixture.stem} {engine}: {record['status']}", flush=True)
    summary = {"verified": sum(r.get("ok") is True for r in results),
               "measured": sum(r["measured"] for r in results),
               "budget_skipped": sum(r.get("status") == "skipped-resource-budget" for r in results),
               "unsupported": sum(r.get("status") == "skipped-unsupported-task" for r in results),
               "dry_run": args.dry_run, "output": str(out)}
    dump(out / "summary.json", summary)
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
