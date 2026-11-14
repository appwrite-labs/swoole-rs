#!/usr/bin/env python3
"""h2spec (pinned) against an open_http2_protocol server, h2c and TLS, on every engine.

A case that passes on upstream and not on another engine fails the run; upstream's own
failures are listed, not hidden. Strict cases run too: they only gate where upstream passes.

    python3 tests/modules/http2/h2spec.py --engine upstream=target/upstream/src/modules/swoole.so \
      --engine cargo=target/ext/swoole.so
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import platform
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, server, write_report

VERSION = 'v2.6.0'
SHA256 = {
    'linux': '157ee0de702e01ad40e752dbf074b366027e550c8e7504f9450da2809e279318',
    'darwin': '981cb9f90a6f5e36300063022bd4eb7438d3dcf66d63a146a8541359697d1601',
}


def h2spec():
    """The pinned release binary, downloaded once into target/h2spec/."""
    system = platform.system().lower()
    binary = ROOT / 'target/h2spec' / VERSION / system / 'h2spec'
    if not binary.exists():
        url = f'https://github.com/summerwind/h2spec/releases/download/{VERSION}/h2spec_{system}_amd64.tar.gz'
        data = urllib.request.urlopen(url, timeout=60).read()
        if hashlib.sha256(data).hexdigest() != SHA256[system]:
            raise RuntimeError(f'{url}: SHA256 mismatch')
        binary.parent.mkdir(parents=True, exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            binary.write_bytes(archive.extractfile('h2spec').read())
        binary.chmod(0o755)
    return binary


def cases(junit):
    """{case id: (description, passed|failed|error|skipped, message)} from h2spec's JUnit report."""
    rows = {}
    for suite in ET.parse(junit).getroot().iter('testsuite'):
        for index, case in enumerate(suite.iter('testcase'), 1):
            outcome = next(((tag, case.find(tag).text or '') for tag in ('failure', 'error', 'skipped')
                            if case.find(tag) is not None), ('passed', ''))
            rows[f"{suite.get('package')}/{index}"] = (case.get('classname'), *outcome)
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--engine', action='append', required=True, help='NAME=/path/to/swoole.so; one must be upstream')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/h2spec.json')
    args = parser.parse_args()
    engines = parse_engines(args.engine, args.php, args.php_extension)
    if 'upstream' not in [engine.name for engine in engines]:
        parser.error('the gate needs an engine named upstream')
    binary = h2spec()
    logs = args.output.resolve().parent / (args.output.stem + '-logs')
    logs.mkdir(parents=True, exist_ok=True)
    results = {}
    with tempfile.TemporaryDirectory(prefix='h2spec-') as work:
        tls = Path(work) / 'server'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=localhost',
                        '-keyout', f'{tls}.key', '-out', f'{tls}.crt'], check=True, capture_output=True)
        def run(engine, mode, env, flags):
            junit = Path(work) / f'{engine.name}-{mode}.xml'
            with server(engine, Path(__file__).with_suffix('.php'), env) as (port, _), \
                    (logs / f'{engine.name}-{mode}.log').open('w') as log:
                # Exit 1 means some cases failed, which the comparison below judges.
                subprocess.run([binary, '--strict', '-p', str(port), *flags, '-j', junit], stdout=log,
                               stderr=subprocess.STDOUT, timeout=600)
            if not junit.exists():
                raise RuntimeError(f'{engine.name} {mode}: h2spec wrote no report; see {logs}')
            return engine.name, mode, cases(junit)

        # Most of a run is h2spec waiting out its 2 s timeouts, so the runs go side by side.
        modes = [('h2c', {'TEST_TLS': ''}, []), ('tls', {'TEST_TLS': str(tls)}, ['-t', '-k'])]
        with ThreadPoolExecutor() as pool:
            for name, mode, rows in pool.map(lambda job: run(*job), [(e, *m) for e in engines for m in modes]):
                results.setdefault(name, {})[mode] = rows
    oracle = results['upstream']
    report = {'h2spec': VERSION, 'engines': [engine.metadata() for engine in engines], 'results': {}, 'regressions': []}
    lines = ['| engine | mode | passed | failed | skipped |', '| --- | --- | --- | --- | --- |']
    for name, modes in results.items():
        for mode, rows in modes.items():
            counts = {outcome: sum(row[1] == outcome for row in rows.values()) for outcome in ('passed', 'failed', 'error', 'skipped')}
            report['results'].setdefault(name, {})[mode] = {
                **counts, 'not_passed': {case: {'desc': row[0], 'outcome': row[1], 'message': row[2]}
                                         for case, row in sorted(rows.items()) if row[1] != 'passed'}}
            lines.append(f"| {name} | {mode} | {counts['passed']} | {counts['failed'] + counts['error']} | {counts['skipped']} |")
            if name == 'upstream':
                continue
            for case, row in sorted(oracle[mode].items()):
                if row[1] == 'passed' and rows.get(case, (None, 'missing'))[1] != 'passed':
                    report['regressions'].append({'engine': name, 'mode': mode, 'case': case, 'desc': row[0],
                                                  'outcome': rows.get(case, (None, 'missing'))[1]})
    write_report(args.output, report)
    # Markdown, for the job summary.
    print('\n'.join(lines))
    if report['regressions']:
        print('\nRegressions (pass on upstream, not here):\n')
    for row in report['regressions']:
        print(f"- {row['engine']} {row['mode']} `{row['case']}` {row['desc']}: {row['outcome']}")
    print('\n<details><summary>Cases upstream fails too</summary>\n')
    for mode, rows in oracle.items():
        for case, row in sorted(rows.items()):
            if row[1] in ('failed', 'error'):
                print(f'- {mode} `{case}` {row[0]}')
    print('\n</details>')
    empty = not all(oracle.values())
    if empty:
        print('\nupstream ran no h2spec cases')
    return int(bool(report['regressions']) or empty)


if __name__ == '__main__':
    sys.exit(main())
