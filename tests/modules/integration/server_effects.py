#!/usr/bin/env python3
"""Qualify production Server effect custody; injected SDK outcomes are distinct from optional real PHP cases."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import run_isolated, write_report


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def command(report, argv, *, cwd=ROOT, timeout=120):
    result = run_isolated(list(map(str, argv)), cwd=cwd, timeout=timeout)
    report.setdefault('commands', []).append(dict(argv=list(map(str, argv)), cwd=str(cwd),
        exit=result.returncode, stdout=result.stdout, stderr=result.stderr))
    if result.returncode:
        raise ValueError('Command failed: ' + shlex.join(list(map(str, argv))))
    return result


def sdk_flags(build):
    flags = {}
    for line in (build / 'CMakeFiles/core-tests.dir/flags.make').read_text().splitlines():
        if ' = ' in line:
            key, value = line.split(' = ', 1)
            flags[key] = shlex.split(value)
    link = shlex.split((build / 'CMakeFiles/core-tests.dir/link.txt').read_text())
    libraries, skip = [], False
    for value in link[1:]:
        if skip:
            skip = False
        elif value == '-o':
            skip = True
        elif not value.endswith('.o') and not value.startswith('-std='):
            libraries.append(value)
    compile_flags = [*flags.get('CXX_DEFINES', []), *flags.get('CXX_INCLUDES', []),
                     *[value for value in flags.get('CXX_FLAGS', []) if not value.startswith('-std=')]]
    return link[0], compile_flags, libraries


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--archive', type=Path, required=True)
    p.add_argument('--core', type=Path, required=True)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--config', type=Path, required=True)
    p.add_argument('--sdk-report', type=Path, help='Reuse completed worker-retirement SDKs with matching library hashes')
    p.add_argument('--jobs', type=int, default=2)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--php', type=Path, help='Optional actual ZTS pair, with --php-config and both extensions')
    p.add_argument('--php-config', type=Path)
    p.add_argument('--native-extension', type=Path)
    p.add_argument('--selected-extension', type=Path)
    p.add_argument('--selected-source', type=Path)
    a = p.parse_args()
    output = a.output.resolve()
    work = output.with_suffix('')
    work.mkdir(parents=True, exist_ok=True)
    archive = a.archive.resolve()
    prefix = archive.parents[1]
    fixture = Path(__file__).resolve().with_suffix('.cc')
    register = fixture.with_name('server_effects_register.cc')
    typed = fixture.with_name('server_effects_prefix.cc')
    report = dict(passed=False, scope=__doc__, archive=str(archive), archive_sha256=sha(archive),
                  sources={path.name: sha(path) for path in [fixture, register, typed]},
                  injected_provider_qualified=False, actual_php_qualified=False,
                  rust_instrumented=False, pthread_forced_unwind_qualified=False)
    try:
        if sys.platform != 'linux':
            raise ValueError('Actual core SDK qualification requires Linux')
        if a.sdk_report:
            prior = json.loads(a.sdk_report.read_text())
            if not prior.get('passed'):
                raise ValueError('The prerequisite SDK qualification did not pass')
            for engine in ['upstream', 'hybrid']:
                library = a.core.resolve() / engine / 'lib/libswoole.so'
                if sha(library) != prior['sdks'][engine]['library_sha256']:
                    raise ValueError('Prerequisite SDK library changed: ' + engine)
            report['prerequisite'] = dict(path=str(a.sdk_report), sha256=sha(a.sdk_report))
        else:
            command(report, [sys.executable, ROOT / 'tools/core_tests.py', '--source', a.source,
                '--config', a.config, '--archive', archive, '--build-root', a.core, '--jobs', a.jobs,
                '--file', 'core-tests/src/os/pipe.cpp', '--file', 'core-tests/src/os/msg_queue.cpp',
                '--file', 'core-tests/src/os/process_pool.cpp', '--output', work / 'originals.json'], timeout=1800)
        build = a.core.resolve() / 'hybrid/build'
        compiler, flags, libraries = sdk_flags(build)
        report['sdk'] = dict(library_sha256=sha(build.parent / 'lib/libswoole.so'),
                             config_sha256=sha(build.parent / 'config.h'),
                             provider_header_sha256=sha(prefix / 'include/swoole_rs_native_effect.h'))
        for standard in ['c++14', 'c++17']:
            binary = work / ('sdk-' + standard)
            command(report, [compiler, *flags, '-std=' + standard, '-UNDEBUG', fixture, register,
                             '-o', binary, *libraries], cwd=build)
            observed = command(report, [binary], cwd=build, timeout=30)
            if observed.stdout != 'production Server native effect custody: PASS\n':
                raise ValueError('Actual SDK custody witness did not complete')
            for path in [binary, build.parent / 'lib/libswoole.so']:
                symbols = command(report, ['nm', '-D', '--undefined-only', path]).stdout
                if re.search(r'\b(?:zend_\w+|php_\w+|executor_globals|core_globals)(?:@\S+)?\s*$', symbols, re.M):
                    raise ValueError('PHP-disabled SDK gained a Zend/PHP relocation')
            for sanitize in [False, True]:
                probe = work / ('prefix-' + standard + ('-asan' if sanitize else ''))
                argv = [compiler, '-std=' + standard, '-O2', '-g', '-UNDEBUG', '-Wall', '-Wextra', '-Werror',
                        '-DWRAP_FREE', '-I' + str(prefix / 'include'), typed, archive,
                        '-pthread', '-ldl', '-lm', '-Wl,--wrap=free', '-o', probe]
                if sanitize:
                    argv += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
                command(report, argv)
                observed = command(report, [probe], timeout=30)
                if not re.fullmatch(r'production Server typed prefix propagation: \d+ trials, 3 Rust String drops PASS\n', observed.stdout):
                    raise ValueError('Typed prefix/drop witness did not complete')
        report['injected_provider_qualified'] = True
        if a.php:
            if not all([a.php_config, a.native_extension, a.selected_extension, a.selected_source]):
                raise ValueError('Actual PHP pair requires all PHP configuration, extension and selected-source inputs')
            php_fixture = fixture.with_suffix('.php')
            php_cpp = fixture.with_name('server_effects_php.cc')
            report['sources'].update({path.name: sha(path) for path in [php_fixture, php_cpp]})
            includes = shlex.split(command(report, [a.php_config, '--includes']).stdout)
            identities = {name: sha(path) for name, path in [('native', a.native_extension), ('selected', a.selected_extension)]}
            report['php_extensions_sha256'] = identities
            native_sdk = work / 'native-php-sdk'
            if native_sdk.exists():
                shutil.rmtree(native_sdk)
            command(report, ['git', 'clone', '-q', '--shared', '--no-checkout', a.source, native_sdk])
            from tools.build_upstream import COMMIT
            command(report, ['git', '-C', native_sdk, 'checkout', '-q', '--detach', COMMIT])
            shutil.copy2(a.selected_source / 'config.h', native_sdk / 'config.h')
            for name, source, extension in [('native', native_sdk, a.native_extension),
                                             ('selected', a.selected_source.resolve(), a.selected_extension)]:
                for standard in ['c++14', 'c++17']:
                    companion = work / (name + '-' + standard + '.so')
                    # Native pin headers do not interpret Rust selection macros.
                    # The actual selected ZTS feature config matches this PHP pair.
                    command(report, [compiler, '-std=' + standard, '-O2', '-g', '-shared', '-fPIC',
                        '-DHAVE_CONFIG_H', '-DZEND_ENABLE_STATIC_TSRMLS_CACHE=1', *includes,
                        '-I' + str(source), '-I' + str(source / 'include'),
                        '-I' + str(source / 'ext-src'), '-I' + str(source / 'thirdparty'),
                        '-I' + str(prefix / 'include'), php_cpp, '-o', companion])
                    for mode, expected in [('normal', '["normal",0,1]\n'), ('fatal', '["fatal",255,0]\n'),
                                           ('custom', '["custom",235,0]\n'), ('destructor', '["destructor",235,0]\n')]:
                        observed = command(report, [a.php, '-n', '-d', 'extension=' + str(extension),
                            '-d', 'extension=' + str(companion), php_fixture, mode], timeout=30)
                        if observed.stdout != expected or observed.stderr:
                            raise ValueError('Actual PHP Server outcome/post-call witness failed')
            if any(sha(path) != identities[name] for name, path in [('native', a.native_extension), ('selected', a.selected_extension)]):
                raise ValueError('Actual PHP extension changed during qualification')
            report['actual_php_qualified'] = True
            report['php_scope'] = 'Isolated Core SDK callback in a PHP Thread; no start/fork. Abandoned fixture Server lifetime ends with process; no wider lifetime/leak claim.'
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'injected_provider_qualified', 'actual_php_qualified', 'error']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
