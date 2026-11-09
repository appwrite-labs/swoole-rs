#!/usr/bin/env python3
"""Shields.io endpoint JSON for the README's migration progress badges.

  badges.py --output DIR [--phpt GATE_JSON... --commit SHA]

unsafe.json    `unsafe` blocks, fns and impls in non-test crate code (tools/budget.py), and
               how many crates forbid unsafe_code; the per-crate table goes to the step summary.
speed.json     callgrind instructions per request, Cargo build over upstream, from a merge-tier
               run's callgrind job (--callgrind): how many workloads beat upstream and the
               slowest one. speed.md, beside it, is the per-workload table the badge links to.
phpt.json      applicable pinned PHPTs the Cargo build passes in a merge-tier run (--phpt: that
               run's gate outputs, one per build variant; a test counts if it passes in any),
               with the commit it measured. corpus/upstream/not_applicable.json lists the
               tests left out, and why, in the step summary too.
coverage.json  lines of crates/ the Cargo build ran, from a nightly coverage.yml run's report
               (--coverage: tools/coverage.py's output directory). coverage.md, beside it,
               is the per-crate table and the entry points no run reached.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from tools import budget  # noqa: E402

NOT_APPLICABLE = ROOT / 'corpus/upstream/not_applicable.json'


def phpt_passing(rows, not_applicable=()):
    """(Cargo build passes, first run or ungated re-run, over applicable pinned PHPTs) from every job's
    corpus/upstream/gate.py --output rows together."""
    tests = {row['test'] for row in rows} - set(not_applicable)
    passing = {row['test'] for row in rows if row['engine'] == 'cargo'
               and 'passed' in (row['status'], row.get('rerun'))}
    return len(passing & tests), len(tests)


def endpoint(label, message, color='blue'):
    return {'schemaVersion': 1, 'label': label, 'message': message, 'color': color}


def speed(result, limits, commit):
    """(speed.json endpoint, speed.md table) from benchmarks/callgrind.py's --output and the limits.
    Ratios are the Cargo build over upstream instructions per request: below 1 is faster than upstream.
    A result from before the Cargo build replaced the hybrid (#2414) has a `hybrid` row instead."""
    measured = {name: row.get('cargo', row.get('hybrid')) for name, row in result['instructions_per_request'].items()}
    ratios = {name: measured[name] / row['upstream'] for name, row in result['instructions_per_request'].items()}
    mean = 1.0
    for ratio in ratios.values():
        mean *= ratio ** (1 / len(ratios))
    slowest = max(ratios, key=ratios.get)
    worst = ratios[slowest]
    color = 'brightgreen' if worst <= 1 else 'yellow' if worst <= 1.05 else 'orange'
    faster = sum(ratio <= 1 for ratio in ratios.values())
    badge = endpoint('speed vs upstream', f'{faster}/{len(ratios)} faster · slowest {slowest} {worst:.2f}×', color)
    rows = ''.join(
        f"| {name} | {measured[name]:,.0f} | {row['upstream']:,.0f} | {ratios[name]:.4f}× | "
        f"{limits.get(name, '')}{'×' if name in limits else ''} | {'✅' if ratios[name] <= 1 else '❌' if name in limits and ratios[name] > limits[name] else '➖'} |\n"
        for name, row in result['instructions_per_request'].items())
    table = (f'# Speed vs upstream Swoole\n\nCallgrind instructions per request (lower is better), measured by the '
             f'merge tier on `{commit[:7]}` (`benchmarks/callgrind.py`, {result.get("requests", "?")} requests). '
             f'Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is '
             f'`benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.\n\n'
             f'| workload | cargo | upstream | ratio | limit | |\n| --- | ---: | ---: | ---: | ---: | --- |\n{rows}\n'
             f'Geometric mean {mean:.4f}×. Goal: ≤ 1.00× on every workload '
             f'([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).\n')
    return badge, table


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--phpt', type=Path, nargs='+', help="A merge-tier run's gate outputs (phpt.json, phpt-zts.json, ...)")
    parser.add_argument('--commit', help='The commit that run measured')
    parser.add_argument('--callgrind', type=Path, help="A merge-tier run's callgrind.json")
    parser.add_argument('--callgrind-commit', help='The commit that callgrind run measured')
    parser.add_argument('--coverage', type=Path, help="A coverage.yml run's report (summary.json, coverage.md)")
    args = parser.parse_args()
    if args.phpt and not args.commit:
        parser.error('--phpt needs --commit')
    if args.callgrind and not args.callgrind_commit:
        parser.error('--callgrind needs --callgrind-commit')
    args.output.mkdir(parents=True, exist_ok=True)
    counts = budget.counts()
    unsafe, unforbidden = counts['unsafe'], counts['unforbidden']
    not_applicable = json.loads(NOT_APPLICABLE.read_text())
    if summary := os.environ.get('GITHUB_STEP_SUMMARY'):
        rows = ''.join(f'| {crate} | {n} |\n' for crate, n in sorted(unsafe.items(), key=lambda item: -item[1]))
        excluded = ''.join(f'| `{test}` | {reason} |\n' for test, reason in sorted(not_applicable.items()))
        with open(summary, 'a') as out:
            out.write(f'| crate | unsafe |\n| --- | ---: |\n{rows}\n'
                      f'| PHPT not applicable | reason |\n| --- | --- |\n{excluded}')
    badges = {
        'unsafe': endpoint('unsafe blocks', f'{sum(unsafe.values()):,} · '
                           f'{len(unforbidden) - sum(unforbidden.values())}/{len(unforbidden)} crates safe'),
    }
    if args.phpt:
        rows = [row for path in args.phpt for row in json.loads(path.read_text())]
        passing, tests = phpt_passing(rows, not_applicable)
        badges['phpt'] = endpoint('upstream PHPTs passing', f'{passing:,} / {tests:,} @ {args.commit[:7]}')
    if args.callgrind:
        limits = json.loads((ROOT / 'benchmarks/callgrind_limits.json').read_text())
        badges['speed'], table = speed(json.loads(args.callgrind.read_text()), limits, args.callgrind_commit)
        (args.output / 'speed.md').write_text(table)
    if args.coverage:
        summary = json.loads((args.coverage / 'summary.json').read_text())
        badges['coverage'] = endpoint('crates/ line coverage', f"{100 * summary['covered'] / summary['lines']:.1f}% "
                                      f"of {summary['lines'] / 1000:.0f}k lines @ {summary['commit'][:7]}")
        shutil.copy2(args.coverage / 'coverage.md', args.output / 'coverage.md')
    for name, badge in badges.items():
        (args.output / (name + '.json')).write_text(json.dumps(badge) + '\n')
        print(f"{name}: {badge['message']}")


if __name__ == '__main__':
    main()
