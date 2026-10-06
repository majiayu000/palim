"""Rebuild frozen sources and replay the recorded Rust benchmark boundaries."""

import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import tempfile
import time
import zipfile


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--quick", action="store_true", help="one process on three small fixtures")
    args = parser.parse_args()
    report = Path(__file__).resolve().parent
    repo = report.parent.parent
    evidence = json.loads((report / "measurements.json").read_text())
    previous = repo / evidence["previous_evidence"]["path"]
    assert digest(previous.read_bytes()) == evidence["previous_evidence"]["sha256"]
    root = Path(tempfile.mkdtemp(prefix="palim-guard-number-"))
    print(root, flush=True)

    with zipfile.ZipFile(previous) as archive:
        fixtures = root / "fixtures"
        fixtures.mkdir()
        for name, entry in evidence["previous_evidence"]["fixture_entries"].items():
            data = archive.read(entry)
            assert digest(data) == evidence["environment"]["fixtures"][name]
            (fixtures / name).write_bytes(data)
        for label in ["baseline", "final"]:
            for relative, expected in evidence["environment"]["source_hashes"][label].items():
                data = (archive.read("candidate/" + relative) if label == "baseline"
                        else evidence["candidate_files"][relative].encode())
                assert digest(data) == expected, (label, relative)
                path = root / label / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)

    for label in ["baseline", "final"]:
        probe = root / ("probe-" + label)
        (probe / "src").mkdir(parents=True)
        (probe / "Cargo.toml").write_text(evidence["probe"]["cargo"].replace("../final", "../" + label))
        (probe / "Cargo.lock").write_text(evidence["probe"]["lock"])
        for name, content in evidence["probe"]["sources"].items():
            (probe / "src" / name).write_text(content)
        subprocess.run(["cargo", "build", "--release", "--locked", "--manifest-path",
                        str(probe / "Cargo.toml")], check=True)

    # Stop building before timing. Each invocation independently verifies both
    # forward application and inverse restoration outside its timed sections.
    executables = {label: root / ("probe-" + label) / "target/release/palim-scale-competitor-probe"
                   for label in ["baseline", "final"]}
    executables["json-patch"] = executables["final"]
    records = []

    def save(name, value):
        (root / name).write_text(json.dumps(value, indent=2) + "\n")

    def run(label, case, mode, repeat, fixture=None):
        fixture = fixture or fixtures / (case + ".json")
        started = time.monotonic()
        row = json.loads(subprocess.check_output(
            [str(executables[label]), "json-patch" if label == "json-patch" else mode, str(fixture)],
            timeout=240))
        row["wire_sha256"] = digest(row.pop("wire").encode())
        assert row["self_verified"]
        row.update(label=label, case=case, mode=mode, repeat=repeat,
                   wall_seconds=time.monotonic() - started)
        for metric in ["core", "pipeline"]:
            row[metric + "_ms"] = statistics.median(row[metric + "_samples_ms"])
        records.append(row)
        save("records.json", records)
        print(label, case, mode, row["core_ms"], row["pipeline_ms"], flush=True)

    cases = sorted(path.stem for path in fixtures.glob("*.json"))
    if args.quick:
        cases = ["small-config-edit", "add-wide-100", "disjoint-2000"]
    repeats = 1 if args.quick else 3
    for repeat in range(repeats):
        for case in cases:
            combinations = [("baseline", "plain"), ("final", "plain"), ("json-patch", "plain"),
                            ("final", "optimized"), ("final", "plain-guarded"), ("final", "guarded")]
            if not args.quick and case in ["file_migrations_move_copy_5000", "scalar-guards-100000", "add-wide-100000"]:
                combinations += [("baseline", "optimized"), ("baseline", "guarded")]
            if repeat % 2:
                combinations.reverse()
            for label, mode in combinations:
                run(label, case, mode, repeat)

    if not args.quick:
        for count in [100, 400, 1600]:
            a, b = list(range(count)), list(range(100000, 100000 + count))
            name = "pguard-disjoint-" + str(count)
            fixture = root / (name + ".json")
            fixture.write_text(json.dumps(dict(name=name, left=a, right=b,
                left_raw=json.dumps(a, separators=(",", ":")), right_raw=json.dumps(b, separators=(",", ":"))),
                separators=(",", ":")))
            for repeat in range(3):
                for label in (["baseline", "final"] if repeat % 2 == 0 else ["final", "baseline"]):
                    run(label, name, "plain-guarded", repeat, fixture)

    summary = []
    for case, mode, label in dict.fromkeys((r["case"], r["mode"], r["label"]) for r in records):
        group = [r for r in records if (r["case"], r["mode"], r["label"]) == (case, mode, label)]
        assert len(group) == repeats and len({r["wire_sha256"] for r in group}) == 1
        summary.append(dict(case=case, mode=mode, label=label, wire_sha256=group[0]["wire_sha256"],
            wire_bytes=group[0]["wire_bytes"], processes=len(group),
            **{k + "_ms": statistics.median(r[k + "_ms"] for r in group) for k in ["core", "pipeline"]}))
    for case in cases:
        paired = [r for r in summary if r["case"] == case and r["mode"] == "plain" and r["label"] in ["baseline", "final"]]
        assert len({r["wire_sha256"] for r in paired}) == 1
    save("summary.json", summary)

    allocation_cases = [("add-wide-10000", "plain"), ("add-wide-100000", "plain"),
        ("file_migrations_move_copy_5000", "plain"), ("scalar-guards-100000", "optimized"),
        ("scalar-guards-100000", "plain-guarded"), ("scalar-guards-100000", "guarded"),
        ("file_migrations_move_copy_5000", "plain-guarded"), ("disjoint-20000", "plain-guarded")]
    if args.quick:
        allocation_cases = [("add-wide-100", "plain")]
    allocations = []
    for case, mode in allocation_cases:
        labels = ["baseline", "final"] if mode == "plain" or case == "scalar-guards-100000" else ["final"]
        if mode == "plain":
            labels.append("json-patch")
        for label in labels:
            executable = root / ("probe-" + ("final" if label == "json-patch" else label)) / "target/release/allocation"
            row = json.loads(subprocess.check_output(
                [str(executable), str(fixtures / (case + ".json")), "json-patch" if label == "json-patch" else mode],
                timeout=240))
            row.update(case=case, label=label, mode=mode, wire_sha256=digest(row.pop("wire").encode()))
            assert row["roundtrip_verified"] and row["allocation"]["final_live_delta_bytes"] == 0
            allocations.append(row)
    save("allocations.json", allocations)
    save("replay.json", dict(quick=args.quick, timing_processes=len(records), allocation_probes=len(allocations),
        frozen_source_hashes_verified=True, fixture_hashes_verified=True, forward_and_inverse_verified=True))
    print("COMPLETE", root, flush=True)


if __name__ == "__main__":
    main()
