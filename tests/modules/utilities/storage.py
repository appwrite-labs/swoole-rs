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
import shutil
import subprocess
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import parse_engines, run_isolated, write_report
from tools.hybrid_identity import attest
from bytes_gate import SIGNATURES, body
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
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--source-ref', required=True)
    parser.add_argument('--output', type=Path, required=True)
    phase = parser.add_mutually_exclusive_group()
    phase.add_argument('--backtrace', action='store_true', help='Run normal enabled/filter/reset backtrace behavior instead of storage/concurrent reopen')
    phase.add_argument('--pretty', action='store_true', help='Compare actual public pretty-name method against the unchanged pinned body')
    phase.add_argument('--pretty-originals', action='store_true', help='Execute two unchanged pretty-name original GoogleTests through the actual public method')
    phase.add_argument('--logger-originals', action='store_true', help='Execute unchanged rotation, memory-stream and pretty-name original GoogleTests')
    phase.add_argument('--hash-originals', action='store_true', help='Execute unchanged defined PHP/Jenkins hash original GoogleTests')
    phase.add_argument('--inline-originals', action='store_true', help='Execute seven unchanged inline-utility originals with engine-specific native/selected header bodies')
    phase.add_argument('--global-logger-originals', action='store_true', help='Execute four unchanged global Logger originals with coherent per-engine SDKs')
    phase.add_argument('--redirect-original', action='store_true', help='Execute original redirect_1 and verify child assertion observability')
    phase.add_argument('--redirect-originals', action='store_true', help='Execute both redirect originals with private path inputs and child-failure observer')
    args = parser.parse_args()
    redirect = args.redirect_original or args.redirect_originals
    coherent = args.global_logger_originals or redirect
    originals = args.pretty_originals or args.logger_originals or args.hash_originals or args.inline_originals or args.global_logger_originals or redirect
    fixture = 'pretty_originals' if originals else 'pretty' if args.pretty else 'backtrace' if args.backtrace else 'storage'
    work = args.output.resolve().parent / ('logger-storage-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    report = dict(passed=False, created_utc=datetime.now(timezone.utc).isoformat(), work=str(work),
                  scope=__doc__, fixture=fixture, engines=[], results=[])
    try:
        # The selected side's sources, headers and config.h are the hybrid build's.
        hybrid, hybrid_flags = hybrid_tree(args.archive)
        report['hybrid_tree'] = str(hybrid)
        for name in [fixture + '.cc', fixture + '.php', 'storage_summary.py'] + (['logger_layout.cc'] if fixture == 'storage' else []) + ['coherent_sdk.py'] + (['child_assertions.h', 'terminal_stdout.py'] if redirect else []):
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
            if args.inline_originals:
                original = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':core-tests/src/core/string.cpp'])
                (work / 'pinned-string.cpp').write_bytes(original)
                names = ['ltrim', 'rtrim', 'strnpos', 'strnstr', 'substr_len', 'starts_with', 'ends_with']
                fragments = [body(original.decode(), 'TEST(string, ' + name + ')') for name in names]
                excerpt = ('using namespace std;\n' + '\n'.join(fragments)).encode()
                expected = ['string.' + name for name in names]
                # Native bodies from the pin, selected bodies from the hybrid build's headers.
                for name, signatures in SIGNATURES.items():
                    pinned = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':' + name]).decode()
                    for variant, content in [('native', pinned), ('selected', (hybrid / name).read_text())]:
                        (work / (variant + '-' + Path(name).name + '.inc')).write_text('\n'.join(body(content, signature) for signature in signatures))
                config = '#include <cassert>\n#include <cctype>\n#include <sys/types.h>\n#include <strings.h>\n' + public('#define SW_STRL(', '#define sw_likely(')
                for index, variant in enumerate(['selected', 'native']):
                    config += ('#ifdef M14_SELECTED_INLINE\n#include "swoole_rs_utilities.h"\n' if index == 0 else '#else\n')
                    config += 'namespace swoole {\n#include "' + variant + '-swoole_util.h.inc"\n}\n#include "' + variant + '-swoole.h.inc"\n'
                config += '#endif\n#define M14_GTEST_FILTER ' + json.dumps('--gtest_filter=' + ':'.join(expected)) + '\n'
                report['limits'] = 'Seven unchanged original TEST/ASSERT bodies, exact engine-specific native/selected M14inline wrappers and loaded selected Rust exports. No M01String object/copy or fullheader/worldbuild, full39inventory, sanitizer/lifetime/performance claim.'
            if args.global_logger_originals:
                names = ['level', 'date_format_long_string', 'date_with_microseconds', 'ignore_error']
                excerpt = ('\n'.join(body(original.decode(), 'TEST(log, ' + name + ')') for name in names)).encode()
                expected = ['log.' + name for name in names]
                config = '#include "swoole_file.h"\n#include "swoole_process_pool.h"\n#include <regex>\n#define M14_PRIVATE_LOG_FILE 1\n'
                config += original[original.index(b'const char *file ='):original.index(b'TEST(log, level)')].decode()
                config += '#define M14_GTEST_FILTER ' + json.dumps('--gtest_filter=' + ':'.join(expected)) + '\n'
                report['limits'] = 'Four unchanged global Logger originals through real GoogleTest and coherent frozen native/selected SDKs. Ordinary String construction/read consumers only. Unsafe date_format scanf, hardcoded open_fail, spawn adapters and full39/performance unqualified.'
            if redirect:
                names = ['redirect_1', 'redirect_2'] if args.redirect_originals else ['redirect_1']
                excerpt = '\n'.join(body(original.decode(), 'TEST(log, ' + name + ')') for name in names).encode()
                expected = ['log.' + name for name in names]
                main_source = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':core-tests/src/main.cpp'], text=True)
                (work / 'pinned-main.cpp').write_text(main_source)
                helpers = '\n'.join(body(main_source, signature) for signature in ['pid_t spawn_exec(', 'int spawn_exec_and_wait('])
                config = '#include "swoole_file.h"\n#include "swoole_process_pool.h"\n#include <system_error>\n#define M14_PRIVATE_LOG_FILE 1\n#define M14_CHILD_ASSERTIONS 1\n#include "child_assertions.h"\n'
                config += original[original.index(b'const char *file ='):original.index(b'TEST(log, level)')].decode()
                if args.redirect_originals:
                    original_inputs = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':core-tests/include/test_core.h'], text=True)
                    (work / 'pinned-test_core.h').write_text(original_inputs)
                    report['test_path_input'] = dict(original_macro=next(line for line in original_inputs.splitlines() if line.startswith('#define TEST_LOG_FILE ')), private_binding='(::file)')
                    config += '#define TEST_LOG_FILE (::file)\n'
                config += 'namespace swoole { namespace test {\n' + helpers + '\n}}\n'
                config += 'TEST(m14_observer, child_failure) { ASSERT_EQ(test::spawn_exec_and_wait([]() { ADD_FAILURE() << "observer negative control"; }), 0); }\n'
                config += '#define M14_GTEST_FILTER ' + json.dumps('--gtest_filter=' + ':'.join(expected)) + '\n'
            # Each engine's config takes its declarations from its own swoole.h.
            headers = {'native': subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':include/swoole.h'], text=True),
                       'selected': (hybrid / 'include/swoole.h').read_text()}
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
        report['source_attestation'] = attest(args.build_manifest, args.archive, selected, args.source_ref, 512)
        manifest = json.loads(args.build_manifest.read_text())
        # Every helper gets its own engine's headers: the pinned SDK, or the hybrid build's.
        logger_layout = any('include/swoole_log.h' in row.get('upstream_paths', []) for row in manifest['inputs']['patches'])
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
        report['coherent_sdk'] = {'native': stage(work / 'native', Path(native_identity['extension']).parent, args.source, args.source_ref, ROOT),
                                  'selected': dict(hybrid_tree=str(hybrid), flags=hybrid_flags,
                                                   string_header_sha256=sha(hybrid / 'include/swoole_string.h'))}
        (work / 'selected').mkdir()
        shutil.copyfile(hybrid / 'config.h', work / 'selected/config.h')
        sdk_headers = {'native': ['-I' + str(work / 'native/include')], 'selected': ['-I' + str(work / 'selected'), *hybrid_flags]}
        openssl = shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True))
        if coherent and report['coherent_sdk']['native']['string_header_sha256'] == report['coherent_sdk']['selected']['string_header_sha256']:
            raise ValueError('Selected SDK lacks actual String overlay')
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
        if coherent:
            required = ['sw_logger', 'file_get_contents', 'swoole_get_worker_type', 'swoole_set_worker_type']
        if redirect:
            required = ['sw_logger', 'file_get_contents', 'swoole_waitpid', 'redirect_stdout_and_stderr', 'fork']
        if args.redirect_originals:
            required += ['read_content']
        if args.hash_originals:
            required = ['swoole_hash_php', 'swoole_hash_jenkins', 'swoole_random_int', 'swoole_random_bytes']
        if args.inline_originals:
            required = []
            if re.search(r'swr_v[0-9]+_utils_', symbols.stdout):
                raise ValueError('Native inline helper unexpectedly imports Rust utilities')
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
        required = (['swr_v1_string_'] if coherent else ['swr_v2_utils_ltrim', 'swr_v2_utils_rtrim', 'swr_v2_utils_search', 'swr_v1_utils_compare', 'swr_v1_utils_partition'] if args.inline_originals else [])
        if logger_layout and args.backtrace:
            required += ['swr_v1_utils_log_state_scalar_set']
        if symbols.returncode or any(name not in symbols.stdout for name in required):
            raise ValueError('Selected inline helper lacks required Rust utility imports')
        exports = run_isolated(['nm', *(['-D'] if sys.platform.startswith('linux') else []), '-g', selected['extension']], timeout=30)
        report['selected_inline_exports'] = {name: re.findall(r'^.*\b[TW]\s+_?' + name + (r'\w+$' if name.endswith('_') else r'$'), exports.stdout, re.M) for name in required}
        if exports.returncode or not all(report['selected_inline_exports'].values()):
            raise ValueError('Actual selected DSO lacks required defined inline utility exports')
        layouts = None
        if fixture == 'storage':
            layouts = {}
            layout_source = Path(__file__).with_name('logger_layout.cc')
            report['layout_probes'] = {}
            for identity in report['engines']:
                name = identity['name']
                binary = work / ('layout-' + name)
                layout_command = ['c++', '-std=c++14', '-O2', '-Wall', '-Wextra', '-Werror',
                                  *sdk_headers['selected' if identity['rust_feature_bits'] & 512 else 'native'], str(layout_source), '-o', str(binary)]
                compiled = run_isolated(layout_command, timeout=60)
                probe = dict(command=layout_command, compile_exit=compiled.returncode,
                             compile_stdout=compiled.stdout, compile_stderr=compiled.stderr)
                report['layout_probes'][name] = probe
                if compiled.returncode:
                    raise ValueError('Logger SDK layout probe compilation failed')
                observed = run_isolated([str(binary)], timeout=15)
                probe.update(exit=observed.returncode, stdout=observed.stdout, stderr=observed.stderr)
                if observed.returncode or observed.stderr:
                    raise ValueError('Logger SDK layout probe failed')
                layouts[name] = json.loads(observed.stdout)
            report['expected_engine_layouts'] = layouts
        launch = [sys.executable, str(work / 'terminal_stdout.py')] if redirect else []
        report['stdout_environment'] = 'terminal with raw output processing; libc line buffering inherited by fork' if redirect else 'pipe'
        for engine, identity in zip(engines, report['engines']):
            with tempfile.TemporaryDirectory(prefix='m14-storage-') as directory:
                engine_helper = inline_helper if identity['rust_feature_bits'] & 512 else helper
                result = run_isolated(launch + engine.command('-d', 'extension=' + str(engine_helper), str(work / (fixture + '.php')), directory,
                                                     *([str(len(expected))] if originals else [])),
                                      timeout=30, env={**os.environ, 'TZ': 'UTC0', 'LC_ALL': 'C'})
                row = dict(engine=engine.name, exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                           extension_sha256_after=sha(identity['extension']), passed=False,
                           files={p.name: dict(content=p.read_text()) for p in sorted(Path(directory).iterdir()) if p.is_file()})
                report['results'].append(row)
                if result.returncode == 0 and not result.stderr and row['extension_sha256_after'] == identity['sha256']:
                    remaining = result.stdout
                    if redirect and row['files'].get('child-assertions.txt', {}).get('content') != 'child-exit\n' * len(expected):
                        raise ValueError('Child assertion observer missing completion or recorded failure')
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
                if redirect:
                    with tempfile.TemporaryDirectory(prefix='m14-observer-negative-') as control:
                        result = run_isolated(launch + engine.command('-d', 'extension=' + str(engine_helper), str(work / (fixture + '.php')), control, '1'), timeout=30, env={**os.environ, 'TZ': 'UTC0', 'LC_ALL': 'C', 'M14_CHECK_OBSERVER': '1'})
                        files = {p.name: p.read_text() for p in Path(control).iterdir() if p.is_file()}
                        control_cases = list(ET.fromstring(files['gtest.xml']).iter('testcase'))
                        journal = files.get('child-assertions.txt')
                        rejected = (result.returncode == 0 and not result.stderr and journal == 'failure\nchild-exit\n'
                                    and len(control_cases) == 1 and control_cases[0].get('name') == 'child_failure' and not list(control_cases[0]))
                        row['observer_negative_control'] = dict(exit=result.returncode, stdout=result.stdout, stderr=result.stderr, files=files,
                                                                child_failure_hidden_by_original_exit=True, rejected_by_journal=rejected)
                        if not rejected:
                            raise ValueError('Child assertion negative control did not expose original zero-exit blindness')
        if not all(row['passed'] for row in report['results']):
            raise ValueError('Logger storage fixture failed; all engine outputs retained')
        compare(report['results'], layouts)
        receipt = report['source_attestation']
        if sha(args.build_manifest) != receipt['manifest_sha256'] or sha(args.archive) != receipt['archive_sha256']:
            raise ValueError('Selected production inputs changed')
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
