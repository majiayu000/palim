"""Rebuild frozen optimization steps and replay their benchmark boundaries."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import tempfile
import zipfile


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--quick', action='store_true', help='two small cases, one process per configuration')
    args = parser.parse_args()
    report = Path(__file__).resolve().parent
    repo = report.parent.parent
    data = json.loads((report / 'measurements.json').read_text())
    previous = data['previous_evidence']
    archive_path = repo / previous['path']
    assert digest(archive_path.read_bytes()) == previous['sha256']
    root = Path(tempfile.mkdtemp(prefix='palim-opt-rotate-'))
    print(root, flush=True)
    fixtures = root / 'fixtures'
    fixtures.mkdir()
    with zipfile.ZipFile(archive_path) as archive:
        for name, entry in previous['fixture_entries'].items():
            (fixtures / name).write_bytes(archive.read(entry))
    for name, content in data['extra_fixtures'].items():
        (fixtures / name).write_text(content)
    for section in [*data['steps'], data['comparison']]:
        for name, expected in section['environment']['fixture_hashes'].items():
            assert digest((fixtures / (name + '.json')).read_bytes()) == expected
        aliases = section.get('source_aliases', {})
        for label, files in section['environment']['source_hashes'].items():
            for relative, expected in files.items():
                assert digest(data['frozen_sources'][aliases.get(label, label)][relative].encode()) == expected

    # Build all versions before any timing; sources and locked dependencies
    # are independent of the caller's current checkout and Git history.
    for label, files in data['frozen_sources'].items():
        for relative, content in files.items():
            path = root / label / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        probe = root / ('probe-' + label)
        (probe / 'src').mkdir(parents=True)
        (probe / 'Cargo.toml').write_text(data['probe']['cargo'].replace('../final', '../' + label))
        (probe / 'Cargo.lock').write_text(data['probe']['lock'])
        for name, content in data['probe']['sources'].items():
            (probe / 'src' / name).write_text(content)
        subprocess.run(['cargo', 'build', '--release', '--locked', '--manifest-path',
                        str(probe / 'Cargo.toml')], check=True)

    records, allocations = [], []

    def save(name, value):
        (root / name).write_text(json.dumps(value, indent=2) + '\n')

    def executable(label, aliases, binary):
        source = aliases.get(label, label)
        if source == 'json-patch':
            source = 'final'
        return root / ('probe-' + source) / 'target/release' / binary

    def run(section, aliases, case, mode, label, repeat):
        row = json.loads(subprocess.check_output([
            str(executable(label, aliases, 'palim-scale-competitor-probe')),
            'json-patch' if label == 'json-patch' else mode, str(fixtures / (case + '.json'))], timeout=240))
        assert row['self_verified']
        row.update(section=section, label=label, case=case, mode=mode, repeat=repeat,
                   wire_sha256=digest(row.pop('wire').encode()))
        for metric in ['core', 'pipeline']:
            row[metric + '_ms'] = statistics.median(row[metric + '_samples_ms'])
        records.append(row)
        save('records.json', records)
        print(section, label, case, mode, row['core_ms'], row['pipeline_ms'], flush=True)

    for section in [*data['steps'], dict(name='comparison', **data['comparison'])]:
        name = section['name']
        aliases = section.get('source_aliases', {})
        original = section['records']
        cases = list(dict.fromkeys((r['case'], r['mode']) for r in original))
        if args.quick:
            cases = [pair for pair in cases if pair[0] in ['small-config-edit', 'file_migrations_move_copy_5000']
                     and pair[1] == ('plain' if name == 'comparison' else 'optimized')]
        for repeat in range(1 if args.quick else 3):
            for case, mode in cases:
                labels = list(dict.fromkeys(r['label'] for r in original if r['case'] == case and r['mode'] == mode))
                if repeat % 2:
                    labels.reverse()
                for label in labels:
                    run(name, aliases, case, mode, label, repeat)
        selected = [r for r in records if r['section'] == name]
        for case, mode in cases:
            paired = [r for r in selected if r['case'] == case and r['mode'] == mode and r['label'] != 'json-patch']
            assert len({r['wire_sha256'] for r in paired}) == 1
        probes = section['allocations']
        if args.quick:
            probes = [r for r in probes if r['case'] == 'file_migrations_move_copy_5000' and r['mode'] == 'optimized']
        for old in probes:
            label, case, mode = old['label'], old['case'], old['mode']
            row = json.loads(subprocess.check_output([
                str(executable(label, aliases, 'allocation')), str(fixtures / (case + '.json')),
                'json-patch' if label == 'json-patch' else mode], timeout=240))
            assert row['roundtrip_verified'] and row['allocation']['final_live_delta_bytes'] == 0
            row.update(section=name, label=label, case=case, mode=mode,
                       wire_sha256=digest(row.pop('wire').encode()))
            assert row['wire_sha256'] == old['wire_sha256']
            allocations.append(row)
        save('allocations.json', allocations)
    save('replay.json', dict(quick=args.quick, timing_processes=len(records), allocation_probes=len(allocations),
        frozen_source_hashes_verified=True, fixture_hashes_verified=True, forward_and_inverse_verified=True))
    print('COMPLETE', root, flush=True)


if __name__ == '__main__':
    main()
