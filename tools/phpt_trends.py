"""PHPT clearances across completed scheduled main runs of the merge tier."""
import json
from pathlib import Path
import subprocess
import tempfile

from corpus.upstream import gate


def read_evidence(directory):
    report = directory / 'clearances.json'
    if report.is_file():
        return json.loads(report.read_text())
    # Earlier phpt artifacts contain the same inputs, including every recheck.
    report = directory / 'phpt.json'
    if report.is_file():
        rechecked = list(gate.rows((directory / 'recheck').glob('*.json')))
        return gate.clearance_evidence(json.loads(report.read_text()), rechecked)
    return None


def trends(gh, count, share):
    runs = json.loads(gh('run', 'list', '--workflow', 'merge.yml', '--branch', 'main', '--event', 'schedule',
                         '--status', 'completed', '--limit', str(count), '--json', 'databaseId,url'))
    tests, upstream, missing = {}, set(), []
    for run in runs:
        with tempfile.TemporaryDirectory() as temp:
            try:
                gh('run', 'download', str(run['databaseId']), '-n', 'phpt', '-D', temp)
            except subprocess.CalledProcessError as error:
                if 'no valid artifacts' not in error.stderr and 'no artifacts' not in error.stderr:
                    raise
                evidence = None
            else:
                evidence = read_evidence(Path(temp))
        if evidence is None:
            missing.append(run['url'])
            continue
        upstream.update(evidence['upstream_failed'])
        for group in ('also_upstream', 'cargo_only'):
            for test in evidence[group]:
                tests.setdefault(test, {'also_upstream': [], 'cargo_only': []})[group].append(run['url'])
    measured = len(runs) - len(missing)
    rows = []
    for test, groups in sorted(tests.items()):
        cleared = sum(map(len, groups.values()))
        if cleared / measured > share:
            rows.append({'test': test, 'cleared': cleared, 'upstream_also_failed': test in upstream or bool(groups['also_upstream']),
                         **groups})
    return {'requested': count, 'measured': measured, 'missing': missing, 'tests': rows}


def file_issues(gh, report):
    # Missing runs could hide upstream failures or inflate the clearance rate.
    if report['measured'] != report['requested']:
        return
    for row in report['tests']:
        if row['upstream_also_failed']:
            continue
        test = row['test']
        title = f'Intermittent Cargo PHPT failure: {test}'
        existing = json.loads(gh('issue', 'list', '--state', 'open', '--label', 'kind:bug',
                                 '--search', f'"{test}" in:title,body',
                                 '--json', 'title,body'))
        if any(test in item['title'] or test in item['body'] for item in existing):
            continue
        body = (f'Cargo failed `{test}` and passed its rechecks in {row["cleared"]} of the last '
                f'{report["measured"]} scheduled main runs. Upstream had no observed failure for this test '
                'in those runs.\n\n' + '\n'.join(f'- {url}' for url in row['cargo_only']))
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'body.md'
            path.write_text(body)
            row['issue'] = gh('issue', 'create', '--title', title, '--body-file', str(path), '--label',
                              'kind:bug,p2,status:ready', '--assignee', 'loks0n').strip()
