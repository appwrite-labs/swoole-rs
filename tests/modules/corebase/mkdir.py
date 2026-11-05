#!/usr/bin/env python3
"""Compare real mkdir callers (pinned and hybrid-build base.cc) with scoped IO traces and
private filesystem cases."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import tempfile
from patching import check_git_environment, hybrid_corebase, prove_calls
from cases import mkdir_cases

ROOT = Path(__file__).resolve().parents[3]
import sys
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT as PIN
LOCAL = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--include', type=Path, action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    report = dict(passed=False, upstream_commit=PIN, created_utc=datetime.now(timezone.utc).isoformat(),
                  commands=[], native_sanitizers=args.sanitize,
                  rust_sanitizers='External archive; inspect exact artifact provenance',
                  scope='Whole original/patched native TU. Deterministic IO trace and actual private filesystem; logging/last_error are explicit modeled shims, not native logger/global validation.',
                  exclusions=['Original empty-input UB and full C-visible PATH_MAX-1 nontrailing append overflow'])
    args.output.parent.mkdir(parents=True, exist_ok=True)

    def run(argv, cwd=None, data=None):
        env = os.environ.copy()
        if args.sanitize:
            env.update(ASAN_OPTIONS='abort_on_error=1:detect_leaks=' + ('0' if platform.system() == 'Darwin' else '1'),
                       UBSAN_OPTIONS='halt_on_error=1:print_stacktrace=1')
        process = subprocess.Popen(list(map(str, argv)), cwd=cwd, env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        try:
            stdout, stderr = process.communicate(data, timeout=60)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = process.communicate()
            report['commands'].append(dict(argv=list(map(str, argv)), interrupted=True,
                                            stdout=stdout.decode(errors='replace'), stderr=stderr.decode(errors='replace')))
            raise
        report['commands'].append(dict(argv=list(map(str, argv)), returncode=process.returncode,
                                       stderr=stderr.decode(errors='replace')))
        if process.returncode:
            raise RuntimeError('Command failed; original diagnostics retained in report')
        return stdout

    def scoped(body):
        start = body.index('bool swoole_mkdir_recursive(')
        opening = body.index('{', start)
        depth = 1;end = opening + 1
        while depth:
            depth += (body[end] == '{') - (body[end] == '}');end += 1
        names = ['access', 'mkdir', 'swoole_sys_warning', 'swoole_error_log']
        before = ''.join('#pragma push_macro("' + n + '")\n' for n in names) + '#include "mkdir-shim.h"\n'
        after = '\n' + ''.join('#pragma pop_macro("' + n + '")\n' for n in reversed(names))
        return body[:start] + before + body[start:end] + after + body[end:] + '\n#include "mkdir-driver.h"\n'

    try:
        check_git_environment()
        source = args.source.resolve();archive = args.archive.resolve()
        original = run(['git', '-C', source, 'show', PIN + ':src/core/base.cc']).decode()
        patched, hybrid_flags, report['hybrid'] = hybrid_corebase(archive)
        patched = patched.decode()
        with tempfile.TemporaryDirectory(prefix='swoole-mkdir-') as temporary:
            work = Path(temporary)
            common = ['-std=c++17', '-O2', '-ffunction-sections', '-fdata-sections', '-I' + str(LOCAL)]
            common += ['-I' + str(p.resolve()) for p in args.include]
            if args.sanitize:
                common += ['-g', '-fsanitize=address,undefined', '-fno-omit-frame-pointer']
            pinned_flags = ['-DHAVE_CONFIG_H', '-I' + str(source), '-I' + str(source / 'include')]
            link = ['-Wl,-dead_strip'] if platform.system() == 'Darwin' else ['-Wl,--gc-sections', '-ldl', '-lpthread', '-lm']
            outputs = {}
            for engine, body in [('original', original), ('rust', patched)]:
                cpp = work / (engine + '.cc');cpp.write_text(scoped(body))
                binary = work / engine
                # Each engine builds from its own tree: the pin, or the hybrid build.
                flags = [*common, *(hybrid_flags if engine == 'rust' else pinned_flags)]
                run(['c++', *flags, cpp, *([archive] if engine == 'rust' else []), *link, '-o', binary])
                report.setdefault('actual_calls', {})[engine] = prove_calls(binary, ['mkdir_complete'],
                    engine == 'rust', run, args.output.with_suffix('.' + engine))
                outputs[engine] = run([binary], data=mkdir_cases())
                args.output.with_suffix('.' + engine + '.jsonl').write_bytes(outputs[engine])
                filesystem = work / (engine + '-filesystem');filesystem.mkdir()
                assert run([binary, '--real', filesystem]) == b'real filesystem PASS\n'
            assert outputs['original'] == outputs['rust'], 'Actual mkdir IO-order/bytes/result/errno mismatch'
            rows = [json.loads(row) for row in outputs['original'].splitlines()]
            assert len(rows) == 270
            report.update(passed=True, matched=len(rows), path_max=rows[0]['path_max'], real_filesystem_engines=2)
    finally:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({k: report[k] for k in ['passed', 'matched', 'path_max', 'real_filesystem_engines']}))


if __name__ == '__main__':
    main()
