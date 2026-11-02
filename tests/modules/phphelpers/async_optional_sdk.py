#!/usr/bin/env python3
"""Compile complete unchanged optional SQLite/ODBC callers; no linking/DB/runtime."""
import argparse
import hashlib
import json
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import uuid
from gate import ROOT, PIN
sys.path.insert(0, str(ROOT))
from tools.build_upstream import require_build_space
from tools.harness import run_isolated, write_report


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--sdk-report', type=Path, required=True)
    parser.add_argument('--provider', choices=['sqlite', 'odbc'], default='sqlite')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error('Use a unique receipt')
    provider = args.provider
    flag = 'SW_USE_' + provider.upper()
    expected = (['swoole_sqlite3_close', 'swoole_sqlite3_close_v2', 'swoole_sqlite3_exec',
                 'swoole_sqlite3_open_v2', 'swoole_sqlite3_prepare_v2', 'swoole_sqlite3_step'] if provider == 'sqlite' else
                ['swoole_odbc_' + name for name in ['SQLConnect', 'SQLDescribeCol', 'SQLDisconnect', 'SQLDriverConnect',
                 'SQLEndTran', 'SQLExecDirect', 'SQLExecute', 'SQLGetData', 'SQLMoreResults', 'SQLPrepare', 'SQLPutData']])
    work = output.parent / ('phphelpers-optional-' + provider + '-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    report = dict(passed=False, scope=__doc__, commands=[], work=str(work),
                  native_runtime_executed=False, measurement_performed=False,
                  optional_feature=flag, real_async_callers=len(expected))
    try:
        require_build_space(work)
        sdk = json.loads(args.sdk_report.read_text())
        if not sdk.get('passed') or sdk.get('pin') != PIN:
            raise ValueError('Require completed matching helper SDK')
        report['sdk_config_producer'] = sdk['selected_source_ref']
        vendor_flags = []
        extra_config = ''
        if provider == 'odbc':
            probe = run_isolated(['pkg-config', '--cflags', 'odbc'], timeout=20)
            report['odbc_pkg_config'] = dict(exit=probe.returncode, stdout=probe.stdout, stderr=probe.stderr)
            if probe.returncode or probe.stderr:
                raise ValueError('Installed ODBC SDK flags unavailable')
            vendor_flags = shlex.split(probe.stdout) + ['-DPDO_ODBC_TYPE="unixODBC"']
            extra_config = ''.join('#undef '+name+'\n#define '+name+' 1\n' for name in ['HAVE_SQL_H', 'HAVE_SQLTYPES_H', 'HAVE_SQLEXT_H'])
        name = 'ext-src/swoole_' + provider + '.cc'
        for engine in ['native', 'hybrid']:
            # Native compiles the pinned caller, hybrid the one in the SDK's hybrid build tree.
            unit = work / engine / Path(name).name
            unit.parent.mkdir()
            unit.write_bytes(subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':' + name]) if engine == 'native'
                             else (Path(sdk['hybrid_source']) / name).read_bytes())
            if unit.read_text().count('php_swoole_async(') != len(expected):
                raise ValueError('Optional async caller inventory changed: ' + engine)
            for standard in ['c++14', 'c++17']:
                row = next(item for item in sdk['commands'] if item['engine'] == engine and item['cpp_standard'] == standard and not item['stdext_enabled'] and item['unit'].endswith('php_swoole_cxx.cc'))
                old_profile = Path(sdk['work']) / (engine + '-profile-False')
                profile = work / (engine + '-' + standard)
                profile.mkdir()
                shutil.copy2(old_profile / 'php_swoole.h', profile / 'php_swoole.h')
                config = (old_profile / 'config.h').read_text()
                (profile / 'config.h').write_text(config + '\n/* Explicit private optional caller SDK profile, no runtime. */\n#undef ' + flag + '\n#define ' + flag + ' 1\n' + extra_config)
                # Reuse the completed SDK's exact compiler/include/feature inputs.
                command = row['command'][:row['command'].index('-MD')]
                command = [item for item in command if item != '-fsyntax-only']
                command = ['-I' + str(profile) if item == '-I' + str(old_profile) else item for item in command]
                obj, dep = profile / (provider + '.o'), profile / (provider + '.d')
                command += vendor_flags + ['-O2', '-Wall', '-Werror', '-MD', '-MF', str(dep), '-c', str(unit), '-o', str(obj)]
                require_build_space(work)
                result = run_isolated(command, timeout=120)
                observed = dict(engine=engine, cpp_standard=standard, command=command,
                                exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
                report['commands'].append(observed)
                if result.returncode == 0:
                    paths = shlex.split(dep.read_text().replace('\\\n', ' ').split(':', 1)[1])
                    consumed = {str(Path(path).resolve()) for path in paths}
                    header = str((Path(sdk['work']) / ('headers-' + engine) / 'ext-src/php_swoole_cxx.h').resolve())
                    if header not in consumed or sha(header) != row['dependencies_sha256'][header]:
                        raise ValueError('Optional caller did not consume same assigned whole header')
                    if not any(Path(path).name == ('sqlite3.h' if provider == 'sqlite' else 'sql.h') for path in consumed):
                        raise ValueError('Enabled optional TU did not consume vendor header')
                    symbols = run_isolated(['nm', '-u', str(obj)], timeout=20)
                    observed.update(nm_exit=symbols.returncode,
                                    undefined_symbols=symbols.stdout, nm_stderr=symbols.stderr)
                    imported = 'swr_v1_phphelpers_async' in symbols.stdout
                    observed['expected_async_import'] = symbols.returncode == 0 and not symbols.stderr and imported == (engine != 'native')
                    defined = run_isolated(['nm', '-g', str(obj)], timeout=20)
                    wrappers = sorted(set(re.findall(r'\bT\s+_?(swoole_' + ('sqlite3' if provider == 'sqlite' else 'odbc') + r'_\w+)$', defined.stdout, re.M)))
                    observed.update(defined_nm_exit=defined.returncode, defined_nm_stderr=defined.stderr,
                                    defined_symbols=defined.stdout, defined_wrappers=wrappers,
                                    all_async_wrappers_compiled=defined.returncode == 0 and not defined.stderr and set(expected).issubset(wrappers))
                write_report(output, report)
        report['passed'] = len(report['commands']) == 4 and all(row['exit'] == 0 and row.get('expected_async_import') and row.get('all_async_wrappers_compiled') for row in report['commands'])
    except Exception as error:
        report['passed'] = False
        report['error'] = str(error)
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
