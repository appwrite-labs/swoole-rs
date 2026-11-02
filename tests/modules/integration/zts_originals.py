#!/usr/bin/env python3
"""Run named correctness inventories on existing verified ZTS worlds without rebuilding."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import uuid


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--checkout', type=Path, required=True)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--zts-report', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--php', default='php')
    p.add_argument('--profile', choices=['stdext-originals', 'collections', 'statistics'], default='stdext-originals',
                   help='Separate original, collection and read-only statistics profiles; no cross-profile qualification')
    a = p.parse_args()
    sys.path.insert(0, str(a.checkout.resolve()))
    from tools.build_upstream import COMMIT
    from tools.harness import Engine, run_isolated, write_report
    output = a.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    work = output.parent / ('zts-originals-' + uuid.uuid4().hex)
    report = dict(passed=False, executed=False, work=str(work), commands=[], profile=a.profile,
                  scope='Complete unchanged stdext originals on these existing real ZTS artifacts only; no other inventory, lifetime, platform or performance qualification.')
    if a.profile == 'collections':
        report['scope'] = 'Defined ordinary collection and binary-column fixtures on this real ZTS pair; no concurrent Table, full originals, lifetime, other platform or performance qualification.'
    if a.profile == 'statistics':
        report['scope'] = 'Read-only Coroutine statistics on this real ZTS pair; no Coroutine/Event/callback creation, scheduler/lifetime, originals or performance qualification.'
    try:
        zts = json.loads(a.zts_report.read_text())
        if zts.get('passed') is not True or zts.get('php', [None, False])[1] is not True:
            raise ValueError('Successful real ZTS producer required')
        engines = zts['engines']
        if len(engines) != 2 or {r['name'] for r in engines} != {'native', 'selected'}:
            raise ValueError('Exact native/selected ZTS pair required')
        archive = Path(zts['work']) / 'rust-prefix/lib/libswoole_rs_php.a'
        if sha(archive) != zts['producer']['archive_sha256']:
            raise ValueError('Producer Rust archive changed')
        for row in engines:
            if sha(row['extension']) != row['extension_sha256']:
                raise ValueError('ZTS extension changed: ' + row['name'])
            actual = Engine(row['name'], Path(row['extension']), a.php, ('sockets',)).metadata()
            if (actual['php'], actual['zts'], actual['rust_feature_bits']) != (
                    zts['php'][0], True, row['metadata']['rust_feature_bits']):
                raise ValueError('Actual PHP/ZTS/feature identity differs')
        report['producer'] = dict(source_ref=zts['producer']['source_ref'], engines=engines)
        work.mkdir()

        if a.profile == 'statistics':
            selected = next(row for row in engines if row['name'] == 'selected')
            if not selected['metadata']['rust_feature_bits'] & 134217728:
                raise ValueError('Selected real ZTS artifact must enable M29')
            # The independently identified driver supplies this read-only fixture;
            # it need not have existed in the frozen producer's compile-only stage.
            fixture = Path(__file__).resolve().parents[1] / 'phpcoroutines/statistics.php'
            identity = sha(fixture)
            report.update(fixture=str(fixture), results=[])
            for engine in engines:
                command = Engine(engine['name'], Path(engine['extension']), a.php, ('sockets',)).command(fixture)
                report['executed'] = True
                result = run_isolated(command, timeout=30, cwd=work)
                observed = dict(engine=engine['name'], command=command, exit=result.returncode,
                                stdout=result.stdout, stderr=result.stderr, passed=False)
                report['results'].append(observed)
                if result.returncode == 0 and not result.stderr:
                    fields = json.loads(result.stdout)
                    valid = (isinstance(fields, list) and len(fields) == 2 and all(
                        isinstance(row, dict) and len(row) in (9, 12) and all(
                            type(value) is int for value in row.values()) for row in fields))
                    observed.update(observations=fields, passed=valid)
                write_report(output, report)
            if sha(fixture) != identity or sha(archive) != zts['producer']['archive_sha256'] or any(
                    sha(row['extension']) != row['extension_sha256'] for row in engines):
                raise ValueError('Immutable statistics input changed')
            report['passed'] = all(row['passed'] for row in report['results']) and (
                report['results'][0].get('observations') == report['results'][1].get('observations'))
            if not report['passed']:
                report['error'] = 'ZTS statistics differ or query failed; first outputs retained'
            write_report(output, report)
            print(json.dumps({key: report.get(key) for key in ['passed', 'executed', 'error', 'work']}))
            return int(not report['passed'])

        if a.profile == 'collections':
            selected = next(row for row in engines if row['name'] == 'selected')
            if not selected['metadata']['rust_feature_bits'] & 33554432:
                raise ValueError('Selected real ZTS artifact must enable M27')
            report['fixtures'] = []
            for name, count in [('php_collections.php', 324), ('php_column_bytes.php', 16)]:
                fixture = a.checkout / 'tests/modules/phpcollections' / name
                identity = sha(fixture)
                observations = []
                row = dict(fixture=str(fixture), expected_checks=count, results=observations)
                report['fixtures'].append(row)
                for engine in engines:
                    command = Engine(engine['name'], Path(engine['extension']), a.php, ('sockets',)).command(fixture)
                    report['executed'] = True
                    result = run_isolated(command, timeout=30, cwd=work)
                    observed = dict(engine=engine['name'], command=command, exit=result.returncode,
                                    stdout=result.stdout, stderr=result.stderr, passed=False)
                    observations.append(observed)
                    if result.returncode == 0 and not result.stderr:
                        try:
                            summary = json.loads(result.stdout)
                            observed.update(summary=summary, passed=isinstance(summary, dict) and summary.get('checks') == count)
                        except ValueError:
                            observed['error'] = 'Collection summary is not JSON'
                    write_report(output, report)
                if sha(fixture) != identity:
                    raise ValueError('Collection fixture changed during execution')
                row['passed'] = all(item['passed'] for item in observations) and len({
                    json.dumps(item.get('summary'), sort_keys=True) for item in observations}) == 1
            if sha(archive) != zts['producer']['archive_sha256'] or any(
                    sha(row['extension']) != row['extension_sha256'] for row in engines):
                raise ValueError('Immutable artifact changed during collection execution')
            report['passed'] = all(row['passed'] for row in report['fixtures'])
            if not report['passed']:
                report['error'] = 'Collection inventory failed; all first outputs retained'
            write_report(output, report)
            print(json.dumps({key: report.get(key) for key in ['passed', 'executed', 'error', 'work']}))
            return int(not report['passed'])

        def run(command):
            result = subprocess.run(list(map(str, command)), capture_output=True, text=True, timeout=900)
            report['commands'].append(dict(command=list(map(str, command)), exit=result.returncode,
                                            stdout=result.stdout, stderr=result.stderr))
            write_report(output, report)
            if result.returncode:
                raise ValueError('Command failed; first output preserved')

        # Configured ZTS trees have no Git HEAD. A separate pinned clone supplies
        # original blobs; matching phpize supplies the official PHP runner.
        source = work / 'source'
        run(['git', 'clone', '--shared', '--no-checkout', a.source, source])
        run(['git', '-C', source, 'checkout', '--detach', COMMIT])
        selected = next(r for r in engines if r['name'] == 'selected')
        runner = Path(selected['extension']).parents[1] / 'run-tests.php'
        shutil.copy2(runner, source / 'run-tests.php')
        manifest, originals = work / 'manifest.json', work / 'originals.json'
        run([sys.executable, a.checkout / 'corpus/upstream/tree.py', '--source', source,
             '--group', 'swoole_stdext', '--module', 'phphelpers', '--requires', 'stdext', '--output', manifest])
        report['executed'] = True
        run([sys.executable, a.checkout / 'corpus/upstream/run.py', '--source', source, '--manifest', manifest,
             '--php', a.php, '--php-extension', 'sockets', '--require-all', '--isolate-cases', '--output', originals,
             *[arg for row in engines for arg in ['--engine', row['name'] + '=' + row['extension']]]])
        outcome = json.loads(originals.read_text())
        report['originals'] = dict(path=str(originals), outcome=outcome)
        expected = {(row['name'], test['path']) for row in engines
                    for test in json.loads(manifest.read_text())['tests']}
        observed = [(row['engine'], row['test']) for row in outcome.get('results', [])]
        if len(observed) != len(expected) or set(observed) != expected:
            raise ValueError('Original engine/test inventory differs from complete manifest')
        if outcome.get('execution_status') != 'complete' or not outcome.get('results') or any(
                row['status'] != 'passed' for row in outcome['results']):
            raise ValueError('Complete originals did not all pass')
        if sha(archive) != zts['producer']['archive_sha256'] or any(
                sha(row['extension']) != row['extension_sha256'] for row in engines):
            raise ValueError('Immutable artifact changed during originals')
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    write_report(output, report)
    print(json.dumps({k: report.get(k) for k in ['passed', 'executed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
