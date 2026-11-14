#!/usr/bin/env python3
"""Actual public Logger storage/filename methods through a private PHP test extension."""
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
from storage_summary import compare

from tools.build_upstream import COMMIT as PIN
HEADER = '3e6dd302ac18d32f9a173dcbfca0680f16b9682a5f5031456099cda5b1bbb180'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def public(*prefixes, count=None):
    """A placeholder for the lines of an engine's own swoole.h that start with prefixes,
    filled in per engine when the originals config is written."""
    return '@@' + json.dumps([prefixes, count]) + '@@\n'


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
    phase = parser.add_mutually_exclusive_group()
    phase.add_argument('--backtrace', action='store_true', help='Run normal enabled/filter/reset backtrace behavior instead of storage/concurrent reopen')
    phase.add_argument('--pretty', action='store_true', help='Compare actual public pretty-name method against the unchanged pinned body')
    phase.add_argument('--pretty-originals', action='store_true', help='Execute two unchanged pretty-name original GoogleTests through the actual public method')
    phase.add_argument('--logger-originals', action='store_true', help='Execute unchanged rotation, memory-stream and pretty-name original GoogleTests')
    phase.add_argument('--hash-originals', action='store_true', help='Execute unchanged defined PHP/Jenkins hash original GoogleTests')
    args = parser.parse_args()
    originals = args.pretty_originals or args.logger_originals or args.hash_originals
    fixture = 'pretty_originals' if originals else 'pretty' if args.pretty else 'backtrace' if args.backtrace else 'storage'
    work = args.output.resolve().parent / ('logger-storage-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    report = dict(passed=False, created_utc=datetime.now(timezone.utc).isoformat(), work=str(work),
                  scope=__doc__, fixture=fixture, engines=[], results=[])
    try:
        # The selected side's sources, headers and config.h are the Cargo build's.
        hybrid_flags = native_flags(args.archive)
        for name in [fixture + '.cc', fixture + '.php', 'storage_summary.py'] + ['coherent_sdk.py']:
            path = Path(__file__).with_name(name)
            (work / name).write_bytes(path.read_bytes())
        header = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':include/swoole_log.h'])
        if hashlib.sha256(header).hexdigest() != HEADER:
            raise ValueError('Public Logger header differs from pinned class')
        if originals:
            original = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':core-tests/src/core/log.cpp'])
            (work / 'pinned-log.cpp').write_bytes(original)
            excerpt = original[original.index(b'namespace TestA {'):original.index(b'TEST(log, ignore_error)')]
            expected = ['log.pretty_name', 'log.pretty_name_lambda']
            config = ''
            if args.logger_originals:
                rotation = original[original.index(b'TEST(log, rotation)'):original.index(b'TEST(log, redirect_1)')]
                stream = original[original.index(b'TEST(log, set_stream)'):]
                excerpt = rotation + excerpt + stream
                expected += ['log.rotation', 'log.set_stream']
                declaration = original[original.index(b'const char *file ='):original.index(b'TEST(log, level)')]
                config = '#define M14_PRIVATE_LOG_FILE 1\n' + public('#define SW_STRL(', count=1) + declaration.decode()
            config += '#define M14_GTEST_FILTER ' + json.dumps('--gtest_filter=' + ':'.join(expected)) + '\n'
            if args.hash_originals:
                original = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':core-tests/src/core/hash.cpp'])
                (work / 'pinned-hash.cpp').write_bytes(original)
                excerpt = original[original.index(b'static const int hash_count'):]
                expected = ['hash.php', 'hash.jenkins']
                declarations = public('uint64_t swoole_random_int(', 'size_t swoole_random_bytes(', '#define SW_LOOP_N(', count=3)
                config = '#include "swoole_hash.h"\n' + declarations + '#define M14_GTEST_FILTER ' + json.dumps('--gtest_filter=' + ':'.join(expected)) + '\n'
                report['limits'] = 'Two unchanged hash originals PHP/Jenkins with real random input provider,8192originalsizes andassertions. Austin signedbinary undefined-shift original andCRC32 unselected/unqualified. No fullgroup/39original inventory, worldbuild, sanitizer, fault/crash/OOM/lifetime or performance claim.'
            # Each engine's config takes its declarations from its own swoole.h.
            headers = {'native': subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':include/swoole.h'], text=True),
                       'selected': (ROOT / 'native/corebase/swoole.h').read_text()}
            for name, text in headers.items():
                def declared(match):
                    prefixes, count = json.loads(match.group(1))
                    lines = [line for line in text.splitlines() if line.startswith(tuple(prefixes))]
                    if count is not None and len(lines) != count:
                        raise ValueError(f'{name} swoole.h declarations changed: {prefixes}')
                    return ''.join(line + '\n' for line in lines)
                (work / (name + '-config')).mkdir()
                (work / (name + '-config/logger-originals-config.h')).write_text(re.sub(r'@@(.*?)@@\n', declared, config))
            (work / 'pretty_originals.inc').write_bytes(excerpt)
            report['expected_original_cases'] = expected
        if args.pretty:
            original = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':src/core/log.cc'], text=True)
            excerpt = original[original.index('std::string Logger::get_pretty_name'):original.index('bool Logger::open')]
            (work / 'pretty_reference.inc').write_text(excerpt.replace('Logger::get_pretty_name', 'reference_pretty'))
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
        helper = work / 'm14_storage.so'
        google_raw = shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs', 'gtest'], text=True)) if originals else []
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
        if originals:
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
        def helper_command(engine, output):
            return ['c++', '-std=c++17' if originals else '-std=c++14', '-O2', '-pthread', '-Wall', '-Wextra', '-Werror', *includes,
                    '-DHAVE_CONFIG_H', *sdk_headers[engine], *openssl, *(['-I' + str(work / (engine + '-config'))] if originals else []),
                    '-I' + str(work), *(['-bundle', '-undefined', 'dynamic_lookup'] if sys.platform == 'darwin' else ['-shared', '-fPIC']),
                    '-MD', '-MF', str(work / (engine + '.d')), str(work / (fixture + '.cc')), *google, '-o', str(output)]
        command = helper_command('native', helper)
        result = run_isolated(command, timeout=60)
        report['compilation'] = dict(command=command, exit=result.returncode, stdout=result.stdout,
                                     stderr=result.stderr, php_sdk=version)
        if result.returncode:
            raise ValueError('Private Logger helper compilation failed')
        symbols = run_isolated(['nm', '-u', str(helper)], timeout=15)
        report['helper_imports'] = dict(exit=symbols.returncode, stdout=symbols.stdout, stderr=symbols.stderr)
        required = ['get_pretty_name'] if args.pretty or originals else ['Logger3put', 'reset', 'open', 'close'] if args.backtrace else ['gen_real_file', 'set_stream', 'reopen', 'get_file']
        if args.logger_originals:
            required += ['sw_logger', 'set_stream', 'open', 'close']
        if args.hash_originals:
            required = ['swoole_hash_php', 'swoole_hash_jenkins', 'swoole_random_int', 'swoole_random_bytes']
        if symbols.returncode or any(name not in symbols.stdout for name in required):
            raise ValueError('Helper lacks required imported production Logger methods')
        inline_helper = work / 'm14_inline_selected.so'
        selected_command = helper_command('selected', inline_helper)
        selected_command[-2:-2] = ['-DM14_SELECTED_INLINE']
        result = run_isolated(selected_command, timeout=60)
        report['selected_inline_compilation'] = dict(command=selected_command, exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
        if result.returncode:
            raise ValueError('Selected inline helper compilation failed')
        symbols = run_isolated(['nm', '-u', str(inline_helper)], timeout=15)
        report['selected_inline_imports'] = dict(exit=symbols.returncode, stdout=symbols.stdout, stderr=symbols.stderr)
        required = ['swr_v1_logger_set'] if args.backtrace else []
        if symbols.returncode or any(name not in symbols.stdout for name in required):
            raise ValueError('Selected inline helper lacks required Rust utility imports')
        exports = run_isolated(['nm', *(['-D'] if sys.platform.startswith('linux') else []), '-g', selected['extension']], timeout=30)
        report['selected_inline_exports'] = {name: re.findall(r'^.*\b[TW]\s+_?' + name + (r'\w+$' if name.endswith('_') else r'$'), exports.stdout, re.M) for name in required}
        if exports.returncode or not all(report['selected_inline_exports'].values()):
            raise ValueError('Actual selected DSO lacks required defined inline utility exports')
        for engine, identity in zip(engines, report['engines']):
            with tempfile.TemporaryDirectory(prefix='m14-storage-') as directory:
                engine_helper = inline_helper if identity['rust_feature_bits'] & 512 else helper
                result = run_isolated(engine.command('-d', 'extension=' + str(engine_helper), str(work / (fixture + '.php')), directory,
                                                     *([str(len(expected))] if originals else [])),
                                      timeout=30, env={**os.environ, 'TZ': 'UTC0', 'LC_ALL': 'C'})
                row = dict(engine=engine.name, exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                           extension_sha256_after=sha(identity['extension']), passed=False,
                           files={p.name: dict(content=p.read_text()) for p in sorted(Path(directory).iterdir()) if p.is_file()})
                report['results'].append(row)
                if result.returncode == 0 and not result.stderr and row['extension_sha256_after'] == identity['sha256']:
                    remaining = result.stdout
                    if originals:
                        cases = list(ET.fromstring(row['files']['gtest.xml']['content']).iter('testcase'))
                        row['original_cases'] = [dict(case.attrib) for case in cases]
                        if ({case.get('classname') + '.' + case.get('name') for case in cases} != set(expected)
                                or len(cases) != len(expected) or any(case.get('status') != 'run' or case.get('result') != 'completed' or list(case) for case in cases)):
                            raise ValueError('Original GoogleTest XML did not pass selected unchanged cases')
                        remaining = remaining.splitlines()[-1]
                    if args.backtrace:
                        row['traces'] = {}
                        for phase in ['DEFAULT', 'FILTER', 'ENABLED', 'RESET']:
                            match = re.search(r'M14_TRACE_' + phase + r'_BEGIN\n(.*?)M14_TRACE_' + phase + r'_END\n', remaining, re.S)
                            if match is None:
                                raise ValueError('Missing actual backtrace phase markers: ' + phase)
                            trace = match.group(1)
                            row['traces'][phase] = trace.splitlines()
                            if phase in ['DEFAULT', 'FILTER']:
                                if trace:
                                    raise ValueError('Unrequested or filtered backtrace emitted')
                            elif not trace or not re.search(r'swoole_print_backtrace|Logger3put|Logger::put', trace):
                                raise ValueError('Enabled backend has no actual Logger/print_backtrace frames')
                            remaining = remaining[:match.start()] + remaining[match.end():]
                    row['summary'] = json.loads(remaining)
                    row['passed'] = True
        if failed := [row for row in report['results'] if not row['passed']]:
            raise ValueError('Logger storage fixture failed; all engine outputs retained: ' + '; '.join(
                f"{row['engine']} exit {row['exit']}, stderr {row['stderr'][-400:]!r}" for row in failed))
        compare(report['results'])
        unchanged(args.build_manifest, report['source_attestation'])
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
