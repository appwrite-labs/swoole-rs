#!/usr/bin/env python3
"""Compare the pinned inline helpers with the hybrid build's headers, including signedness/locale."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import run_isolated, write_report
from tools.build_hybrid import hybrid_tree
from tools.build_upstream import COMMIT as PIN
SIGNATURES = {'include/swoole_util.h': ['static inline size_t ltrim(', 'static inline size_t rtrim(char *', 'static inline size_t rtrim(const char *', 'static inline ssize_t substr_len(', 'static inline bool starts_with(const char *', 'static inline bool starts_with(const std::string &', 'static inline bool ends_with(const char *'],
              'include/swoole.h': ['static inline unsigned int swoole_streq(', 'static inline unsigned int swoole_strcaseeq(', 'static inline unsigned int swoole_str_starts_with(', 'static inline unsigned int swoole_str_istarts_with(', 'static inline const char *swoole_strnstr(', 'static inline const char *swoole_strncasestr(', 'static inline ssize_t swoole_strnpos(', 'static inline ssize_t swoole_strrnpos(', 'static inline void swoole_strtolower(']}

def body(source, signature):
    start = source.index(signature)
    end = source.index('{', start) + 1
    depth = 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--rust-asan', action='store_true', help='Declare separately instrumented Rust archive; verify ASan imports')
    parser.add_argument('--native-ubsan-only', action='store_true', help='Use compatible native UBSan with separately supplied Rust ASan runtime')
    parser.add_argument('--asan-runtime', type=Path)
    args = parser.parse_args()
    if args.native_ubsan_only and not (args.rust_asan and args.asan_runtime):
        parser.error('--native-ubsan-only requires Rust ASan and its runtime')
    report = {'pin': PIN,
              'scope': 'Exact pinned inline trim/search/equality/prefix/suffix/partition bodies against the hybrid build\'s swoole.h/swoole_util.h, host libc locale and char mode. Available and unavailable locale names are explicit; unavailable locales are unqualified. Invalid reverse bounds/empty needles and undefined signed-char ctype arguments excluded. Native ASan/UBSan only; Rust uninstrumented. No PHP, full-header build, performance or activation claim.',
              'results': [], 'passed': False}
    report['rust_asan_declared'] = args.rust_asan
    if args.rust_asan:
        from rust_asan import inspect
        report['rust_asan_imports'] = inspect(args.archive, ['swr_v2_utils_ltrim', 'swr_v2_utils_rtrim', 'swr_v2_utils_search'])
        report['scope'] = report['scope'].replace('Rust uninstrumented.', 'Rust ASan declared and imports verified; precompiled std uninstrumented.')
    with tempfile.TemporaryDirectory(prefix='swr-inline-utils-') as temporary:
        tree = Path(temporary)
        # Upstream's side slices the pin; the candidate is the hybrid build's own headers.
        functions = []
        for name, signatures in SIGNATURES.items():
            data = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':' + name], text=True)
            functions.extend(body(data, signature) for signature in signatures)
        (tree / 'original.inc').write_text('\n'.join(functions))
        hybrid, hybrid_flags = hybrid_tree(args.archive)
        report['hybrid_tree'] = str(hybrid)
        for mode in ['signed', 'unsigned']:
            binary = tree / mode
            command = ['c++', '-std=c++17', '-O2', '-f' + mode + '-char', '-Wall', '-Wextra', '-Werror',
                       '-fsanitize=' + ('undefined' if args.native_ubsan_only else 'address,undefined'), '-fno-sanitize-recover=all', '-I' + str(tree),
                       *hybrid_flags, '-DHAVE_CONFIG_H', '-I' + str(hybrid), str(Path(__file__).with_suffix('.cc')),
                       str(args.archive.resolve()), '-lm', '-o', str(binary)]
            if args.asan_runtime:
                runtime = args.asan_runtime.resolve(strict=True)
                command += [str(runtime), '-Wl,-rpath,' + str(runtime.parent)]
                report['asan_runtime'] = str(runtime)
            if sys.platform.startswith('linux'): command += ['-ldl', '-lpthread', '-lrt', '-lutil']
            result = run_isolated(command, timeout=60)
            row = {'char': mode, 'compile': command, 'compile_exit': result.returncode, 'compile_stderr': result.stderr}
            report['results'].append(row)
            if result.returncode: break
            result = run_isolated([str(binary)], timeout=30)
            row.update(exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
            if result.returncode: break
        report['passed'] = len(report['results']) == 2 and all(row.get('exit') == 0 for row in report['results'])
    if args.native_ubsan_only:
        report['scope'] = report['scope'].replace('Native ASan/UBSan', 'Native UBSan')
    write_report(args.output, report)
    print(json.dumps(report))
    return int(not report['passed'])

if __name__ == '__main__':
    sys.exit(main())
