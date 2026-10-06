#!/usr/bin/env python3
"""Replay the frozen Rust A/B comparison. No publication or repository mutation."""
import argparse
import hashlib
import io
import json
import os
import pathlib
import shutil
import statistics
import subprocess
import tarfile
import zipfile

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
MODES = ['plain', 'optimized', 'plain-guarded', 'guarded']

def digest(data):
    return hashlib.sha256(data).hexdigest()

def command(args, **kwargs):
    result = subprocess.run(args, capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f'{args}: {result.stderr.decode(errors="replace")}')
    return result.stdout

def fixture(name, left, right):
    compact = lambda v: json.dumps(v, ensure_ascii=False, separators=(',', ':'))
    return compact(dict(name=name, left=left, right=right,
                        left_raw=compact(left), right_raw=compact(right))).encode()

def shuffled(n):
    values = list(range(n))
    state = 20261006
    for i in range(n - 1, 0, -1):
        state = (1664525 * state + 1013904223) & 0xffffffff
        j = state % (i + 1)
        values[i], values[j] = values[j], values[i]
    return values

def prepare(work, evidence):
    work.mkdir(parents=True, exist_ok=True)
    archive = command(['git', 'archive', evidence['base_commit'], 'Cargo.toml', 'Cargo.lock', 'src', 'benches', 'README.md'], cwd=REPO)
    for label, overrides in evidence['sources'].items():
        folder = work / label
        folder.mkdir(exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(archive)) as source:
            source.extractall(folder, filter='data')
        for name, content in overrides.items():
            (folder / name).write_text(content)
        probe = work / f'probe-{label}'
        (probe / 'src').mkdir(parents=True, exist_ok=True)
        for name, content in evidence['probe'].items():
            if name == 'Cargo.toml':
                content = content.replace('../candidate', f'../{label}')
            elif name == 'Cargo.lock':
                content = evidence['probe_locks'][label]
            (probe / name).write_text(content)
    fixtures = work / 'fixtures'
    fixtures.mkdir(exist_ok=True)
    old = evidence['previous_evidence']
    raw = (REPO / old['path']).read_bytes()
    assert digest(raw) == old['sha256']
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        for name, member in old['fixture_entries'].items():
            (fixtures / name).write_bytes(archive.read(member))
    prior = json.loads((REPO / 'results/ci-crosslang-20261006/measurements.json').read_text())
    for name, content in prior['cross_language']['extra_fixtures'].items():
        (fixtures / name).write_text(content)
    (fixtures / 'shuffle-20000.json').write_bytes(fixture('shuffle-20000', list(range(20000)), shuffled(20000)))
    long_values = [f'item-{i}-' + 'é~ /"\\' * 64 for i in range(2000)]
    (fixtures / 'shuffle-long-2000.json').write_bytes(fixture('shuffle-long-2000', long_values, [long_values[i] for i in shuffled(2000)]))
    small = list(range(128))
    cases = {
        'reverse-small': (small, small[::-1]),
        'shuffle-small': (small, shuffled(128)),
        'rotate-one-small': (small, small[1:] + small[:1]),
        'rotate-half-small': (small, small[64:] + small[:64]),
        'nested-escaped-small': ({'a~/é': small}, {'a~/é': small[::-1]}),
        'long-small': (long_values[:128], long_values[:128][::-1]),
        'duplicates-small': ([i % 4 for i in small], [i % 4 for i in small][::-1]),
        'nonpermutation-small': (small, [i + 1 for i in small]),
    }
    for name, (left, right) in cases.items():
        (fixtures / f'{name}.json').write_bytes(fixture(name, left, right))
    return fixtures

def build(work):
    for label in ['baseline', 'fold', 'candidate']:
        command(['cargo', 'build', '--release', '--locked', '--offline'], cwd=work / f'probe-{label}')
        print(f'built {label}', flush=True)

def frozen(work, evidence, output):
    prior = json.loads((REPO / 'results/opt-rotate-20261006/measurements.json').read_text())['oracle']
    oracle = work / 'oracle-replay'
    oracle.mkdir(exist_ok=True)
    for name, content in prior['fixtures'].items():
        (oracle / name).write_text(content)
    records = []
    for label in ['baseline', 'fold', 'candidate']:
        source = oracle / label
        source.mkdir(exist_ok=True)
        shutil.copytree(work / label / 'src', source / 'src', dirs_exist_ok=True)
        shutil.copytree(work / label / 'benches', source / 'benches', dirs_exist_ok=True)
        for name in ['Cargo.toml', 'Cargo.lock', 'README.md']:
            shutil.copyfile(work / label / name, source / name)
        rfc = source / 'src/rfc.rs'
        rfc.write_text(rfc.read_text() + prior['modules'])
        destination = oracle / f'{label}-output'
        destination.mkdir(exist_ok=True)
        env = os.environ.copy()
        env.update(CARGO_TARGET_DIR=str(oracle / f'target-{label}'),
                   PALIM_INDEX_ORACLE_OUTPUT_DIR=str(destination),
                   PALIM_MIXED_ORACLE_OUTPUT=str(destination / 'targeted-output.json'),
                   PALIM_SNAPSHOT_ORACLE_OUTPUT=str(destination / 'snapshot-output.json'),
                   PALIM_GUARD_CONSTRUCTION_OUTPUT=str(destination / 'construction-output.json'))
        command(['cargo', 'test', '--lib', '--locked', '--offline'], cwd=source, env=env)
        for name, expected in evidence['oracle']['final_comparison'].items():
            raw = (destination / name).read_bytes()
            assert digest(raw) == expected['sha256'], (label, name)
            records.append(dict(label=label, file=name, records=len(json.loads(raw)), sha256=digest(raw)))
        print(f'frozen {label}: 3,305 records identical', flush=True)
    output.write_text(json.dumps(records, indent=2))

def record(work, label, case, mode, allocation=False):
    binary = work / f'probe-{label}/target/release'
    path = work / f'fixtures/{case}.json'
    args = [str(binary / ('allocation' if allocation else 'palim-scale-competitor-probe'))]
    args += [str(path), mode] if allocation else [mode, str(path)]
    result = json.loads(command(args, timeout=240))
    raw = result.pop('wire').encode()
    result.update(label=label, fixture=case, mode=mode, wire_sha256=digest(raw))
    if allocation:
        assert result['allocation']['final_live_delta_bytes'] == 0
    else:
        result['core_median_ms'] = statistics.median(result['core_samples_ms'])
        result['pipeline_median_ms'] = statistics.median(result['pipeline_samples_ms'])
    return result

def measure(work, evidence, output):
    records, allocations, direct = [], [], []
    cases = [path.removesuffix('.json') for path in evidence['previous_evidence']['fixture_entries']]
    cases += ['shuffle-2000', 'shuffle-20000', 'shuffle-long-2000']
    # Every existing scale fixture, all four RFC contracts, paired before/after.
    # Rust json-patch is a control only for plain; fold-only isolates A on opt.
    for repeat in range(3):
        labels = ['baseline', 'candidate'] if repeat % 2 == 0 else ['candidate', 'baseline']
        for case in cases:
            for mode in MODES:
                for label in labels:
                    result = record(work, label, case, mode)
                    result['repeat'] = repeat
                    records.append(result)
            result = record(work, 'baseline', case, 'json-patch')
            result['repeat'] = repeat
            records.append(result)
            if case in ['scalar-guards-100000', 'file_migrations_move_copy_5000', 'disjoint-20000']:
                result = record(work, 'fold', case, 'optimized')
                result['repeat'] = repeat
                records.append(result)
            output.write_text(json.dumps(dict(records=records, allocations=allocations, direct=direct), indent=2))
            print(f'round {repeat + 1}/3 {case}', flush=True)
    for case, mode in [('shuffle-2000', 'plain'), ('shuffle-20000', 'plain'),
                       ('rotate-1000000', 'plain'), ('scalar-guards-100000', 'optimized'),
                       ('file_migrations_move_copy_5000', 'optimized')]:
        for label in ['baseline', 'candidate']:
            allocations.append(record(work, label, case, mode, True))
    for case in ['reverse-small', 'shuffle-small', 'rotate-one-small', 'rotate-half-small',
                 'nested-escaped-small', 'long-small', 'duplicates-small', 'nonpermutation-small']:
        for label in ['baseline', 'candidate']:
            path = work / f'fixtures/{case}.json'
            values = json.loads(command([str(work / f'probe-{label}/target/release/freeze-direct'), str(path)]))
            for value in values:
                raw = value.pop('wire').encode()
                value.update(label=label, fixture=case, wire_sha256=digest(raw))
                direct.append(value)
    output.write_text(json.dumps(dict(records=records, allocations=allocations, direct=direct), indent=2))
    print(f'{len(records)} timed processes, {len(allocations)} allocation processes, {len(direct)} direct records', flush=True)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workdir', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--phase', choices=['all', 'prepare', 'frozen', 'measure'], default='all')
    args = parser.parse_args()
    evidence = json.loads((HERE / 'measurements.json').read_text())
    if args.phase != 'measure':
        prepare(args.workdir, evidence)
        build(args.workdir)
    if args.phase in ['all', 'frozen']:
        frozen(args.workdir, evidence, args.output.with_suffix('.frozen.json'))
    if args.phase in ['all', 'measure']:
        measure(args.workdir, evidence, args.output)
