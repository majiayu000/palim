"""Rebuild the frozen CI-fix source and replay the cross-language comparison."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--quick', action='store_true', help='one process on small controls and shuffle prototypes')
    args = parser.parse_args()
    report = Path(__file__).resolve().parent
    repo = report.parent.parent
    data = json.loads((report / 'measurements.json').read_text())
    evidence = repo / data['previous_evidence']['path']
    assert hashlib.sha256(evidence.read_bytes()).hexdigest() == data['previous_evidence']['sha256']
    root = Path(tempfile.mkdtemp(prefix='palim-ci-crosslang-'))
    cross = root / 'cross'
    cross.mkdir()
    print(root, flush=True)
    with zipfile.ZipFile(evidence) as archive:
        for name in archive.namelist():
            if name.startswith(('fixtures/', 'rust/', 'go/', 'js/')) and not name.endswith('/'):
                path = cross / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.read(name))
    source = cross / 'source'
    source.mkdir()
    archived = subprocess.check_output([
        'git', 'archive', data['candidate_commit'], 'src', 'Cargo.toml', 'Cargo.lock', 'README.md', 'benches'
    ], cwd=repo)
    with tarfile.open(fileobj=io.BytesIO(archived)) as archive:
        archive.extractall(source, filter='data')
    matrix = data['cross_language']
    for name, content in matrix['extra_fixtures'].items():
        (cross / 'fixtures' / name).write_text(content)
    for relative, expected in matrix['environment']['hashes'].items():
        path = cross / relative
        if path.is_file():
            assert hashlib.sha256(path.read_bytes()).hexdigest() == expected, relative
    (cross / 'registry.json').write_text(json.dumps(matrix['registry'], indent=2) + '\n')
    runner = matrix['runner_source']
    if args.quick:
        runner = runner.replace('registry=json.loads', "cases=[c for c in cases if c['case']=='small-config-edit']\nregistry=json.loads")
        runner = runner.replace('range(3)', 'range(1)').replace('3 independent processes', '1 quick-check process')
    (cross / 'run.py').write_text(runner)

    # Finish all builds before any timings. Prototypes remain outside the repo.
    subprocess.run(['cargo', 'build', '--release', '--locked', '--bin', 'palim-scale-competitor-probe'], cwd=cross / 'rust', check=True)
    subprocess.run(['go', 'build', '-mod=readonly', '-o', 'probe', '.'], cwd=cross / 'go', check=True)
    subprocess.run(['npm', 'ci', '--ignore-scripts', '--no-audit', '--no-fund'], cwd=cross / 'js', check=True)
    for label, variant in data['source_variants'].items():
        target = root / label
        shutil.copytree(source, target)
        for relative, content in variant['overrides'].items():
            (target / relative).write_text(content)
        probe = root / ('probe' if label == 'profile' else label + '-probe')
        (probe / 'src').mkdir(parents=True)
        (probe / 'Cargo.toml').write_text(variant['probe_cargo'])
        (probe / 'Cargo.lock').write_text(variant['probe_lock'])
        for name, content in variant['probe_sources'].items():
            (probe / 'src' / name).write_text(content)
        subprocess.run(['cargo', 'build', '--release', '--locked', '--bin', 'palim-scale-competitor-probe'], cwd=probe, check=True)
        if label == 'profile':
            subprocess.run(['cargo', 'build', '--release', '--locked', '--bin', 'profile'], cwd=probe, check=True)

    subprocess.run(['python3', 'run.py'], cwd=cross, check=True)
    candidates = data['scratch_candidates']['runner_source']
    if args.quick:
        candidates = candidates.replace('records=[]', "configs=[c for c in configs if c[0] in ['small-config-edit','shuffle-2000']]\nrecords=[]")
        candidates = candidates.replace('range(3)', 'range(1)')
    (root / 'compare-candidates.py').write_text(candidates)
    (root / 'shuffle-2000.json').write_text(matrix['extra_fixtures']['shuffle-2000.json'])
    subprocess.run(['python3', 'compare-candidates.py'], cwd=root, check=True)
    print('Replayed measurements:', cross / 'measurements')
    print('Scratch candidate results:', root / 'candidate-summary.json')
    print('Profile binary:', root / 'probe/target/release/profile')


if __name__ == '__main__':
    main()
