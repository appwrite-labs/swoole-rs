"""Real timer queue C ABI gate; injected clock, no PHP/reactor integration claim.

ASan/UBSan (--sanitize) instruments the C/C++ caller only, unless --archive was
compiled with Rust ASan (--rust-asan). Not exercised: ID, round and execution
counter exhaustion, allocation faults, PHP callbacks, Zend bailout, reactor
wakeups and multithreaded fork allocator safety.
"""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

from tools.harness import ROOT, run_isolated

LAYOUT = '''#include "swoole_rs_timer.h"
_Static_assert(sizeof(void*) == 8, "64-bit supported hosts");
_Static_assert(sizeof(swr_v1_timer_view) == 56, "view size");
_Static_assert(offsetof(swr_v1_timer_view, token) == 48, "token offset");
_Static_assert(offsetof(swr_v1_timer_view, removed) == 40, "removed offset");
_Static_assert(sizeof(swr_v1_timer_stats) == 32, "stats size");
_Static_assert(_Generic(((swr_v1_timer_view*)0)->id, int64_t: 1, default: 0), "signed ID");
int timer_layout(void) { return SWR_TIMER_OK; }
'''


def test_kernel(pytestconfig, tmp_path):
    sanitize, archive = pytestconfig.getoption('sanitize'), pytestconfig.getoption('archive')
    rust_asan = pytestconfig.getoption('rust_asan')
    assert not rust_asan or (sanitize and archive), '--rust-asan requires --sanitize and an instrumented --archive'
    env = dict(os.environ)
    if sanitize:
        env['ASAN_OPTIONS'] = f'detect_leaks={0 if sys.platform == "darwin" else 1}:abort_on_error=1'
        env['UBSAN_OPTIONS'] = 'halt_on_error=1:print_stacktrace=1'

    def run(command, timeout=60):
        result = run_isolated(command, timeout=timeout, cwd=ROOT, env=env)
        assert result.returncode == 0, f'{command} failed ({result.returncode}): {result.stderr}'
        return result

    if archive is None:
        run(['cargo', 'build', '-p', 'swoole-rs-timer', '--release', '--locked'], timeout=180)
        archive = (ROOT / os.environ.get('CARGO_TARGET_DIR', 'target') / os.environ.get('CARGO_BUILD_TARGET', '')
                   / 'release/libswoole_rs_timer.a')
    archive = Path(archive).resolve(strict=True)
    if rust_asan:
        assert '__asan_report_' in subprocess.run(['nm', '-u', str(archive)], capture_output=True, text=True).stdout, \
            'Selected archive lacks Rust ASan report imports'
    # Compile the frozen header as C11 independently of the C++ executable.
    (tmp_path / 'layout.c').write_text(LAYOUT)
    run(shlex.split(os.environ.get('CC', 'cc')) + ['-std=c11', '-Wall', '-Wextra', '-Werror', '-I', str(ROOT / 'native/include'),
                                                   '-c', str(tmp_path / 'layout.c'), '-o', str(tmp_path / 'layout.o')])
    command = shlex.split(os.environ.get('CXX', 'c++')) + ['-std=c++17', '-O2', '-g', '-Wall', '-Wextra', '-Werror',
        '-I', str(ROOT / 'native/include'), str(Path(__file__).with_name('kernel.cc')), str(archive)]
    if sanitize:
        command += ['-fsanitize=address,undefined', '-fno-omit-frame-pointer']
    command += ['-lc', '-lm']
    if sys.platform.startswith('linux'):
        command += ['-ldl', '-lpthread', '-lrt', '-lutil']
    run(command + ['-o', str(tmp_path / 'kernel')])
    observations = json.loads(run([str(tmp_path / 'kernel')], timeout=15).stdout)
    assert observations['topics_passed'] == 13, observations
