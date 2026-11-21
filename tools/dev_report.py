#!/usr/bin/env python3
"""Inspect the latest development loop: bottlenecks, cache decisions and failures."""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
import math
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.harness import ROOT


MAX_JSON_BYTES = 32 * 1024 * 1024


def read_json(path, with_digest=False):
    """Bound memory and refuse a receipt changed while being read."""
    with Path(path).open('rb') as handle:
        before = os.fstat(handle.fileno())
        raw = handle.read(MAX_JSON_BYTES + 1)
        after = os.fstat(handle.fileno())
    if len(raw) > MAX_JSON_BYTES:
        raise ValueError(f'JSON exceeds {MAX_JSON_BYTES} byte inspection limit')
    if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
        raise ValueError('Receipt changed during inspection; select a completed run')
    data = json.loads(raw)
    if not isinstance(data, dict):
        raise ValueError('Expected a JSON object')
    return (data, hashlib.sha256(raw).hexdigest()) if with_digest else data


def phase_failed(row):
    """Keep raw exit status and explicit gate verdict separate."""
    return bool(row.get('returncode')) or row.get('failed') is True


def engine_name(item, default=None):
    engine = item.get('engine')
    if isinstance(engine, dict):
        engine = engine.get('name')
    return engine if isinstance(engine, str) else default


def engine_observations(data):
    """Normalize recorded list/map identities without deriving test outcomes."""
    engines = data.get('engines', [])
    if isinstance(engines, dict):
        return [dict(value, name=value.get('name') or name) for name, value in engines.items()
                if isinstance(value, dict)]
    if isinstance(engines, list):
        return [value for value in engines if isinstance(value, dict) and value.get('name')]
    return []


def case_status(item):
    status = item.get('status')
    if status is None and isinstance(item.get('passed'), bool):
        return 'passed' if item['passed'] else 'failed'
    return status


def case_name(item, default=None):
    return next((item[key] for key in ['test', 'project', 'case', 'name'] if item.get(key) is not None), default)


def problem_detail(item):
    text = str(item.get('detail') or item.get('error') or item.get('stderr') or item.get('stdout')
               or item.get('sanitizer_warnings') or '')
    markers = [text.find(marker) for marker in ['Assert failed:', 'Fatal error:', 'ERROR: AddressSanitizer',
                                                'runtime error:', 'WARNING:']]
    start = min((index for index in markers if index >= 0), default=0)
    return text[start:start + 600]


def outcomes(data):
    """Keep every recorded status; compare exact case identities, never timings."""
    counts = defaultdict(Counter)
    cases = defaultdict(lambda: defaultdict(list))
    problems = []
    results = data.get('results', [])
    if isinstance(results, dict):
        results = []  # Some native gates store aggregate counts, not case outcomes.
    if not isinstance(results, list):
        raise ValueError('Expected results to be a list or aggregate object')
    for item in results:
        if not isinstance(item, dict):
            continue
        status = case_status(item)
        if status is None:
            continue
        engine = engine_name(item, 'unspecified')
        case = case_name(item, 'unknown')
        counts[engine][str(status)] += 1
        key = (str(case), item.get('sha256'), item.get('backend'))
        if case != 'unknown':
            cases[key][engine].append(status)
        if status != 'passed':
            problems.append({'engine': engine_name(item), 'case': case, 'status': status,
                             'detail': problem_detail(item),
                             'returncode': item.get('returncode', item.get('exit_code')),
                             'runner_log': item.get('runner_log'), 'artifacts': item.get('artifacts')})
    comparisons = []
    for (case, checksum, backend), engines in cases.items():
        # A missing control, changed assertion, or repeated case is not a matched comparison.
        candidate = next((name for name in ('cargo', 'hybrid') if name in engines), 'rust')
        if candidate not in engines or 'upstream' not in engines:
            continue
        hybrid, upstream = engines[candidate], engines['upstream']
        if len(hybrid) != 1 or len(upstream) != 1:
            relation = 'ambiguous repeated case'
        elif hybrid[0] == upstream[0] == 'passed':
            continue
        elif hybrid[0] == upstream[0]:
            relation = 'same recorded status'
        else:
            relation = 'different recorded statuses'
        comparisons.append({'case': case, 'sha256': checksum, 'backend': backend,
                            candidate: hybrid, 'upstream': upstream, 'relation': relation})
    comparisons.sort(key=lambda row: row['relation'] != 'different recorded statuses')
    return dict(counts), problems, comparisons


def nested_problems(data):
    """Expose explicit nested non-green evidence, without inventing case verdicts."""
    found = []
    stack = [(data, '$', None)]
    while stack:
        value, path, inherited_engine = stack.pop()
        if isinstance(value, list):
            stack.extend((child, f'{path}[{index}]', inherited_engine)
                         for index, child in reversed(list(enumerate(value))))
        elif isinstance(value, dict):
            engine = engine_name(value, inherited_engine)
            status = value.get('status')
            non_green = {key for key in ['status', 'coverage_status', 'execution_status']
                         if isinstance(value.get(key), str) and value[key].lower() in {
                             'failed', 'failure', 'fail', 'error', 'skipped', 'skip', 'warned', 'warning',
                             'unsupported', 'blocked', 'not_run', 'not-run', 'xfail', 'xfailed', 'bork',
                             'timeout', 'timed_out', 'unavailable', 'interrupted', 'running', 'pending'}}
            passed = value.get('passed')
            code = value.get('returncode', value.get('exit_code', value.get('exit')))
            # Direct results already have their own per-case counts. Descendants still matter.
            direct_case = path.startswith('$.results[') and path.count('[') == 1 and path.endswith(']')
            direct_case = direct_case and not (non_green - {'status'} or value.get('sanitizer_warnings') or
                                               (passed is False and status == 'passed'))
            if path != '$' and not direct_case and (
                    non_green or passed is False or
                    (type(code) is int and code != 0) or value.get('sanitizer_warnings')):
                found.append({'path': path, 'engine': engine, 'status': status, 'passed': passed,
                              'returncode': code, 'coverage_status': value.get('coverage_status'),
                              'execution_status': value.get('execution_status'),
                              'log': value.get('log'), 'detail': problem_detail(value)})
            for key, child in reversed(list(value.items())):
                # Engine-keyed receipts are common in native gates. Preserve this recorded label.
                label = key if path == '$.engines' else engine
                if isinstance(child, (dict, list)):
                    stack.append((child, f'{path}.{key}', label))
    return found


def build_identity(build, report):
    raw = build.get('build_manifest')
    if not raw:
        return None
    # Only immutable build receipts qualify. Never inspect a moving extension or HEAD.
    path = sidecar(raw, report)
    data, digest = read_json(path, with_digest=True)
    if data.get('build_mode') != 'immutable':
        raise ValueError(f"Build receipt is not immutable (recorded mode {data.get('build_mode')!r}): {path}")
    inputs = data.get('inputs', {})
    return {'manifest': str(path), 'manifest_sha256': digest,
            'build_mode': data.get('build_mode'),
            'extension_sha256': data.get('extension_sha256'),
            'source_diff_sha256': data.get('source_diff_sha256'),
            'feature_bits': data.get('migrated_feature_bits'),
            'upstream_commit': inputs.get('upstream_commit'),
            'rust_archive_sha256': inputs.get('rust_archive_sha256'),
            'rust_sources_sha256': inputs.get('rust_sources_sha256'),
            'modules': inputs.get('modules'), 'fix_halfclose': inputs.get('fix_halfclose')}


def latest(directory, lane=None):
    reports = sorted(Path(directory).glob('*/loop.json'), key=lambda path: path.stat().st_mtime_ns, reverse=True)
    for path in reports:
        if lane is None or read_json(path)['lane'] == lane:
            return path
    raise ValueError(f'No {lane or "development"} loop reports in {directory}; run tools/dev.py first')


def sidecar(raw, report):
    """Linux reports retain /work paths; their sibling files also work on the host."""
    path = Path(raw)
    sibling = report.parent / path.name
    if sibling.is_file():
        return sibling
    if path.is_relative_to('/work') and tuple(report.parts[-5:-1])[:3] == ('target', 'reports', 'dev'):
        # /work is the documented private Linux workspace, not another host checkout.
        return report.parents[4] / path.relative_to('/work')
    return path


def caches(value, prefix=''):
    if not isinstance(value, dict):
        return []
    if 'cache_hit' in value:
        return [{'artifact': prefix or 'build', 'hit': value['cache_hit'],
                 'reason': value.get('cache_reason', 'not recorded')}]
    return [row for name, child in value.items()
            for row in caches(child, '.'.join(part for part in [prefix, name] if part))]


def inspect(path, limit=5):
    path = Path(path).resolve(strict=True)
    data, digest = read_json(path, with_digest=True)
    phases = data['phases']
    if data['status'] == 'passed' and any(phase_failed(row) for row in phases):
        raise ValueError('Report says passed but contains a failed phase')
    summary = {'report': str(path), 'report_sha256': digest,
               'source_commit': data.get('source_commit'),
               'incomplete': data['status'] not in ['passed', 'failed'], 'lane': data['lane'], 'status': data['status'],
               'infrastructure_error': data.get('infrastructure_error'), 'blocked_phase': data.get('blocked_phase'),
               'created_utc': data.get('created_utc'),
               'seconds': data.get('seconds'), 'recorded_seconds': round(sum(row['seconds'] for row in phases), 4),
               'hypothesis': data.get('hypothesis'), 'expected_observation': data.get('expected_observation'),
               'slowest': [], 'failures': [], 'warnings': [], 'outcomes': [], 'identities': [],
               'engine_identities': [], 'phase_count': len(phases), 'failed_phase_count': sum(phase_failed(p) for p in phases)}
    enriched = []
    engine_identities = {}
    for phase in phases:
        decisions = caches(phase.get('build', {})) + caches(phase.get('compile', {}), 'compile')
        warnings = []
        problems = []
        command = phase.get('command', [])
        try:
            identity = build_identity(phase.get('build', {}), path)
            if identity:
                summary['identities'].append(identity)
        except (OSError, ValueError, TypeError) as error:
            warnings.append(f'Build identity unavailable: {error}')
        for index, argument in enumerate(command):
            if argument not in ['--output', '--report']:
                continue
            if index + 1 == len(command):
                warnings.append(f'Missing path after {argument}')
                continue
            artifact = sidecar(command[index + 1], path)
            try:
                outcome = read_json(artifact)
                decisions = caches(outcome) or decisions
                try:
                    counts, found, comparisons = outcomes(outcome)
                except (ValueError, TypeError, AttributeError) as error:
                    warnings.append(f'Case inventory unavailable: {artifact}: {error}')
                    counts, found, comparisons = {}, [], []
                nested = nested_problems(outcome)
                problems.extend(found)
                for engine in engine_observations(outcome):
                    key = json.dumps(engine, sort_keys=True)
                    identity = {name: value for name, value in engine.items()
                                if name.endswith('sha256') or name in
                                ['name', 'php', 'swoole', 'rust_bridge_abi', 'rust_feature_bits',
                                 'zts', 'extension', 'php_extensions', 'capabilities']}
                    recorded = engine_identities.setdefault(key, {
                        'identity': identity, 'observation_sha256': hashlib.sha256(key.encode()).hexdigest(),
                        'reports': [], 'phases': []})
                    if str(artifact) not in recorded['reports']:
                        recorded['reports'].append(str(artifact))
                    if phase['phase'] not in recorded['phases']:
                        recorded['phases'].append(phase['phase'])
                gate_status = outcome.get('status')
                if isinstance(outcome.get('passed'), bool):
                    gate_status = 'passed' if outcome['passed'] else 'failed'
                if gate_status == 'failed' and not phase['returncode']:
                    warnings.append('Receipt records a failed gate despite phase exit zero')
                coverage_status = outcome.get('coverage_status')
                if counts or gate_status is not None or nested or coverage_status is not None:
                    summary['outcomes'].append({'phase': phase['phase'], 'report': str(artifact),
                                                'counts': counts, 'gate_status': gate_status,
                                                'coverage_status': coverage_status,
                                                'gate_detail': problem_detail(outcome),
                                                'problem_count': len(found), 'problems': found[:limit],
                                                'nested_problem_count': len(nested),
                                                'nested_problems': nested[:limit],
                                                'execution_status': outcome.get('status', outcome.get('execution_status')),
                                                'comparison_count': len(comparisons),
                                                'comparisons': comparisons[:limit],
                                                'upstream_commit': outcome.get('upstream_commit',
                                                    outcome.get('upstream', {}).get('commit'))})
            except (OSError, ValueError, TypeError, AttributeError) as error:
                warnings.append(f'Artifact unavailable: {artifact}: {error}')
        summary['warnings'].extend({'phase': phase['phase'], 'detail': warning} for warning in warnings)
        row = {'phase': phase['phase'], 'seconds': phase['seconds'], 'returncode': phase['returncode'],
               'share_percent': round(100 * phase['seconds'] / summary['recorded_seconds'], 2) if summary['recorded_seconds'] else 0,
               'caches': decisions, 'warnings': warnings}
        enriched.append(row)
        if phase_failed(phase):
            log = None
            # Bound reading as well as output: compiler/test logs can be very large.
            try:
                if not phase.get('log'):
                    raise ValueError('No log path recorded')
                log = sidecar(phase['log'], path)
                with log.open('rb') as handle:
                    handle.seek(0, 2)
                    handle.seek(max(0, handle.tell() - 16384))
                    lines = handle.read().decode(errors='replace').splitlines()
                excerpt = '\n'.join(lines[-16:])[-4000:]
            except (OSError, ValueError, TypeError) as error:
                excerpt = f'Log unavailable: {error}'
            summary['failures'].append({'phase': phase['phase'], 'returncode': phase['returncode'],
                                        'log': str(log) if log else None, 'excerpt': excerpt,
                                        'problem_count': len(problems), 'problems': problems[:8], 'warnings': warnings})
    summary['engine_identities'] = list(engine_identities.values())
    summary['slowest'] = sorted(enriched, key=lambda row: row['seconds'], reverse=True)[:limit]
    if 'benchmarks_skipped' in data:
        summary['benchmarks_skipped'] = data['benchmarks_skipped']
    return summary


def comparison_snapshot(path, limit=5, allow_partial=False):
    """Retain repetitions and explicit incomplete-run observations."""
    path = Path(path).resolve(strict=True)
    data, digest = read_json(path, with_digest=True)
    incomplete = data.get('status') not in ['passed', 'failed']
    if incomplete and not allow_partial:
        raise ValueError(f'Comparison requires completed reports: {path}')
    phases = defaultdict(list)
    identities, warnings = [], []
    for phase in data['phases']:
        row = {'returncode': phase['returncode'], 'seconds': phase.get('seconds'),
               'runner': next(iter(phase.get('command', [])), None),
               'status': phase.get('status'), 'failed': phase_failed(phase), 'nested_problem_count': 0, 'nested_problems': [],
               'cases': [], 'engines': [], 'receipts': [], 'warnings': [], 'upstream_commits': []}
        try:
            identity = build_identity(phase.get('build', {}), path)
            if identity:
                identities.append(identity)
        except (OSError, ValueError, TypeError) as error:
            warnings.append(str(error))
        command = phase.get('command', [])
        for index, argument in enumerate(command):
            if argument not in ['--output', '--report']:
                continue
            try:
                if index + 1 == len(command):
                    raise ValueError(f'Missing path after {argument}')
                artifact = sidecar(command[index + 1], path)
                outcome, checksum = read_json(artifact, with_digest=True)
                row['receipts'].append({'path': str(artifact), 'sha256': checksum})
                nested = nested_problems(outcome)
                row['nested_problem_count'] += len(nested)
                row['nested_problems'].extend(dict(item, report=str(artifact)) for item in nested[:limit])
                upstream = outcome.get('upstream_commit', outcome.get('upstream', {}).get('commit'))
                if upstream:
                    row['upstream_commits'].append(upstream)
                row['engines'].extend(engine_observations(outcome))
                results = outcome.get('results', [])
                if not isinstance(results, list):
                    row['warnings'].append('No individual case inventory in receipt')
                    continue
                for item in results:
                    if not isinstance(item, dict):
                        row['warnings'].append('Malformed case entry')
                        continue
                    status = case_status(item)
                    row['cases'].append({'engine': engine_name(item),
                        'case': case_name(item),
                        'sha256': item.get('sha256'), 'backend': item.get('backend'), 'status': status})
            except (OSError, ValueError, TypeError, AttributeError) as error:
                row['warnings'].append(str(error))
        phases[phase['phase']].append(row)
    if data['status'] == 'passed' and any(phase_failed(row) for rows in phases.values() for row in rows):
        raise ValueError('Report says passed but contains a failed phase')
    engines = {json.dumps({key: value for key, value in engine.items()
                          if key.endswith('sha256') or key in ['name', 'php', 'swoole', 'zts', 'rust_feature_bits']},
                         sort_keys=True)
               for rows in phases.values() for row in rows for engine in row['engines']}
    return {'report': str(path), 'report_sha256': digest, 'source_commit': data.get('source_commit'),
            'lane': data['lane'], 'status': data['status'], 'incomplete': incomplete,
            'first_failed_phase': next(({'phase': phase['phase'], 'returncode': phase['returncode'],
                'runner': next(iter(phase.get('command', [])), None)}
                for phase in data['phases'] if phase_failed(phase)), None),
            'infrastructure_error': data.get('infrastructure_error'), 'blocked_phase': data.get('blocked_phase'),
            'identities': identities, 'engine_identities': [json.loads(key) for key in sorted(engines)],
            'warnings': warnings, 'phases': dict(phases)}


def valid_seconds(value):
    return type(value) in [int, float] and math.isfinite(value) and value >= 0


def compare(before, after, limit=5, allow_partial=False):
    old, new = (comparison_snapshot(path, limit, allow_partial) for path in [before, after])
    result = {'before': {k: v for k, v in old.items() if k != 'phases'},
              'after': {k: v for k, v in new.items() if k != 'phases'},
              'incomplete': old['incomplete'] or new['incomplete'],
              'timing_scope': 'Recorded development wall times only; concurrent work and differing configurations preclude benchmark or speed claims.',
              'runner_changes': [], 'phase_timings': [], 'phase_exits': [], 'phase_changes': defaultdict(list), 'incomparable': [], 'new_failures': [],
              'resolved_failures': [], 'unchanged_failures': [], 'other_status_changes': []}
    for name in sorted(old['phases'].keys() | new['phases'].keys()):
        left, right = old['phases'].get(name, []), new['phases'].get(name, [])
        times = [[row['seconds'] for row in rows] for rows in [left, right]]
        comparable_time = (len(left) == len(right) == 1 and
                           all(valid_seconds(values[0]) for values in times))
        result['phase_timings'].append({'phase': name, 'before_seconds': times[0], 'after_seconds': times[1],
            'delta_seconds': round(times[1][0] - times[0][0], 4) if comparable_time else None,
            'observation': 'paired unique phase' if comparable_time else 'missing, repeated or unrecorded timing'})
        result['phase_exits'].append({'phase': name, 'before': [r['returncode'] for r in left],
                                      'after': [r['returncode'] for r in right]})
        if len(left) == len(right) == 1:
            if left[0]['runner'] != right[0]['runner']:
                result['runner_changes'].append({'phase': name, 'before': left[0]['runner'],
                    'after': right[0]['runner'], 'both_recorded': all(row['runner'] is not None for row in [left[0], right[0]])})
            prior, current = left[0]['returncode'], right[0]['returncode']
            category = ('unknown_exits' if any(type(code) is not int for code in [prior, current]) else
                        'unchanged_failures' if prior and current else 'new_failures' if current else
                        'recovered_exits' if prior else 'unchanged_zero_exits')
        else:
            category = ('pending_or_absent_before' if not left and old['incomplete'] else
                        'pending_or_absent_after' if not right and new['incomplete'] else
                        'added' if not left else 'removed' if not right else 'repeated')
        result['phase_changes'][category].append({'phase': name,
            'before': [{key: row[key] for key in ['returncode', 'seconds', 'status', 'runner', 'nested_problem_count',
                                                 'nested_problems', 'receipts']} for row in left],
            'after': [{key: row[key] for key in ['returncode', 'seconds', 'status', 'runner', 'nested_problem_count',
                                                'nested_problems', 'receipts']} for row in right]})
        reasons = ['Incomplete run; no qualified case transition credit'] if result['incomplete'] else []
        unqualified_rows = []
        if len(left) != 1 or len(right) != 1:
            reasons.append('Missing or repeated phase; absence is not resolution')
        else:
            left, right = left[0], right[0]
            # Unit/build phases without case output still have explicit phase exit comparisons.
            if not reasons and not any(left['receipts'] + right['receipts'] + left['warnings'] + right['warnings']) and not any(
                    [left['returncode'], right['returncode']]):
                continue
            reasons.extend(left['warnings'] + right['warnings'])
            if any(type(row['returncode']) is not int for row in [left, right]):
                reasons.append('Unknown phase exit; no recovery credit')
            if left['nested_problem_count'] or right['nested_problem_count']:
                reasons.append('Nested non-green observations; review raw receipt before claiming case recovery')
            if not left['receipts'] or not right['receipts']:
                reasons.append('Missing case receipt')
            if not left['cases'] or not right['cases']:
                reasons.append('Missing individual case inventory')
            inventories = []
            for phase in [left, right]:
                index = defaultdict(list)
                for case in phase['cases']:
                    if any(case[key] is None for key in ['case', 'engine', 'sha256', 'status']):
                        reasons.append('Missing case assertion, engine or status identity')
                    key = json.dumps({k: v for k, v in case.items() if k != 'status'}, sort_keys=True)
                    index[key].append(case['status'])
                if any(len(statuses) != 1 for statuses in index.values()):
                    reasons.append('Repeated case identity; first outcomes remain ambiguous')
                inventories.append(index)
            for key in inventories[0].keys() & inventories[1].keys():
                identity = json.loads(key)
                before_status, after_status = inventories[0][key], inventories[1][key]
                if (all(identity[field] is not None for field in ['case', 'engine', 'sha256']) and
                        len(before_status) == len(after_status) == 1 and
                        before_status[0] is not None and after_status[0] is not None and
                        before_status != after_status):
                    unqualified_rows.append(dict(identity, before=before_status[0], after=after_status[0]))
            unqualified_rows.sort(key=lambda row: (str(row['engine']), str(row['case'])))
            if left['upstream_commits'] != right['upstream_commits']:
                reasons.append('Changed pinned upstream identity')
            if inventories[0].keys() != inventories[1].keys():
                reasons.append('Changed case/assertion/backend/engine inventory')
            # Different candidate binaries are the intended input. Runtime/control changes are not.
            engine_names = {case['engine'] for case in left['cases']}
            if engine_names & {'cargo', 'hybrid', 'rust'}:
                engine_names.add('upstream')
            for engine in engine_names:
                observations = [[e for e in phase['engines'] if e.get('name') == engine] for phase in [left, right]]
                fields = ['php', 'swoole', 'zts', 'php_extensions', 'capabilities']
                reference_fields = [key for key in ['sha256', 'binary_sha256', 'library_sha256']
                                    if any(e.get(key) for items in observations for e in items)]
                if engine == 'upstream':
                    fields.extend(reference_fields)
                if any(len(items) != 1 for items in observations):
                    reasons.append(f'Missing or repeated engine observation: {engine}')
                elif any(observations[0][0].get(field) != observations[1][0].get(field) for field in fields):
                    reasons.append(f'Changed runtime or reference identity: {engine}')
                elif engine == 'upstream' and not reference_fields:
                    reasons.append('Missing recorded reference binary identity')
            if not reasons:
                for key, statuses in inventories[0].items():
                    prior, current = statuses[0], inventories[1][key][0]
                    row = dict(json.loads(key), phase=name, before=prior, after=current)
                    if prior == current == 'failed':
                        result['unchanged_failures'].append(row)
                    elif prior == 'failed' and current == 'passed':
                        result['resolved_failures'].append(row)
                    elif prior == 'passed' and current == 'failed':
                        result['new_failures'].append(row)
                    elif prior != current:
                        result['other_status_changes'].append(row)
        if reasons:
            result['incomparable'].append({'phase': name, 'reasons': sorted(set(reasons)),
                'unqualified_status_change_count': len(unqualified_rows),
                'unqualified_status_changes': unqualified_rows[:limit],
                'before_case_count': sum(len(p['cases']) for p in old['phases'].get(name, [])),
                'after_case_count': sum(len(p['cases']) for p in new['phases'].get(name, [])),
                'before_receipts': [r for p in old['phases'].get(name, []) for r in p['receipts']],
                'after_receipts': [r for p in new['phases'].get(name, []) for r in p['receipts']]})
    result['incomparable'].sort(key=lambda row: not any(
        p['returncode'] for snapshot in [old, new] for p in snapshot['phases'].get(row['phase'], [])))
    result['unqualified_status_change_count'] = sum(
        row['unqualified_status_change_count'] for row in result['incomparable'])
    result['runner_change_count'] = len(result['runner_changes'])
    result['runner_changes'] = result['runner_changes'][:limit]
    result['phase_change_counts'] = {key: len(rows) for key, rows in result['phase_changes'].items()}
    result['phase_changes'] = {key: rows[:limit] for key, rows in result['phase_changes'].items()}
    for snapshot, side in [(old, 'before'), (new, 'after')]:
        result[side]['slowest'] = sorted(
            [dict(phase=name, occurrence=index + 1, seconds=row['seconds'], returncode=row['returncode'])
             for name, rows in snapshot['phases'].items() for index, row in enumerate(rows)
             if valid_seconds(row['seconds'])], key=lambda row: row['seconds'], reverse=True)[:limit]
    result['counts'] = {key: len(result[key]) for key in
                        ['new_failures', 'resolved_failures', 'unchanged_failures', 'other_status_changes', 'incomparable']}
    for key in result['counts']:
        result[key] = result[key][:limit]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path, nargs='?', help='Loop JSON; defaults to the latest local run')
    parser.add_argument('--compare', type=Path, metavar='BEFORE', help='Compare BEFORE against the selected report')
    parser.add_argument('--allow-partial', action='store_true', help='Allow partial comparison observations; no qualified case credit')
    parser.add_argument('--linux', action='store_true', help='Select the latest run in the persistent Linux workspace')
    parser.add_argument('--lane', choices=['edit', 'candidate', 'integrate'], help='Select the latest run of this lane')
    parser.add_argument('--limit', type=int, default=5, help='Number of slowest phases to show')
    parser.add_argument('--json', action='store_true', help='Emit a compact machine-readable summary')
    args = parser.parse_args()
    if args.limit < 1:
        parser.error('--limit must be positive')
    if args.allow_partial and not args.compare:
        parser.error('--allow-partial requires --compare')
    if args.report and (args.linux or args.lane):
        parser.error('Use an explicit report or --linux/--lane selectors, not both')
    try:
        directory = ROOT / ('target/linux-dev/target/reports/dev' if args.linux else 'target/reports/dev')
        selected = args.report or latest(directory, args.lane)
        summary = compare(args.compare, selected, args.limit, args.allow_partial) if args.compare else inspect(selected, args.limit)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(2, f'Cannot inspect loop: {error}\n')
    if args.compare:
        if args.json:
            print(json.dumps(summary, indent=2))
        else:
            print(f"Before: {summary['before']['report']} ({summary['before']['status']})")
            print(f"After:  {summary['after']['report']} ({summary['after']['status']})")
            print(summary['timing_scope'])
            if summary['incomplete']:
                print('Partial comparison: missing phases are pending or absent, never passes or resolutions.')
            for side in ['before', 'after']:
                print(f"First failed {side}: " + json.dumps(summary[side]['first_failed_phase']))
                print(f"Recorded {side} source commit: " + (summary[side]['source_commit'] or 'unavailable'))
                if summary[side]['infrastructure_error'] or summary[side]['blocked_phase']:
                    print(f"Blocked {side}: {summary[side]['blocked_phase']}; {summary[side]['infrastructure_error']}")
                for identity in summary[side]['identities']:
                    print(f'Recorded {side} build identity: ' + json.dumps(identity, sort_keys=True))
                for warning in summary[side]['warnings']:
                    print(f'Warning ({side}): {warning}')
            print('Recorded runner changes: ' + str(summary['runner_change_count']))
            for row in summary['runner_changes']:
                print('Runner observation (no linked runtime inference): ' + json.dumps(row, sort_keys=True))
            print('Phase exit transitions (no case/parity claim): ' + json.dumps(summary['phase_change_counts'], sort_keys=True))
            for side in ['before', 'after']:
                print(f"Slowest {side}: " + json.dumps(summary[side]['slowest'], sort_keys=True))
            timings = sorted(summary['phase_timings'], key=lambda row:
                (row['delta_seconds'] is not None, -abs(row['delta_seconds'] or 0)))
            print(f"Phase timing observations: {len(timings)} (details bounded by --limit)")
            for row in timings[:args.limit]:
                print('Phase timing observation: ' + json.dumps(row, sort_keys=True))
            for category, rows in summary['phase_changes'].items():
                if category != 'unchanged_zero_exits':
                    for row in rows:
                        print('Phase ' + category + ': ' + json.dumps(row, sort_keys=True))
            print('Unqualified recorded status changes (no recovery/regression claim): ' +
                  str(summary['unqualified_status_change_count']))
            print('Comparable exact-case transitions: ' + json.dumps(summary['counts'], sort_keys=True))
            for key in summary['counts']:
                for row in summary[key]:
                    print(key + ': ' + json.dumps(row, sort_keys=True))
                if summary['counts'][key] > len(summary[key]):
                    print(f"{key}: {summary['counts'][key] - len(summary[key])} more; increase --limit")
        return 2 if summary['incomplete'] else 0 if summary['after']['status'] == 'passed' else 1
    if args.json:
        print(json.dumps(summary, indent=2))
    else:
        total = f'{summary["seconds"]:.3f}s' if summary['seconds'] is not None else f'{summary["recorded_seconds"]:.3f}s recorded; incomplete'
        print(f'Recorded {summary["lane"]}: {summary["status"]}; {total}\n{summary["report"]}')
        if summary['incomplete']:
            print('Incomplete run: recorded phases are a partial snapshot; absent phases are not passes.')
        print('Recorded source commit: ' + (summary['source_commit'] or 'unavailable; not inferred from HEAD'))
        if summary['infrastructure_error'] or summary['blocked_phase']:
            print(f"Blocked phase: {summary['blocked_phase']}; {summary['infrastructure_error']}")
        if summary['created_utc']:
            print('Started (UTC): ' + summary['created_utc'])
        if summary['hypothesis']:
            print('Hypothesis: ' + summary['hypothesis'])
        if summary['expected_observation']:
            print('Expected: ' + summary['expected_observation'])
        print(f"Phases: {summary['phase_count']}; failed: {summary['failed_phase_count']}")
        for identity in summary['identities']:
            print('Recorded build identity: ' + json.dumps(identity, sort_keys=True))
        for recorded in summary['engine_identities']:
            identity = recorded['identity']
            print('Recorded engine: ' + json.dumps({key: identity.get(key) for key in
                  ['name', 'sha256', 'php', 'swoole', 'rust_feature_bits', 'zts']}, sort_keys=True))
        for warning in summary['warnings']:
            print(f"Warning ({warning['phase']}): {warning['detail']}")
        for outcome in summary['outcomes']:
            nonpasses = {engine: counts for engine, counts in outcome['counts'].items()
                         if any(status != 'passed' for status in counts)}
            if (nonpasses or outcome['gate_status'] not in [None, 'passed'] or outcome['nested_problem_count'] or
                    outcome['coverage_status'] not in [None, 'passed']):
                print(f"Outcomes {outcome['phase']} (gate={outcome['gate_status']}, coverage={outcome['coverage_status']}): " + json.dumps(outcome['counts'], sort_keys=True))
                if outcome['gate_detail'] and outcome['gate_status'] not in [None, 'passed']:
                    print('  Gate detail: ' + outcome['gate_detail'])
                for problem in outcome['problems']:
                    print(f"  {problem['engine']}: {problem['case']}: {problem['status']} "
                          f"exit={problem['returncode']}\n{problem['detail']}")
                    if problem['runner_log']:
                        print('  Runner: ' + problem['runner_log'])
                if outcome['problem_count'] > len(outcome['problems']):
                    print(f"  {outcome['problem_count'] - len(outcome['problems'])} more cases in {outcome['report']}")
                for nested in outcome['nested_problems']:
                    print('  Nested evidence: ' + json.dumps(nested, sort_keys=True))
                if outcome['nested_problem_count'] > len(outcome['nested_problems']):
                    print(f"  {outcome['nested_problem_count'] - len(outcome['nested_problems'])} more nested observations in {outcome['report']}")
                for comparison in outcome['comparisons']:
                    print('  ' + json.dumps(comparison, sort_keys=True))
                if outcome['comparison_count'] > len(outcome['comparisons']):
                    print(f"  {outcome['comparison_count'] - len(outcome['comparisons'])} more comparisons in {outcome['report']}")
        for row in summary['slowest']:
            print(f'  {row["seconds"]:8.3f}s ({row["share_percent"]:5.1f}%)  {row["phase"]}  exit={row["returncode"]}')
            for cache in row['caches']:
                print(f'             {cache["artifact"]}: {"hit" if cache["hit"] else "miss"}; {cache["reason"]}')
        for failure in summary['failures']:
            print(f'\nFailed {failure["phase"]} (exit {failure["returncode"]}): {failure["log"]}')
            if not failure['problems']:
                print(failure['excerpt'])
        if 'benchmarks_skipped' in summary:
            print('Benchmarks skipped: ' + summary['benchmarks_skipped'])
    return 0 if summary['status'] == 'passed' else 1 if summary['status'] == 'failed' else 2


if __name__ == '__main__':
    sys.exit(main())
