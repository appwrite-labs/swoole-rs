#!/usr/bin/env python3
"""Real Zend named C++ entry through a private test-only PHP extension; no production ABI edit."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import sys
import uuid

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import require_build_space
from tools.harness import parse_engines, run_isolated, write_report
from tools.hybrid_identity import attest

from tools.build_upstream import COMMIT as PIN
PUBLIC_HEADER_SHA256 = 'ca26ee5b99862115c6cd069568624ea1af89310fa5909e4419bd0d0895e32e7f'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def build_helper(sdk_path, work, report, profile="native"):
    sdk = json.loads(sdk_path.read_text())
    if not sdk.get('passed'):
        raise ValueError('Require completed passing private SDK report')
    rows = [row for row in sdk['commands'] if row['engine'] == profile and not row['stdext_enabled']
            and row.get('cpp_standard') == 'c++14' and row['unit'] == 'ext-src/php_swoole_cxx.cc']
    if len(rows) != 1:
        raise ValueError('Require one '+profile+' C++14 common-helper SDK profile')
    row = rows[0]
    for path, digest in row['dependencies_sha256'].items():
        if sha(path) != digest:
            raise ValueError('SDK dependency changed: ' + path)
    native_header = Path(sdk['work']) / 'headers-native/ext-src/php_swoole_cxx.h'
    if sdk['pin'] != PIN or sdk['mirror_sha256']['ext-src/php_swoole_cxx.h'] != PUBLIC_HEADER_SHA256 or sha(native_header) != PUBLIC_HEADER_SHA256:
        raise ValueError('Native public C++ header differs from pinned ABI')
    consumed=[Path(path) for path in row['dependencies_sha256'] if Path(path).name=='php_swoole_cxx.h']
    if len(consumed)!=1:raise ValueError('Require one actually consumed facade header')
    public_header=consumed[0]
    expected=PUBLIC_HEADER_SHA256 if profile=='native' else sdk.get('selected_header_sha256')
    if sha(public_header)!=expected or (profile!='native' and not sdk.get('layout_matched')):
        raise ValueError('Selected public header/layout identity differs from qualified SDK')
    command = [item for item in row['command'][:row['command'].index('-MD')] if item != '-fsyntax-only']
    helper = work / 'm26_helpers.so'
    command += ['-O2', *(['-bundle', '-undefined', 'dynamic_lookup'] if sys.platform == 'darwin' else ['-shared', '-fPIC']),
                '-MD', '-MF', str(work / 'entrypoint.d'), str(work / 'entrypoint.cc'), '-o', str(helper)]
    require_build_space(work)
    result = run_isolated(command, timeout=120)
    report['helper_compilation'] = dict(command=command, exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                                        sdk_profile=profile, cpp_standard='c++14', sdk_php=sdk['php_version'])
    if result.returncode:
        raise ValueError('Test-only helper compilation failed')
    import shlex
    paths = shlex.split((work / 'entrypoint.d').read_text().replace('\\\n', ' ').split(':', 1)[1])
    # Consumers re-verify these before reusing the helper.
    report['helper_compilation']['dependencies_sha256'] = {str(Path(path).resolve()): sha(path) for path in paths}
    return helper


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sdk-report', type=Path, required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--source-ref', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    work = output.parent / ('phphelpers-entry-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    report = dict(created_utc=datetime.now(timezone.utc).isoformat(), passed=False, work=str(work),
                  scope=__doc__, engines=[], results=[], child_results=[])
    try:
        for name in ['entrypoint.cc', 'entrypoint.php', 'entrypoint-child.php']:
            (work / name).write_bytes(Path(__file__).with_name(name).read_bytes())
        engines = parse_engines(args.engine, args.php, args.php_extension)
        report['engines'] = [engine.metadata() for engine in engines]
        if {bool(item['rust_feature_bits'] & 16777216) for item in report['engines']} != {False, True}:
            raise ValueError('Require native and phphelpers selected engines')
        selected = next(item for item in report['engines'] if item['rust_feature_bits'] & 16777216)
        report['source_attestation'] = attest(args.build_manifest, args.archive, selected, args.source_ref, 16777216)
        helper = build_helper(args.sdk_report, work, report)
        if any(item['php'] != report['helper_compilation']['sdk_php'] for item in report['engines']):
            raise ValueError('Helper SDK and actual PHP engine versions differ')
        for engine, identity in zip(engines, report['engines']):
            result = run_isolated(engine.command('-d', 'extension=' + str(helper), str(work / 'entrypoint.php')), timeout=30)
            row = dict(engine=engine.name, exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                       extension_sha256_after=sha(identity['extension']), passed=False)
            report['results'].append(row)
            row['passed'] = result.returncode == 0 and not result.stderr and row['extension_sha256_after'] == identity['sha256']
            if row['passed']:
                try:
                    row['summary'] = json.loads(result.stdout)
                except ValueError:
                    row['passed'] = False
        if not all(row['passed'] for row in report['results']):
            raise ValueError('Named entry fixture failed; every engine first output retained')
        if len({json.dumps(row['summary'], sort_keys=True) for row in report['results']}) != 1:
            raise ValueError('Named entry native/selected summaries differ')
        for engine, identity in zip(engines, report['engines']):
            for mode in ['direct', 'coroutine']:
                for outcome in ['return', 'exception', 'missing']:
                    result = run_isolated(engine.command('-d', 'extension=' + str(helper), '-d', 'display_errors=stderr', '-d', 'log_errors=0',
                                                         str(work / 'entrypoint-child.php'), mode, outcome), timeout=30)
                    row = dict(engine=engine.name, mode=mode, outcome=outcome, exit=result.returncode, stdout=result.stdout,
                               stderr=result.stderr, extension_sha256_after=sha(identity['extension']), passed=False)
                    report['child_results'].append(row)
                    try:
                        lines = result.stdout.splitlines()
                        row['summary'] = json.loads(lines[-1])
                        expected = dict(coroutine=mode == 'coroutine', outcome=outcome, code=0 if outcome == 'return' else 255, signal=0)
                        markers = [] if outcome == 'missing' else ['M26 named enter'] + (['M26 named return'] if outcome == 'return' else [])
                        diagnostic = (not result.stderr if outcome == 'return' else
                                      ('thrown in' in result.stderr and ('RuntimeException: M26 ordinary named exception' in result.stderr
                                                                       if outcome == 'exception' else 'M26_named_missing_callback' in result.stderr)))
                        row['passed'] = (result.returncode == 0 and row['summary'] == expected and lines[:-1] == markers and diagnostic
                                         and row['extension_sha256_after'] == identity['sha256'])
                    except (ValueError, IndexError):
                        pass
        if not all(row['passed'] for row in report['child_results']):
            raise ValueError('Named native-only child outcome failed; first diagnostics retained')
        diagnostics = {}
        for row in report['child_results']:
            diagnostics.setdefault((row['mode'], row['outcome']), set()).add(row['stderr'])
        if any(len(values) != 1 for values in diagnostics.values()):
            raise ValueError('Named native/selected child diagnostics differ')
        report['matched_child_diagnostics'] = True
        receipt = report['source_attestation']
        if sha(args.build_manifest) != receipt['manifest_sha256'] or sha(args.archive) != receipt['archive_sha256']:
            raise ValueError('Selected production receipt/archive changed')
        if any(sha(path) != digest for path, digest in report['helper_compilation']['dependencies_sha256'].items()):
            raise ValueError('Helper build dependency changed')
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
