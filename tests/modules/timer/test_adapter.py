"""Drive the hybrid extension's own timer from a native probe; unchanged baseline probe goldens.

The probe compiles against the immutable --manifest build's headers and takes the
Timer, the swoole_timer_* wrappers and the Rust queue from the loaded extension,
so it tests the code that ships wherever that code lives. --sanitize instruments
the probe only; the extension, Rust and PHP are not. Each scenario is an isolated
five-second process group. Original failed upstream baseline reports remain failures.
"""
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys

import pytest

from tools.build_upstream import COMMIT as PIN
from tools.harness import run_isolated

SCENARIOS = ['equal-deadlines', 'self-remove', 'remove-other', 'destructor-remove-other',
             'update-active-repeat', 'destructor-add', 'fork-teardown',
             'delay-active-once', 'nested-select-once']

EXCEPTION_SCENARIOS = ['callback-throws', 'destructor-throws', 'callback-destructor-throw']

EXPECTED = {
    'equal-deadlines': ('AaCcBb', 'AaCcBb', 0, 3, False),
    'self-remove': ('AYNVa', 'AYNVa', 0, 1, False),
    'remove-other': ('AbaCc', 'AbaCc', 0, 3, False),
    'destructor-remove-other': ('AabCc', 'AabCc', 0, 3, False),
    'update-active-repeat': ('ABb', 'ABba', 1, 2, True),
    'destructor-add': ('AaBb', 'AaBbCc', 1, 3, False),
    'delay-active-once': ('AaBb', 'AaBb', 0, 2, False),
    'nested-select-once': ('Aa', 'Aa', 0, 1, False),
}

PROBE = '$ffi=FFI::cdef("int swr_timer_probe(const char *name);", $argv[1]); exit($ffi->swr_timer_probe($argv[2]));'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(name, observations):
    finals = [value for value in observations if value.get('scenario') == name and 'phase' not in value]
    assert len(finals) == 1, f'{name}: one final observation required: {observations}'
    value = finals[0]
    if name == 'wrapper-clear':
        assert value == {'scenario': name, 'inactive': True, 'missing': True, 'active': True,
                         'cancelled_other': True, 'active_view': True, 'remaining': 0,
                         'events': 'cAba', 'destructors': 3}
        return
    if name in EXCEPTION_SCENARIOS:
        assert value == {'scenario': name, 'failure': 'destructor' if name == 'destructor-throws' else 'callback',
                         'survivor_intact': True, 'remaining': 0, 'events': 'AaBb', 'destructors': 2}
        return
    if name == 'fork-active-teardown':
        assert value == {'scenario': name, 'child_status': 0, 'parent_intact': True, 'parent_drained': True, 'parent_destructors': 2}
        return
    if name == 'fork-teardown':
        assert value == {'scenario': name, 'child_status': 0, 'parent_intact': True, 'parent_destructors': 1}
        return
    second, events, count, destructors, a_present = EXPECTED[name]
    assert value == {'scenario': name, 'after_first': '', 'after_second': second, 'events': events,
                     'count_after_second': count, 'destructors': destructors,
                     'a_present': a_present, 'b_present': False}
    phases = [value for value in observations if 'phase' in value]
    assert phases == ([{'phase': 'callback-updated', 'scenario': name}] if name in ['delay-active-once', 'update-active-repeat'] else [])


@pytest.fixture(scope='module')
def probe(pytestconfig, tmp_path_factory):
    """Verify the immutable build, then compile the probe library against its sources."""
    if not pytestconfig.getoption('manifest'):
        pytest.skip('needs --manifest')
    assert sys.platform == 'darwin' or sys.platform.startswith('linux'), 'Native adapter probe supports macOS and Linux only'
    php = pytestconfig.getoption('php')
    linux = sys.platform.startswith('linux')
    php_extensions = list(pytestconfig.getoption('php_extension'))
    builtin_ffi = subprocess.check_output([php, '-n', '-r', 'echo extension_loaded("FFI") ? "yes" : "no";'], text=True, timeout=10).strip() == 'yes'
    if linux and not builtin_ffi and not any(Path(value).stem.lower() == 'ffi' for value in php_extensions):
        php_extensions.insert(0, 'ffi')
    manifest = json.loads(Path(pytestconfig.getoption('manifest')).resolve(strict=True).read_text())
    assert manifest['build_mode'] == 'immutable', 'immutable build required'
    assert manifest['inputs']['upstream_commit'] == PIN and 'timer' in manifest['inputs']['modules'], 'wrong selected source'
    extension = Path(manifest['extension']).resolve(strict=True)
    source = extension.parents[1]
    prefix = extension.parents[2] / 'rust-prefix'
    archive = prefix / 'lib/libswoole_rs_ffi.a'
    assert sha(extension) == manifest['extension_sha256'], 'extension checksum mismatch'
    assert sha(archive) == manifest['inputs']['rust_archive_sha256'], 'archive checksum mismatch'
    actual_pin = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip()
    diff = subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=source, text=True).strip()
    assert actual_pin == PIN and hashlib.sha256(diff.encode()).hexdigest() == manifest['source_diff_sha256'], 'source diff/pin mismatch'
    for name, checksum in manifest['inputs']['headers_sha256'].items():
        assert sha(prefix / 'include' / Path(name).name) == checksum, f'ABI header mismatch: {name}'
    tree = tmp_path_factory.mktemp('timer-adapter')
    for name in subprocess.check_output(['git', 'ls-files', 'include'], cwd=source, text=True).splitlines():
        if (source / name).is_file():
            (tree / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / name, tree / name)
    library = tree / ('probe.so' if linux else 'probe.dylib')
    compiler = shlex.split(os.environ.get('CXX', 'c++'))
    linker_flags = ['-shared', '-fPIC', '-Wl,-Bsymbolic'] if linux else ['-dynamiclib', '-undefined', 'dynamic_lookup']
    selected_defines = ['-DSWOOLE_RS_' + module.upper() + '=1' for module in manifest['inputs']['modules']]
    command = compiler + ['-std=c++17', '-O1', '-g', *linker_flags, *selected_defines,
                          '-I' + str(tree / 'include'), '-I' + str(prefix / 'include')]
    command += shlex.split(manifest['inputs']['effective_cppflags'])
    for option in manifest['configure']:
        if option.startswith('--with-openssl-dir='):
            command += ['-I' + option.split('=', 1)[1] + '/include']
    command += [str(Path(__file__).with_name('probe.cc')), '-lc', '-lm']
    if linux:
        command += ['-ldl', '-lpthread', '-lrt', '-lutil']
    command += ['-o', str(library)]
    environment = dict(os.environ)
    if pytestconfig.getoption('sanitize'):
        command += ['-fsanitize=address,undefined', '-fno-omit-frame-pointer']
        runtime_name = 'libasan.so' if linux else 'libclang_rt.asan_osx_dynamic.dylib'
        runtime = Path(subprocess.check_output(compiler + ['--print-file-name=' + runtime_name], text=True).strip()).resolve()
        assert runtime.is_file(), f'ASan runtime absent: {runtime}'
        environment.update({'LD_PRELOAD' if linux else 'DYLD_INSERT_LIBRARIES': str(runtime),
            'ASAN_OPTIONS': 'detect_leaks=0:halt_on_error=1:abort_on_error=0:exitcode=86:log_path=' + str(tree / 'asan-report'),
            'UBSAN_OPTIONS': 'print_stacktrace=1:halt_on_error=1:exitcode=87'})
    compiled = run_isolated(command, timeout=45)
    assert compiled.returncode == 0, compiled.stderr
    php_extra = [value for module in php_extensions for value in ['-d', 'extension=' + module]]
    return [php, '-n', *php_extra, '-d', 'extension=' + str(extension), '-d', 'ffi.enable=1', '-r', PROBE, str(library)], environment, tree


@pytest.mark.parametrize('name', [*SCENARIOS, 'fork-active-teardown', *EXCEPTION_SCENARIOS, 'wrapper-clear'])
def test_scenario(probe, name):
    command, environment, tree = probe
    before = set(tree.glob('asan-report*'))
    result = run_isolated([*command, name], timeout=5, env=environment)
    logs = {path.name: path.read_text(errors='replace') for path in set(tree.glob('asan-report*')) - before}
    assert not logs, logs
    assert result.returncode == 0, f'process exit {result.returncode}: {result.stdout}{result.stderr}'
    observations = []
    for line in result.stdout.splitlines():
        try:
            observations.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    verify(name, observations)
