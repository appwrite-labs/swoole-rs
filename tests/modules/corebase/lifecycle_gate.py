#!/usr/bin/env python3
"""Bounded real lifecycle gate from an attested full-native pair; never build a world."""
import argparse
import json
from pathlib import Path
import platform
import shutil
import sys
import uuid

from prepare_benchmark import ROOT, clean_environment, sha
from supervisor import cooperative_signals, run_tool
from tools.native_core import library_identity
from tools.build_hybrid import module_patches


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepared', type=Path, required=True,
                        help='Successful prepare_benchmark.py receipt with coherent original-case-checked full DSOs')
    parser.add_argument('--archive', type=Path, required=True, help='Exact selected FFI used to build the selected full DSO')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.prepared = args.prepared.resolve()
    args.archive = args.archive.resolve()
    args.output = args.output.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    attempt = args.output.resolve().parent / ('corebase-lifecycle-' + uuid.uuid4().hex)
    attempt.mkdir()
    if args.output.exists():
        shutil.copy2(args.output, attempt / 'previous-summary.json')
    report = dict(passed=False, status='running', attempt=str(attempt),
                  scope='Bounded actual init/cleanup/thread/fork/task-directory native effects and original-case-checked full-library pair. Full native inventory, PHP, inherited runtime state, optional contexts, sanitizers and performance remain separate.',
                  gates={}, sources={})
    try:
        env, report['removed_tracing'] = clean_environment()
        prepared = json.loads(args.prepared.read_text())
        if prepared.get('passed') is not True:
            raise ValueError('Preparation failed; no full native build attempted')
        report.update(prepared=str(args.prepared.resolve()), prepared_sha256=sha(args.prepared),
                      archive=str(args.archive.resolve()), archive_sha256=sha(args.archive))
        if sha(prepared['identity_receipt']) != prepared['identity_receipt_sha256']:
            raise ValueError('Preparation original-case identity changed')
        selected = prepared['engines']['rust']['build_inputs']
        native = prepared['engines']['native']['build_inputs']
        if (selected['modules'] != ['corebase'] or native['modules'] != []
                or selected['config_sha256'] != native['config_sha256']
                or selected['archive_sha256'] != report['archive_sha256']):
            raise ValueError('Require exact selected FFI, corebase-only callers and original same-config comparator')
        expected = [{**row, 'path': 'migration/patches/corebase/' + row['path']}
                    for row in module_patches(ROOT / 'migration/patches/corebase')]
        if selected['patches'] != expected:
            raise ValueError('Selected full DSO was not built with current owned overlay')
        if selected['headers']['swoole_rs_corebase.h'] != sha(ROOT / 'native/corebase/swoole_rs_corebase.h'):
            raise ValueError('Selected full DSO has a different lifecycle contract')
        for row in prepared['engines'].values():
            if sha(row['build_report']) != row['build_report_sha256'] or library_identity(row['library']['path']) != row['library']:
                raise ValueError('Prepared build report or full library changed')
        for name in ['prepare_benchmark.py', 'lifecycle_gate.py', 'supervisor.py', 'hook_lifetime.py', 'lifecycle_proof.py', 'hook-lifetime.cc', 'fork-lifetime.cc', 'fork-failure.cc', 'init-lifetime.cc', 'init-host-effects.cc', 'init-policy.rs', 'task-directory.cc', 'task-directory-lifetime.cc', 'task-directory-policy.rs']:
            file = Path(__file__).with_name(name)
            report['sources'][name] = sha(file)
            shutil.copy2(file, attempt / name)
        report['imported_helpers'] = {name: sha(ROOT / name) for name in ['tools/harness.py', 'tools/native_core.py', 'tools/build_hybrid.py', 'tools/build_upstream.py', 'tools/check_migration.py']}
        for name, driver in [('cleanup_thread', 'hooks'), ('fork_guards', 'fork'),
                             ('task_directory', 'task-directory'), ('task_lifetime', 'task-lifetime'),
                             *([('syscall_failures', 'fork-failure'), ('init_prefixes', 'init'), ('init_host_effects', 'init-host')] if platform.system() == 'Linux' else [])]:
            child = attempt / (name + '.json')
            command = [sys.executable, Path(__file__).with_name('hook_lifetime.py'), '--prepared', args.prepared,
                       '--driver', driver, '--require-fork', '--require-init', '--output', child]
            if driver in ['task-directory', 'task-lifetime']:
                command.append('--require-task-tmp')
            log = attempt / (name + '.log')
            gate = dict(passed=False, command=list(map(str, command)), execution='in-process supervisor; bounded leaf process groups', log=str(log), report=str(child))
            report['gates'][name] = gate
            exit_code = run_tool(command, log, ROOT, env=env)
            row = json.loads(child.read_text()) if child.is_file() else {}
            passed = exit_code == 0 and row.get('passed') is True
            gate.update(exit=exit_code, passed=passed)
            if not passed:
                raise RuntimeError('Lifecycle ' + name + ' failed; first child receipt retained')
        if (sha(args.prepared) != report['prepared_sha256'] or sha(args.archive) != report['archive_sha256']
                or any(sha(Path(__file__).with_name(name)) != value for name,value in report['sources'].items())
                or any(sha(ROOT / name) != value for name,value in report['imported_helpers'].items())):
            raise RuntimeError('Gate inputs changed during execution')
        report.update(passed=True, status='complete')
    except BaseException as error:
        report.update(passed=False, status='interrupted' if isinstance(error, KeyboardInterrupt) else 'failed', error=str(error))
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report.get(key) for key in ['passed', 'status', 'error', 'attempt']}))
    return int(not report['passed'])


if __name__ == '__main__':
    with cooperative_signals():
        sys.exit(main())
