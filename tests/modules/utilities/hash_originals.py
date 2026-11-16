#!/usr/bin/env python3
"""Upstream's unchanged hash.php and hash.jenkins GoogleTests through a private PHP test
extension, against each engine's own hashes."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_ext import native_flags
from tools.ext_identity import attest, unchanged
from tools.harness import parse_engines, run_isolated, write_report
from coherent_sdk import stage

from tools.build_upstream import COMMIT as PIN
EXPECTED = ['hash.php', 'hash.jenkins']
# The lines of an engine's own swoole.h the originals use, filled in per engine.
DECLARATIONS = ('uint64_t swoole_random_int(', 'size_t swoole_random_bytes(', '#define SW_LOOP_N(')


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-config', default='php-config')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--build-manifest', type=Path, required=True, help="tools/build_ext.py's manifest of the selected engine")
    parser.add_argument('--archive', type=Path, required=True, help="tools/build_ext.py --native's SDK archive, whose patched tree supplies the selected SDK headers")
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    work = args.output.resolve().parent / ('hash-originals-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    report = dict(passed=False, created_utc=datetime.now(timezone.utc).isoformat(), work=str(work),
                  scope=__doc__, engines=[], results=[], expected_original_cases=EXPECTED,
                  limits='Two unchanged hash originals PHP/Jenkins with real random input provider,8192originalsizes andassertions. Austin signedbinary undefined-shift original andCRC32 unselected/unqualified. No fullgroup/39original inventory, worldbuild, sanitizer, fault/crash/OOM/lifetime or performance claim.')
    try:
        # The selected side's sources, headers and config.h are the Cargo build's.
        hybrid_flags = native_flags(args.archive)
        for name in ['hash_originals.cc', 'hash_originals.php']:
            (work / name).write_bytes(Path(__file__).with_name(name).read_bytes())
        original = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':core-tests/src/core/hash.cpp'])
        (work / 'pinned-hash.cpp').write_bytes(original)
        (work / 'hash_originals.inc').write_bytes(original[original.index(b'static const int hash_count'):])
        # Each engine's config takes its declarations from its own swoole.h.
        headers = {'native': subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':include/swoole.h'], text=True),
                   'selected': (ROOT / 'native/corebase/swoole.h').read_text()}
        for name, text in headers.items():
            lines = [line for line in text.splitlines() if line.startswith(DECLARATIONS)]
            if len(lines) != len(DECLARATIONS):
                raise ValueError(f'{name} swoole.h declarations changed: {DECLARATIONS}')
            config = '#include "swoole_hash.h"\n' + ''.join(line + '\n' for line in lines)
            config += '#define M14_GTEST_FILTER ' + json.dumps('--gtest_filter=' + ':'.join(EXPECTED)) + '\n'
            (work / (name + '-config')).mkdir()
            (work / (name + '-config/originals-config.h')).write_text(config)
        engines = parse_engines(args.engine, args.php, args.php_extension)
        report['engines'] = [engine.metadata() for engine in engines]
        if {bool(row['rust_feature_bits'] & 512) for row in report['engines']} != {False, True}:
            raise ValueError('Require native and selected utilities engines')
        selected = next(row for row in report['engines'] if row['rust_feature_bits'] & 512)
        report['source_attestation'] = attest(args.build_manifest, selected, 512)
        # Every helper gets its own engine's headers: the pinned SDK, or the Cargo build's.
        version = subprocess.check_output([args.php_config, '--version'], text=True).strip()
        if any(row['php'] != version for row in report['engines']):
            raise ValueError('PHP SDK differs from loaded runtime version')
        includes = shlex.split(subprocess.check_output([args.php_config, '--includes'], text=True))
        google_raw = shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs', 'gtest'], text=True))
        google = []
        flags = iter(google_raw)
        for flag in flags:
            # Vendor templates use their own warning policy; keep generated
            # helpers/original bodies under the unchanged project -Werror flags.
            if flag == '-I':
                google += ['-isystem', next(flags)]
            elif flag.startswith('-I'):
                google += ['-isystem', flag[2:]]
            else:
                google.append(flag)
        library = Path(subprocess.check_output(['pkg-config', '--variable=libdir', 'gtest'], text=True).strip())
        report['google_test'] = dict(version=subprocess.check_output(['pkg-config', '--modversion', 'gtest'], text=True).strip(),
                                     pkg_config_flags=google_raw, flags=google, libraries=[str(p.resolve()) for p in library.glob('libgtest.*') if p.is_file()])
        if not report['google_test']['libraries']:
            raise ValueError('GoogleTest library identity unavailable')
        native_identity = next(row for row in report['engines'] if not row['rust_feature_bits'] & 512)
        report['coherent_sdk'] = {'native': stage(work / 'native', Path(native_identity['extension']).parents[1], args.source, PIN),
                                  'selected': dict(flags=hybrid_flags)}
        sdk_headers = {'native': ['-I' + str(work / 'native/include')], 'selected': hybrid_flags}
        openssl = shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True))
        helpers = {}
        for engine in ['native', 'selected']:
            helper = work / ('m14_' + engine + '.so')
            command = ['c++', '-std=c++17', '-O2', '-pthread', '-Wall', '-Wextra', '-Werror', *includes,
                       '-DHAVE_CONFIG_H', *sdk_headers[engine], *openssl, '-I' + str(work / (engine + '-config')),
                       '-I' + str(work), *(['-bundle', '-undefined', 'dynamic_lookup'] if sys.platform == 'darwin' else ['-shared', '-fPIC']),
                       '-MD', '-MF', str(work / (engine + '.d')), str(work / 'hash_originals.cc'), *google, '-o', str(helper)]
            result = run_isolated(command, timeout=60)
            report[engine + '_compilation'] = dict(command=command, exit=result.returncode, stdout=result.stdout,
                                                   stderr=result.stderr, php_sdk=version)
            if result.returncode:
                raise ValueError(f'{engine} helper compilation failed')
            symbols = run_isolated(['nm', '-u', str(helper)], timeout=15)
            report[engine + '_imports'] = dict(exit=symbols.returncode, stdout=symbols.stdout, stderr=symbols.stderr)
            # The selected SDK may define the hashes inline over its Rust exports.
            required = ['swoole_hash_php', 'swoole_hash_jenkins', 'swoole_random_int', 'swoole_random_bytes'] if engine == 'native' else []
            if symbols.returncode or any(name not in symbols.stdout for name in required):
                raise ValueError(f'{engine} helper lacks the imported production hashes')
            helpers[engine] = helper
        for engine, identity in zip(engines, report['engines']):
            with tempfile.TemporaryDirectory(prefix='m14-hash-') as directory:
                helper = helpers['selected' if identity['rust_feature_bits'] & 512 else 'native']
                result = run_isolated(engine.command('-d', 'extension=' + str(helper), str(work / 'hash_originals.php'), directory,
                                                     str(len(EXPECTED))),
                                      timeout=30, env={**os.environ, 'TZ': 'UTC0', 'LC_ALL': 'C'})
                row = dict(engine=engine.name, exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                           extension_sha256_after=sha(identity['extension']), passed=False,
                           files={p.name: dict(content=p.read_text()) for p in sorted(Path(directory).iterdir()) if p.is_file()})
                report['results'].append(row)
                if result.returncode == 0 and not result.stderr and row['extension_sha256_after'] == identity['sha256']:
                    cases = list(ET.fromstring(row['files']['gtest.xml']['content']).iter('testcase'))
                    row['original_cases'] = [dict(case.attrib) for case in cases]
                    if ({case.get('classname') + '.' + case.get('name') for case in cases} != set(EXPECTED)
                            or len(cases) != len(EXPECTED) or any(case.get('status') != 'run' or case.get('result') != 'completed' or list(case) for case in cases)):
                        raise ValueError('Original GoogleTest XML did not pass selected unchanged cases')
                    row['summary'] = json.loads(result.stdout.splitlines()[-1])
                    row['passed'] = True
        if failed := [row for row in report['results'] if not row['passed']]:
            raise ValueError('Hash originals failed; all engine outputs retained: ' + '; '.join(
                f"{row['engine']} exit {row['exit']}, stderr {row['stderr'][-400:]!r}" for row in failed))
        unchanged(args.build_manifest, report['source_attestation'])
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
