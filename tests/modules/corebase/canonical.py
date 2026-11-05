#!/usr/bin/env python3
"""Run the selected upstream base GTests on a full native library and the all-modules hybrid one."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import uuid

import hashlib
import os

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT
from tools.native_core import test_outcome
from supervisor import cooperative_signals, run_tool
from tools.native_core import library_identity

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def clean_environment():
    env = os.environ.copy()
    removed = {key: env.pop(key) for key in list(env)
               if key.startswith(('DYLD_PRINT_', 'LD_DEBUG', 'LD_TRACE_'))
               or key in ('LD_VERBOSE', 'LD_WARN', 'LD_SHOW_AUXV', 'LD_PROFILE', 'LD_PROFILE_OUTPUT')}
    for key in ('LD_PRELOAD', 'LD_AUDIT', 'DYLD_INSERT_LIBRARIES'):
        if env.get(key):
            raise ValueError('Injected libraries require a separate matched protocol: ' + key)
    return env, removed


MAC_EXCLUDED = ['coroutine/hook', 'coroutine/system', 'lock/lock', 'network/socket',
                'os/os', 'os/process_pool', 'os/signal', 'server/http_server', 'server/server', 'coroutine/iouring']
LINUX_CASES = 'datahead_dump dec2hex hex2dec random_string file_put_contents file_get_size version_compare common_divisor common_multiple shell_exec file_size eventdata_pack stack_defer_fn string_format dirname mkdir_recursive set_task_tmpdir set_task_tmpdir_in_threads version hook intersection itoa get_systemd_listen_fds type_size vsnprintf snprintf log_level trace_flag only_dump redirect_stdout fatal_error spinlock futex undefined_behavior'.split()
MAC_CASES = ['dec2hex', 'hex2dec', 'version_compare', 'common_divisor', 'common_multiple',
             'dirname', 'itoa', 'type_size', 'mkdir_recursive', 'hook', 'log_level', 'trace_flag']


def attest_originals(path, hybrid, config, archive):
    """Derive an identity only from completed unchanged-original execution records."""
    build = json.loads(path.read_text())
    if (build.get('status') != 'complete' or build.get('selected_cases_passed') is not True
            or build.get('source_commit') != COMMIT or (build['inputs']['hybrid'] is not None) != hybrid
            or build['inputs']['config_sha256'] != sha(config)):
        raise ValueError('Original case build/config/source precondition failed: ' + str(path))
    if build.get('excluded_units') and (build.get('passed') or build.get('whole_build_inventory_supported')):
        raise ValueError('Unsupported native inventory cannot be a full pass')
    if hybrid and build['inputs']['archive_sha256'] != sha(archive):
        raise ValueError('Selected full library used a different FFI archive')
    tree = Path(build['cache']) / 'src'
    if sha(tree / 'config.h') != sha(config):
        raise ValueError('Actual compiled configuration changed')
    library = build['native_library']
    if library_identity(library['path']) != library:
        raise ValueError('Actual full library changed')
    if sha(build['execution_binary']) != build['execution_binary_sha256']:
        raise ValueError('Actual original-test executable changed')
    rows = build.get('results', [])
    if not rows or len({row['name'] for row in rows}) != len(rows):
        raise ValueError('Missing/duplicate original results')
    for index, row in enumerate(rows):
        log = Path(row['log']); xml = log.parent / (str(index) + '-result.xml')
        if (row['status'] != 'passed' or row['exit'] != 0
                or row['native_library_before'] != library or row['native_library_after'] != library
                or sha(log) != row['log_sha256'] or sha(xml) != row['xml_sha256']
                or test_outcome(xml, row['name'], row['exit']).get('status') != 'passed'
                or '[ RUN      ] ' + row['name'] not in log.read_text()
                or '[       OK ] ' + row['name'] not in log.read_text()):
            raise ValueError('Original assertion/result/library identity failed: ' + row['name'])
    expected = {'base.' + name for name in MAC_CASES} if platform.system() == 'Darwin' else {'base.' + name for name in LINUX_CASES}
    if {row['name'] for row in rows} != expected:
        raise ValueError('Selected original name set differs from the supported bounded gate')
    return build, {'source_receipt_sha256': sha(path), 'selected_cases_passed': True,
                   'library': library['resolved_path'], 'library_before_sha256': library['sha256'],
                   'library_after_sha256': library['sha256']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'config', 'archive', 'output']:
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--openssl-dir', type=Path)
    parser.add_argument('--php-dir', type=Path, default=Path('/usr'))
    cache = parser.add_mutually_exclusive_group()
    cache.add_argument('--ccache', type=Path)
    cache.add_argument('--no-ccache', action='store_true')
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error('--jobs must be positive')
    for name in ['source', 'config', 'archive', 'output', 'php_dir']:
        setattr(args, name, getattr(args, name).resolve())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    attempt = args.output.parent / ('corebase-canonical-' + uuid.uuid4().hex); attempt.mkdir()
    if args.output.exists():
        shutil.copy2(args.output, attempt / 'previous-summary.json')
    report = dict(passed=False, status='running', utc=datetime.now(timezone.utc).isoformat(),
                  attempt=str(attempt), commands=[], builds={},
                  scope='Current exact FFI with the all-modules hybrid tree and same-config original comparator; selected original base GTests only. Unsupported full native inventory remains independently failed. No PHP, sanitizer, optional-context, performance or activation claim.')
    env = {}

    def checkpoint():
        args.output.write_text(json.dumps(report, indent=2) + '\n')

    def run(command):
        command = list(map(str, command))
        log = attempt / (str(len(report['commands'])) + '.log')
        row = dict(argv=command, execution='in-process Python supervisor; leaf tools own their bounded process groups', log=str(log))
        report['commands'].append(row); checkpoint()
        try:
            code = run_tool(command, log, ROOT, env=env)
            row['exit'] = code
            return code
        finally:
            checkpoint()

    try:
        env, report['removed_tracing'] = clean_environment()
        report.update(archive=str(args.archive), archive_sha256=sha(args.archive), config_sha256=sha(args.config),
                      runner_sha256=sha(__file__), environment={key: env.get(key) for key in ['PATH', 'PKG_CONFIG_PATH', 'LIBRARY_PATH']},
                      helper_sha256={name: sha(ROOT / name) for name in ['tools/native_core.py', 'tools/build_upstream.py',
                          'tools/harness.py', 'tools/build_hybrid.py', 'tools/check_migration.py',
                          'tests/modules/corebase/supervisor.py']})
        if subprocess.check_output(['git', '-C', str(args.source), 'rev-parse', 'HEAD'], text=True, timeout=30).strip() != COMMIT:
            raise ValueError('Read-only reference must be pinned upstream')
        is_mac = platform.system() == 'Darwin'
        if platform.system() not in ['Darwin', 'Linux']:
            raise ValueError('Unsupported platform; no gate executed')
        excluded = MAC_EXCLUDED if is_mac else ['coroutine/iouring']
        cases = ['base.' + name for name in MAC_CASES] if is_mac else ['base.*']
        common = [sys.executable, ROOT / 'tools/native_core.py', '--source', args.source, '--config', args.config,
                  '--jobs', str(args.jobs), '--php-dir', args.php_dir]
        for name, value in [('openssl-dir', args.openssl_dir), ('ccache', args.ccache)]:
            if value:
                common += ['--' + name, value.resolve()]
        if args.no_ccache:
            common.append('--no-ccache')
        for name in excluded:
            common += ['--exclude-unit', 'core-tests/src/' + name + '.cpp']
        for name in cases:
            common += ['--case', name]
        identity = {'engines': {}}
        for label, hybrid in [('native', False), ('rust', True)]:
            child = attempt / (label + '-originals.json')
            command = [*common, '--output', child]
            if hybrid:
                command += ['--archive', args.archive]
            code = run(command)
            # native_core deliberately exits 1 for explicit unsupported units.
            # It is never promoted to a full-inventory pass by this adapter.
            report['builds'][label] = dict(report=str(child), sha256=sha(child) if child.is_file() else None, exit=code)
            checkpoint()
            build, identity['engines'][label] = attest_originals(child, hybrid, args.config, args.archive)
            if code not in [0, 1] or (code == 1 and not build.get('excluded_units')):
                raise ValueError('Unexpected original gate exit: ' + label)
            report['builds'][label] = dict(report=str(child), sha256=sha(child), exit=code,
                selected_cases_passed=build['selected_cases_passed'], full_inventory_passed=build['passed'],
                excluded_units=build['excluded_units'], case_count=len(build['results']))
            checkpoint()
        identity_path = attempt / 'paired-original-identity.json'; identity_path.write_text(json.dumps(identity, indent=2) + '\n')
        report['full_native_inventory_passed'] = all(row['full_inventory_passed'] for row in report['builds'].values())
        if (sha(__file__) != report['runner_sha256'] or sha(args.archive) != report['archive_sha256'] or sha(args.config) != report['config_sha256']
                or any(sha(ROOT / name) != value for name, value in report['helper_sha256'].items())):
            raise RuntimeError('Input/helper changed during canonical gate')
        report.update(passed=True, status='complete')
    except BaseException as error:
        report.update(passed=False, status='interrupted' if isinstance(error, KeyboardInterrupt) else 'failed', error=str(error))
    checkpoint()
    print(json.dumps({key: report.get(key) for key in ['passed', 'status', 'error', 'attempt']}))
    return int(not report['passed'])


if __name__ == '__main__':
    with cooperative_signals():
        sys.exit(main())
