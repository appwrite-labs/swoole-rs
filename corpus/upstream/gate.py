#!/usr/bin/env python3
"""Merge sharded run.py reports and gate cargo on upstream.

Every PHPT that passes on upstream must pass on every other engine; anything else
exits 1. Tests failing on upstream too are reported, not hidden. Only the official
runner's PASSED counts as passed. Prints a Markdown summary; --output writes every
row as JSON, marking `rerun: passed` on an engine's row when the engine passed a re-run
of a test the re-runs don't gate.

A flagged test may be re-run (--recheck: up to 3 re-runs per engine, or one cargo
re-run when cargo's first failure timed out). If upstream fails a re-run, the test
is flaky on upstream if cargo passes one of its re-runs (up to 9 then), or if
upstream then passes fewer than a third of 9 re-runs (a test both engines almost always
fail, #998, #1948: 2 passes in 9 against cargo's 0 is no evidence of a regression);
otherwise it is a regression. If upstream passes every re-run, any failed cargo re-run is a regression
and passing them all is a pass on re-run. Cargo re-runs until it has both passed and
failed, so one failed re-run of a test racy on both engines doesn't decide it. When cargo both passed and failed its
re-runs, upstream gets more re-runs (up to 24 in all) before that verdict: a test racy
on both engines then shows an upstream failure too. Only regressions fail the gate, and
a flagged test without cargo re-runs stays one.

A re-run whose test passed but whose runner then aborted with the heap-corruption
signature both engines hit at the same rate (#200, #406) is inconclusive: it is left out
of the verdict and re-run once more. A test whose re-runs on an engine are all
inconclusive fails the gate, labelled inconclusive.

On a debug PHP (debug.yml) a failure may carry Zend's leak report or a failed assertion;
the summary lists those per engine, upstream's own too, and gates them as any failure.
"""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.annotations import annotate_each

# First match wins; the reason is the official runner's SKIPIF text.
CATEGORIES = [
    ('service', r'mysql|redis|pgsql|postgres|mongo|database|ftp|ssh|httpbin|docker|kafka|odbc|oracle'),
    ('network', r'online|offline|internet|network|dns|ipv6|proxy'),
    ('zts', r'\bn?zts\b|\bnts\b|thread'),
    ('stdext', r'stdext'),
    ('platform', r'darwin|macos|linux|windows|freebsd|arm|x86|io_uring|root'),
    ('extension', r'extension|not exist|not loaded|not support'),
]
ANSI = re.compile(r'\x1b\[[0-9;]*m')
# PHP's run-tests timeout marker, and run.py's own group deadline.
TIMED_OUT = re.compile(r'process timed out|exceeded its isolated group deadline')
# The official runner's own abort after a passing test (#200).
ABORTED = re.compile(r'corrupted double-linked list[\s\S]*Aborted')
# A debug PHP's report at request end, and a failed ZEND_ASSERT, in the test's output.
REPORTS = [('leak', re.compile(r'memory leaks? detected')), ('assertion', re.compile(r'Assertion .* failed'))]
RERUNS = 3
# Upstream re-runs for a test cargo failed every re-run of after upstream failed
# one (#998, #1948), and for a test cargo both passed and failed on re-run (#1285: an
# upstream race failing 5-10% of runs shows in 24 runs far more often than in 9).
EXTENDED_RERUNS = 9
MIXED_RERUNS = 24
STATUSES = ['passed', 'failed', 'skipped', 'excluded']
# Pinned tests of upstream fixes to code that is Rust now, by the open issue porting them
# (#1905 moved the pin to v6.2.5 ahead of the ports). The gate lists them instead of
# failing on them, and lists those cargo already passes so their entries go.
PENDING_PORTS = {}
PENDING = {f'tests/{test}': issue for issue, tests in PENDING_PORTS.items() for test in tests}


def skip_reason(detail):
    match = re.search(r'reason:\s*(.*)', ANSI.sub('', detail or ''))
    return match.group(1).strip() if match else ''


def category(reason):
    lowered = reason.lower()
    return next((name for name, pattern in CATEGORIES if re.search(pattern, lowered)), 'other')


def rows(reports):
    for report in reports:
        data = json.loads(Path(report).read_text())
        if data.get('execution_status', 'complete') != 'complete':
            raise ValueError(f'{report}: run {data["execution_status"]}')
        for row in data['results']:
            entry = {'test': row['test'], 'family': row['module'], 'engine': row.get('engine', 'upstream'),
                     'status': row['status'], 'upstream_status': row.get('upstream_status')}
            if row['status'] == 'excluded':
                entry['reason'] = row.get('detail', '')
            elif row['status'] == 'skipped':
                entry['reason'] = skip_reason(row.get('detail'))
                entry['category'] = category(entry['reason'])
            elif row['status'] == 'failed':
                entry['timed_out'] = bool(TIMED_OUT.search(row.get('detail') or ''))
                # WARNED failed its first attempt: that failure counts, whatever the runner did next.
                entry['inconclusive'] = (row.get('upstream_status') == 'PASSED'
                                         and bool(ABORTED.search(row.get('detail') or '')))
                entry['report'] = next((name for name, pattern in REPORTS if pattern.search(row.get('detail') or '')), None)
            yield entry


def compare(results):
    """(regressions, failing on both): a missing result counts as not passed."""
    status = defaultdict(dict)
    for row in results:
        status[row['test']][row['engine']] = row['status']
    others = sorted({row['engine'] for row in results} - {'upstream'})
    regressions = sorted(f'{test} ({engine}: {engines.get(engine, "missing")})' for test, engines in status.items()
                         if engines.get('upstream') == 'passed' and test not in PENDING
                         for engine in others if engines.get(engine) != 'passed')
    both = sorted(test for test, engines in status.items() if others and engines.get('upstream') == 'failed'
                  and all(engines.get(engine) == 'failed' for engine in others))
    return regressions, both


def flagged(results):
    """Tests that pass on upstream and not on some other engine."""
    return sorted({entry.split(' (')[0] for entry in compare(results)[0]})


def reruns(rechecked):
    runs = defaultdict(lambda: defaultdict(list))
    for row in rechecked:
        runs[row['test']][row['engine']].append('inconclusive' if row.get('inconclusive') else row['status'])
    return runs


def conclusive(statuses):
    return [status for status in statuses if status != 'inconclusive']


def recheck(tests, rechecked):
    """(regressions, flaky on upstream, passed on re-run, inconclusive) for flagged tests,
    from their re-runs. Regressions and inconclusive tests fail the gate."""
    runs = reruns(rechecked)
    regressions, flaky, recovered, inconclusive = [], [], [], []
    for test in tests:
        if any(statuses and not conclusive(statuses) for statuses in runs[test].values()):
            inconclusive.append(test)
            continue
        upstream = conclusive(runs[test].get('upstream', []))
        others = [status for engine, statuses in runs[test].items() if engine != 'upstream' for status in conclusive(statuses)]
        if any(status != 'passed' for status in upstream):
            # Upstream is noisy; a cargo pass shows cargo isn't simply broken, and so does
            # upstream failing most of its extended re-runs: the first run's pass was the outlier.
            outlier = others and len(upstream) >= EXTENDED_RERUNS and 3 * upstream.count('passed') < len(upstream)
            (flaky if 'passed' in others or outlier else regressions).append(test)
        elif not upstream or not others or any(status != 'passed' for status in others):
            regressions.append(test)
        else:
            recovered.append(test)
    return regressions, flaky, recovered, inconclusive


def pending(tests, rechecked, engine='upstream', results=()):
    """Flagged tests still worth re-running on `engine`. Upstream stops at its first failed
    re-run; `upstream-extended` continues upstream, to 24 re-runs for a test cargo
    both passed and failed, or to 9 for one it failed every re-run of after upstream
    failed one. Another engine stops once its verdict is settled (a pass after an upstream
    failure, otherwise both a pass and a failure, which earns those upstream re-runs) or its
    budget is spent: one re-run if its first failure timed out, nine once upstream failed a
    re-run (a test racy on both engines may fail several cargo re-runs before one passes),
    else three. A failure alone
    doesn't settle it: a test racy on both engines (#862) fails a cargo re-run as often as
    the first run. Inconclusive re-runs don't use the budget, but each engine stops
    after twice its budget in all."""
    runs = reruns(rechecked)

    def within(statuses, budget):
        return len(conclusive(statuses)) < budget and len(statuses) < 2 * budget
    if engine == 'upstream':
        return [test for test in tests if all(status in ('passed', 'inconclusive') for status in runs[test]['upstream'])
                and within(runs[test]['upstream'], RERUNS)]
    if engine == 'upstream-extended':
        def budget(test):
            others = [conclusive(statuses) for engine, statuses in runs[test].items() if engine != 'upstream']
            upstream = conclusive(runs[test].get('upstream', []))
            if 'failed' not in upstream:
                return MIXED_RERUNS if any('passed' in other and 'failed' in other for other in others) else 0
            return EXTENDED_RERUNS if any(other and 'passed' not in other for other in others) else 0
        return [test for test in tests if within(runs[test]['upstream'], budget(test))]
    timed_out = {row['test'] for row in results if row['engine'] == engine and row.get('timed_out')}
    left = []
    for test in tests:
        statuses = runs[test].get(engine, [])
        upstream_failed = any(status not in ('passed', 'inconclusive') for status in runs[test].get('upstream', []))
        settled = 'passed' in statuses and (upstream_failed or 'failed' in statuses)
        budget = 1 if test in timed_out else EXTENDED_RERUNS if upstream_failed else RERUNS
        if not settled and within(statuses, budget):
            left.append(test)
    return left


def flaky_listing(tests, rechecked):
    runs = reruns(rechecked)
    lines = [f'<details><summary>{len(tests)} flaky on upstream (failed an upstream re-run; cargo passed one, or upstream passed under a third of 9; not gated)</summary>', '']
    for test in tests:
        lines.append(f'- `{test}`: ' + '; '.join(f'{engine} {", ".join(statuses)}' for engine, statuses in sorted(runs[test].items())))
    return lines + ['', '</details>', '']


def mark_reruns(results, rechecked):
    """Mark `rerun: passed` on each non-upstream row whose engine passed a re-run of an ungated test."""
    confirmed, _, _, inconclusive = recheck(flagged(results), rechecked)
    runs = reruns(rechecked)
    for row in results:
        if row['engine'] != 'upstream' and row['test'] not in confirmed + inconclusive and 'passed' in runs[row['test']].get(row['engine'], []):
            row['rerun'] = 'passed'


def listing(title, tests):
    return [f'<details><summary>{len(tests)} {title}</summary>', '', *[f'- `{test}`' for test in tests], '', '</details>', '']


def markdown(results, rechecked=None):
    engines = sorted({row['engine'] for row in results}, key=lambda engine: (engine != 'upstream', engine))
    families = defaultdict(Counter)
    for row in results:
        families[row['family']][row['engine'], row['status']] += 1
    lines = []
    for engine in engines:
        statuses = Counter(row['status'] for row in results if row['engine'] == engine)
        skips = Counter(row['category'] for row in results if row['engine'] == engine and row['status'] == 'skipped')
        lines += [f'**{sum(statuses.values())} PHPTs** on {engine}: ' + ', '.join(f'{statuses[s]} {s}' for s in STATUSES),
                  '', 'Skips by reason: ' + ', '.join(f'{n} {c}' for c, n in skips.most_common()), '']
    exclusions = sorted({row['test'] + ': ' + row.get('reason', '') for row in results if row['status'] == 'excluded'})
    if exclusions:
        lines += listing('named originals excluded (not passed)', exclusions)
    regressions, both = compare(results)
    cargo = {row['test']: row['status'] for row in results if row['engine'] != 'upstream' and row['test'] in PENDING}
    lines += listing('pending a port (not gated)', [f'{test} (#{PENDING[test]}): {cargo[test]}' for test in sorted(cargo)
                                                    if cargo[test] != 'passed'])
    lines += listing('pending a port but passing here: drop them from PENDING_PORTS',
                     [test for test in sorted(cargo) if cargo[test] == 'passed'])
    if len(engines) > 1 and rechecked is not None:
        confirmed, flaky, recovered, inconclusive = recheck(flagged(results), rechecked)
        lines += listing('regressions: pass on upstream, fail on a re-run here', confirmed)
        lines += listing('inconclusive: every re-run on an engine ended in the runner abort (#200); gated', inconclusive)
        lines += flaky_listing(flaky, rechecked)
        lines += listing('passed on re-run on both engines (not gated)', recovered)
        lines += listing('failing on upstream too', both)
    elif len(engines) > 1:
        lines += listing('passing on upstream but not here (regressions)', regressions)
        lines += listing('failing on upstream too', both)
    reported = [f'{row["test"]} ({row["engine"]}: {row["report"]})' for row in results if row.get('report')]
    if reported:
        lines += listing('leak or assertion reports (gated like any failure)', reported)
    lines += ['| Family | ' + ' | '.join(f'{e} {s}' for e in engines for s in STATUSES) + ' |',
              '| --- |' + ' --- |' * (len(engines) * len(STATUSES))]
    lines += [f'| {family} | ' + ' | '.join(str(c[e, s]) for e in engines for s in STATUSES) + ' |'
              for family, c in sorted(families.items())]
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reports', nargs='+', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--recheck', type=Path, nargs='+', help='Reports of re-runs of the flagged tests')
    parser.add_argument('--print-regressions', action='store_true', help='Print flagged test paths and exit')
    parser.add_argument('--print-pending', metavar='ENGINE',
                        help='Print flagged tests still worth re-running on ENGINE (with --recheck), and exit')
    args = parser.parse_args()
    results = sorted(rows(args.reports), key=lambda row: (row['test'], row['engine']))
    if args.print_regressions:
        print('\n'.join(flagged(results)))
        return 0
    rechecked = list(rows(args.recheck)) if args.recheck else None
    if args.print_pending:
        print('\n'.join(pending(flagged(results), rechecked or [], args.print_pending, results)))
        return 0
    if args.output:
        if rechecked is not None:
            mark_reruns(results, rechecked)
        args.output.write_text(json.dumps(results, indent=1) + '\n')
    print(markdown(results, rechecked))
    if rechecked is not None:
        confirmed, flaky, recovered, inconclusive = recheck(flagged(results), rechecked)
        annotate_regressions(results, confirmed + inconclusive)
        annotate_each('notice', 'PHPT flaky on upstream', flaky)
        annotate_each('notice', 'PHPT passed on re-run', recovered)
        return int(bool(confirmed or inconclusive))
    regressions = compare(results)[0]
    annotate_each('error', 'PHPT regression', regressions)
    return int(bool(regressions))


def annotate_regressions(results, tests):
    """`::error title=PHPT regression::<test> (<engine>: <status>)` per gated test, as compare() names them."""
    failed = defaultdict(list)
    for row in results:
        if row['engine'] != 'upstream' and row['status'] != 'passed':
            failed[row['test']].append(f"{row['engine']}: {row['status']}")
    annotate_each('error', 'PHPT regression', [f"{test} ({', '.join(failed[test]) or 'failed'})" for test in tests])


if __name__ == '__main__':
    sys.exit(main())
