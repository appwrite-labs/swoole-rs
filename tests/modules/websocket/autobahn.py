#!/usr/bin/env python3
"""Pinned external RFC conformance; never contributes to compatibility pass counts.

The first engine is the control: the run fails when a case does worse on another engine than
on it, in either field (OK, then NON-STRICT, then INFORMATIONAL, then anything else), or when
the suite's results are missing or incomplete. Strict passes are counted, not required.
"""
import argparse
import ast
from collections import Counter
from contextlib import ExitStack
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, server, write_report

IMAGE = 'crossbario/autobahn-testsuite@sha256:519915fb568b04c9383f70a1c405ae3ff44ab9e35835b085239c258b6fac3074'
PROFILE = ['1.*', '2.*', '3.*', '4.*', '5.*', '6.*', '7.*', '10.*', '12.1.1', '13.1.1']
RANK = {'OK': 3, 'NON-STRICT': 2, 'INFORMATIONAL': 1}


def regressions(control, engine):
    """Cases (by id) whose behavior or behaviorClose ranks lower on `engine` than on `control`."""
    def rank(cases, case, field):
        return RANK.get(cases.get(case, {}).get(field), 0)
    return [case for case in sorted(control)
            if any(rank(engine, case, field) < rank(control, case, field) for field in ('behavior', 'behaviorClose'))]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--cases', action='append', help='Autobahn case glob; repeat to replace bounded profile')
    parser.add_argument('--full', action='store_true', help='Include long performance/compression stress cases')
    parser.add_argument('--timeout', type=int, default=1800)
    parser.add_argument('--host', default='host.docker.internal', help='Docker-to-host address')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/autobahn.json')
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error('timeout must be positive')
    if args.full and args.cases:
        parser.error('choose --full or --cases')
    artifacts = args.output.resolve().parent / (args.output.stem + '-artifacts')
    artifacts.mkdir(parents=True, exist_ok=False)  # Never overwrite raw external failures.
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'suite_image': IMAGE,
              'purpose': 'RFC conformance, separate from pinned Swoole compatibility',
              'cases': args.cases or (['*'] if args.full else PROFILE),
              'profile': 'full' if args.full else 'custom' if args.cases else 'bounded-conformance',
              'exclude_cases': [], 'results': [], 'engines': []}
    name = 'swoole-rs-autobahn-' + uuid.uuid4().hex[:12]
    with ExitStack() as stack:
        servers = []
        for engine in parse_engines(args.engine, args.php, args.php_extension):
            report['engines'].append(engine.metadata())
            port, _ = stack.enter_context(server(engine, Path(__file__).with_suffix('.php')))
            servers.append({'agent': engine.name, 'url': f'ws://{args.host}:{port}'})
        config = {'outdir': '/reports', 'servers': servers, 'cases': report['cases'],
                  'exclude-cases': [], 'exclude-agent-cases': {}}
        (artifacts / 'config.json').write_text(json.dumps(config, indent=2))
        subprocess.run(['docker', 'pull', '-q', IMAGE], check=True, stdout=subprocess.DEVNULL)
        report['image_identity'] = subprocess.check_output(
            ['docker', 'image', 'inspect', IMAGE, '--format', '{{.Id}}'], text=True).strip()
        command = ['docker', 'run', '--rm', '--name', name, '--platform', 'linux/amd64',
                   '--add-host', 'host.docker.internal:host-gateway',
                   '-v', f'{artifacts}:/reports', IMAGE, 'wstest', '-m', 'fuzzingclient',
                   '-s', '/reports/config.json']
        try:
            with (artifacts / 'runner.log').open('w') as log:
                result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=args.timeout)
            report['exit_code'] = result.returncode
        except subprocess.TimeoutExpired:
            report['exit_code'] = 124
            report['error'] = 'Suite timed out; incomplete results cannot establish conformance'
        finally:
            subprocess.run(['docker', 'rm', '-f', name], capture_output=True)
        log_text = (artifacts / 'runner.log').read_text(errors='replace')
        plans = [line.removeprefix('Cases = ') for line in log_text.splitlines() if line.startswith('Cases = ')]
        planned = set(ast.literal_eval(plans[0])) if len(plans) == 1 else set()
        report['expected_cases_per_engine'] = len(planned)
        report['suite_version_line'] = next((line for line in log_text.splitlines()
            if line.startswith('Autobahn Fuzzing WebSocket Client')), None)
        if not planned:
            report['error'] = 'Missing or ambiguous suite case inventory'
        index = artifacts / 'index.json'
        if index.exists():
            data = json.loads(index.read_text())
            for engine in report['engines']:
                cases = data.get(engine['name'], {})
                missing = sorted(planned - set(cases))
                unexpected = sorted(set(cases) - planned)
                if missing or unexpected:
                    report['error'] = 'Incomplete or unexpected suite results'
                report['results'].append({'engine': engine['name'], 'total': len(cases),
                    'missing_cases': missing, 'unexpected_cases': unexpected,
                    'behavior_counts': dict(Counter(row['behavior'] for row in cases.values())),
                    'close_counts': dict(Counter(row['behaviorClose'] for row in cases.values())),
                    'strict_passed': sum(row['behavior'] == 'OK' and row['behaviorClose'] == 'OK' for row in cases.values())})
            control, *others = [engine['name'] for engine in report['engines']]
            report['regressions'] = {name: regressions(data.get(control, {}), data.get(name, {})) for name in others}
        else:
            report['error'] = report.get('error', 'Autobahn produced no master index')
        write_report(args.output, report)
    print(json.dumps(report, indent=2))
    return int(bool(report.get('error')) or report['exit_code'] != 0 or not report['results'] or
               any(row['total'] == 0 for row in report['results']) or any(report.get('regressions', {}).values()))


if __name__ == '__main__':
    sys.exit(main())
