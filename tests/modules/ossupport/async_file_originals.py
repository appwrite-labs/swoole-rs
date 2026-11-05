#!/usr/bin/env python3
"""Run the unchanged original coroutine AsyncFile case on native, ossupport-only and complete lower-hook libraries."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT
from tools.harness import run_isolated
from tools.native_core import library_identity


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'config', 'archive', 'output']:
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--jobs', type=int, default=2)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    attempt = args.output.parent / ('async-file-originals-' + uuid.uuid4().hex)
    attempt.mkdir()
    case = 'coroutine_async_file.async_file'
    report = dict(passed=False, scope=__doc__, original_case=case, engines={},
                  full_native_inventory_passed=False, archive_sha256=sha(args.archive),
                  config_sha256=sha(args.config))
    try:
        if sys.platform != 'linux':
            raise ValueError('This gate requires the Linux C++14-compatible GoogleTest SDK; no cases executed')
        lower = ['base64', 'buffer', 'corebase', 'coroutine', 'crc32', 'error', 'framing', 'hooks',
                 'http', 'ossupport', 'protocols', 'reactor', 'timer', 'tls', 'websocket']
        for engine, modules in [('native', []), ('rust-ossupport', ['ossupport']), ('rust-hooks', lower)]:
            child = attempt / (engine + '.json')
            command = [sys.executable, str(ROOT / 'tools/native_core.py'), '--source', str(args.source),
                       '--config', str(args.config), '--jobs', str(args.jobs), '--case', case,
                       '--exclude-unit', 'core-tests/src/coroutine/iouring.cpp', '--output', str(child)]
            if cache := shutil.which('ccache'):
                command += ['--ccache', cache]
            if modules:
                command += [argument for module in modules for argument in ['--module', module]]
                command += ['--archive', str(args.archive)]
            result = run_isolated(command, cwd=ROOT, timeout=600)
            row = dict(command=command, exit=result.returncode, stdout=result.stdout,
                       stderr=result.stderr, report=str(child))
            report['engines'][engine] = row
            original = json.loads(child.read_text())
            assert original['status'] == 'complete' and original['selected_cases_passed'] is True
            assert original['source_commit'] == COMMIT and original['modules'] == modules
            assert original['inputs']['config_sha256'] == sha(args.config)
            if modules:
                assert original['inputs']['archive_sha256'] == sha(args.archive)
            assert result.returncode in [0, 1]
            if result.returncode:
                assert original['excluded_units'] and original['passed'] is False
            assert {entry['name'] for entry in original['results']} == {case}
            assert all(entry['status'] == 'passed' and entry['exit'] == 0 for entry in original['results'])
            assert library_identity(original['native_library']['path']) == original['native_library']
            row.update(selected_original_passed=True, full_native_inventory_passed=original['passed'],
                       native_library=original['native_library'], original_assertion_files=original['original_assertion_files'])
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ['passed', 'scope', 'full_native_inventory_passed']}))
    return not report['passed']


if __name__ == '__main__':
    raise SystemExit(main())
