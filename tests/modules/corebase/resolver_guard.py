#!/usr/bin/env python3
"""Actual Rust request/provider cleanup under a controlled SDK result and contained C++ exception; real DNS is qualified separately."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_ext import native_flags
from tools.harness import run_isolated
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--archive', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
a.output.parent.mkdir(parents=True, exist_ok=True)
report = dict(passed=False, scope=__doc__, commands=[], native_sanitizers=True, rust_instrumented=False)
try:
    flags = native_flags(a.archive)
    with tempfile.TemporaryDirectory(prefix='resolver-guard-', dir=a.output.parent) as temporary:
        binary = Path(temporary) / 'guard'
        command = ['c++', '-std=c++14', '-O2', '-g', '-Wall', '-Wextra', '-Werror',
                   '-fsanitize=address,undefined', '-fno-sanitize-recover=all', *flags,
                   str(Path(__file__).with_suffix('.cc')), str(a.archive.resolve()), '-o', str(binary)]
        command += ['-Wl,-dead_strip'] if sys.platform == 'darwin' else ['-Wl,--gc-sections', '-ldl', '-lpthread', '-lm']
        for invocation in [command, [str(binary)]]:
            result = run_isolated(invocation, timeout=90)
            report['commands'].append(dict(argv=invocation, exit=result.returncode, stdout=result.stdout, stderr=result.stderr))
            if result.returncode:
                raise RuntimeError('Actual resolver diagnostic guard failed')
        report.update(passed=True, outcome=json.loads(result.stdout))
except Exception as error:
    report['error'] = str(error)
a.output.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({key: report[key] for key in ['passed', 'scope']}))
raise SystemExit(not report['passed'])
