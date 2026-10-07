#!/usr/bin/env python3
"""Build the owned add/replace prototype in a temporary frozen source copy."""
import argparse
import collections
import datetime
import hashlib
import io
import json
import pathlib
import platform
import statistics
import subprocess
import tarfile
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
PROBE = ROOT / "scratchpad/owned_patch.rs"
ARCHIVE = ROOT / "results/addition-copy-20261006/evidence.zip"
CASES = ["add-wide-10000", "add-wide-100000", "scalar-guards-100000", "unchanged-100000", "small-config-edit"]
ENGINES = ["palim-borrowed", "json-patch", "owned-ready", "owned-clone"]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def command(args, cwd=None, timeout=300):
    result = subprocess.run(args, cwd=cwd, capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"{args} exited {result.returncode}:\n{result.stderr.decode(errors='replace')}\n{result.stdout.decode(errors='replace')}")
    return result.stdout.decode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-ref", default="HEAD")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    commit = command(["git", "rev-parse", args.source_ref], ROOT).strip()
    source = subprocess.run(
        ["git", "archive", commit, "Cargo.toml", "Cargo.lock", "src", "benches", "README.md"],
        cwd=ROOT, capture_output=True, check=True,
    ).stdout
    evidence = {
        "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "source_commit": commit,
        "source_archive_sha256": digest(source),
        "prototype_sha256": digest(PROBE.read_bytes()),
        "runner_sha256": digest(pathlib.Path(__file__).read_bytes()),
        "fixture_archive_sha256": digest(ARCHIVE.read_bytes()),
        "environment": {"platform": platform.platform(), "rustc": command(["rustc", "-Vv"]).strip(),
                        "cargo": command(["cargo", "-V"]).strip()},
        "method": {
            "scope": "Experimental add/replace only; no changed array pairs, deletions, callbacks or optimization.",
            "core": "Parsed Values -> generate and destroy Patch; owned-ready receives a prepared target outside timing.",
            "generation": "Same Core operation; timestamp immediately after generation, before destroying Patch.",
            "owned_clone_core": "Clone right inside timing -> owned generation -> destroy Patch.",
            "pipeline": "Same raw bytes -> parse both documents -> generate -> serialize -> destroy inputs and output.",
            "sampling": "3 warmups, >=10ms calibration capped at 2048, 9 batches, 3 serial processes; rotated engine order.",
            "comparison": "Borrowed and consuming Core are different ownership contracts; pipeline boundaries are identical.",
        },
        "records": [], "fixtures": {}, "checks": {},
    }
    with tempfile.TemporaryDirectory(prefix="palim-owned-") as work:
        folder = pathlib.Path(work)
        with tarfile.open(fileobj=io.BytesIO(source)) as archive:
            archive.extractall(folder, filter="data")
        (folder / "scratchpad").mkdir()
        (folder / "scratchpad/owned_patch.rs").write_bytes(PROBE.read_bytes())
        lib = folder / "src/lib.rs"
        lib.write_text(lib.read_text() + '\n#[path = "../scratchpad/owned_patch.rs"]\npub mod owned_probe;\n')
        export = folder / "src/export.rs"
        content = export.read_text()
        for name in ["normalize_addition", "check_leaf_depth"]:
            needle = f"fn {name}("
            if content.count(needle) != 1:
                raise RuntimeError(f"expected one existing helper: {name}")
            content = content.replace(needle, "pub(crate) " + needle)
        export.write_text(content)
        (folder / "examples").mkdir()
        (folder / "examples/owned_probe.rs").write_text(
            "fn main() -> Result<(), Box<dyn std::error::Error>> { palim::owned_probe::run() }\n"
        )
        evidence["checks"]["prototype_tests_debug"] = command(
            ["cargo", "test", "--locked", "--offline", "--lib", "owned_probe::tests"], folder
        ).strip()
        evidence["checks"]["prototype_tests_release"] = command(
            ["cargo", "test", "--release", "--locked", "--offline", "--lib", "owned_probe::tests"], folder
        ).strip()
        command(["cargo", "build", "--release", "--locked", "--offline", "--example", "owned_probe"], folder)
        command(["cargo", "clippy", "--all-targets", "--locked", "--offline", "--", "-D", "warnings"], folder)
        evidence["checks"]["prototype_clippy_all_targets"] = "exit 0"
        command(["cargo", "+1.85.0", "check", "--lib", "--locked", "--offline"], folder)
        evidence["checks"]["prototype_msrv_library"] = "exit 0"
        print("prototype: debug/release tests, Clippy and Rust 1.85 passed", flush=True)
        if not args.verify_only:
            fixtures = folder / "fixtures"
            fixtures.mkdir()
            with zipfile.ZipFile(ARCHIVE) as archive:
                for case in CASES:
                    raw = archive.read("fixtures/" + case + ".json")
                    (fixtures / (case + ".json")).write_bytes(raw)
                    evidence["fixtures"][case] = digest(raw)
            binary = folder / "target/release/examples/owned_probe"
            for repeat in range(3):
                order = ENGINES[repeat:] + ENGINES[:repeat]
                for case in CASES:
                    for engine in order:
                        result = json.loads(command([str(binary), engine, str(fixtures / (case + ".json"))], timeout=120))
                        result["repeat"] = repeat
                        result["wire_sha256"] = digest(result.pop("wire").encode())
                        evidence["records"].append(result)
                    print(f"round {repeat + 1}/3: {case}", flush=True)
            groups = collections.defaultdict(list)
            for result in evidence["records"]:
                groups[(result["fixture"], result["engine"])].append(result)
            summaries = []
            for (case, engine), records in groups.items():
                if len({record["wire_sha256"] for record in records}) != 1:
                    raise RuntimeError(f"nondeterministic output: {case}/{engine}")
                summaries.append({
                    "fixture": case, "engine": engine,
                    "core_ms": statistics.median(statistics.median(r["core_samples_ms"]) for r in records),
                    "generation_ms": statistics.median(statistics.median(r["generation_samples_ms"]) for r in records),
                    "pipeline_ms": None if engine == "owned-clone" else statistics.median(
                        statistics.median(r["pipeline_samples_ms"]) for r in records),
                    "wire_bytes": records[0]["wire_bytes"], "wire_sha256": records[0]["wire_sha256"],
                    "operation_counts": records[0]["operation_counts"],
                })
            for case in CASES:
                hashes = {r["wire_sha256"] for r in summaries if r["fixture"] == case}
                if len(hashes) != 1:
                    raise RuntimeError(f"engines differ in output: {case}")
            evidence["summary"] = summaries
            evidence["checks"]["wire_equality_all_engines"] = "all five fixtures; three processes per engine"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n")
    print(f"saved {args.output}", flush=True)


if __name__ == "__main__":
    main()
