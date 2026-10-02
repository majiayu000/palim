import datetime, hashlib, json, platform, statistics, subprocess, sys, tempfile
from pathlib import Path

candidate = Path(sys.argv[1]).resolve()
baseline = Path(sys.argv[2]).resolve()
baseline_binary = Path(sys.argv[3]).resolve()
out = Path(sys.argv[4]).resolve()
candidate_binary = candidate / "target/release/examples/standard_bench"
out.mkdir(parents=True, exist_ok=False)
fixture_dir = baseline / "results/standard-benchmark-pipeline-20260930T214341.446185Z/fixtures"
cases = ["small-config-edit", "disjoint-2000", "rotate-2000", "reverse-2000", "duplicates-high-2000", "prepend-2000", "unchanged-2000"]
binaries = {"published-0.1.0": baseline_binary, "candidate": candidate_binary}
def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
assert sha(baseline_binary) != sha(candidate_binary), "benchmark binaries must differ"
scratch = tempfile.TemporaryDirectory(prefix="palim-performance-fixtures-")
fixture_paths = {case: fixture_dir / (case + ".json") for case in cases[:5]}
for case, target in [("prepend-2000", [-1] + list(range(2000))), ("unchanged-2000", list(range(2000)))]:
    path = Path(scratch.name) / (case + ".json")
    path.write_text(json.dumps({"name":case, "left":list(range(2000)), "right":target}) + "\n")
    fixture_paths[case] = path
metadata = {
    "recorded_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "platform": platform.platform(),
    "cpu": subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip() if platform.system()=="Darwin" else platform.processor(),
    "rustc": subprocess.check_output(["rustc", "-Vv"], text=True).strip(),
    "baseline_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=baseline, text=True).strip(),
    "candidate_base_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=candidate, text=True).strip(),
    "candidate_diff": subprocess.check_output(["git", "diff", "--", "src"], cwd=candidate, text=True),
    "cargo_lock_sha256": sha(candidate / "Cargo.lock"),
    "source_sha256": {label: {str(p.relative_to(root)): sha(p) for p in sorted((root / "src").glob("*.rs"))} for label, root in [("baseline",baseline),("candidate",candidate)]},
    "binary_sha256": {label: sha(path) for label,path in binaries.items()},
    "fixture_sha256": {case: sha(path) for case,path in fixture_paths.items()},
    "additional_fixture_definitions": {"prepend-2000": "left=list(range(2000)); right=[-1]+left", "unchanged-2000":"left=list(range(2000)); right=left"},
    "runner_sha256": sha(candidate / "examples/standard_bench.rs"),
    "sampling_script_sha256": sha(Path(__file__)),
    "method": "identical bytes; parse both documents -> typed standard diff -> serialize Patch; independent application verifies serialized output before timing",
    "sampling": "3 independent processes per engine and version; interleave version pairs and rotate engine order; each process performs 20 warmups, 25ms calibration capped at 16384 iterations, and 10 raw samples",
    "case_engines": {case: ["plain","optimized","json-patch"] if case in cases[:2] else ["plain"] for case in cases},
    "note": "json-patch is the typed Rust positional competitor. Plain enables ID matching but disables factorization, rationalization and tests; optimized enables factorization+rationalization. Array-edit replacements are disabled for factorization and tests."
}
(out / "environment.json").write_text(json.dumps(metadata, indent=2) + "\n")
records=[]
for repeat in range(3):
    for ci, case in enumerate(cases):
        engines = metadata["case_engines"][case]
        offset = (repeat + ci) % len(engines)
        order = engines[offset:] + engines[:offset]
        versions = list(binaries) if (repeat+ci)%2==0 else list(reversed(binaries))
        for engine in order:
            for version in versions:
                command=[str(binaries[version]), engine, str(fixture_paths[case])]
                result=subprocess.run(command, cwd=candidate, capture_output=True, text=True, timeout=180, check=True)
                record=json.loads(result.stdout)
                assert record["ok"] and record["measured"]
                record.update(version=version, repeat=repeat+1)
                records.append(record)
summary=[]
for case in cases:
    for engine in metadata["case_engines"][case]:
        row={"name":case,"engine":engine}
        for version in binaries:
            selected=[r for r in records if r["name"]==case and r["engine"]==engine and r["version"]==version]
            row[version]={"median_ms":statistics.median(r["pipeline_ms"] for r in selected),"process_medians_ms":[r["pipeline_ms"] for r in selected],"patch_bytes":selected[0]["patch_bytes"],"operations":selected[0]["operations"],"operation_counts":selected[0]["operation_counts"]}
            assert len({(r["patch_bytes"],r["operations"]) for r in selected})==1
        row["speedup"]=row["published-0.1.0"]["median_ms"]/row["candidate"]["median_ms"]
        summary.append(row)
(out / "summary.json").write_text(json.dumps({"records":records,"comparison":summary}, indent=2)+"\n")
print(json.dumps(summary, indent=2))
