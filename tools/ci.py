#!/usr/bin/env python3
"""Watch this repository's CI. GitHub merges pull requests itself: `gh pr merge --auto --squash`
puts a PR in main's merge queue once its PR tier passes.

  ci.py wait [--commit SHA]  wait for a workflow run (default: the merge tier on main); a run
                             replaced by a newer push to main follows the newer run
  ci.py status [--commit SHA]  the same, without waiting
  ci.py next                 the claimable issues in claim order (exit 2 when there are none),
                             without those whose title says they need workflow scope when gh's
                             token lacks it
  ci.py stuck                auto-merge PRs that conflict with main, failed a required check or
                             whose finished PR-tier run has none (GitHub abandoned its jobs:
                             re-run it), and PRs the merge queue dropped (fix what their run
                             broke, then queue them again), oldest first (exit 2 when there are none)
  ci.py phpt-baseline [--variant NAME] [--php VERSION]
                             `RUN SHA` of main's newest merge-tier run whose upstream PHPT reports
                             still stand for upstream at HEAD, so it needn't run (exit 2: none)
  ci.py new-failures LOOP... the failed phases in these dev.py loop.json reports that passed, or
                             didn't exist, in main's latest completed merge-tier run (exit 1 if any)
  ci.py new-failures --needs JSON --phases DIR (--save FILE | --baseline FILE)
                             a merge-tier run's failed phases and jobs, read inside the run from
                             its `needs` and `phases*` artifacts, no API call: saved on main as
                             the baseline, or held against it as the merge queue's `required` check

For a pull request: `ci.py wait --workflow pr.yml --commit BRANCH` for its PR tier (a branch
auto-merge deleted resolves through its PR). It returns at once (exit 4, state `conflicts`) when
the PR conflicts with main, since no PR-tier run starts until it's rebased. Nothing waits for a
merge: opening the PR ends an executor's part (AGENTS.md). Transient gh failures (5xx, network)
are retried until the deadline.

For the merge tier, failed phases come from its `phases` artifact, with new and fixed ones
against the previous completed run on main, each new one with the open issues that mention it
(or 'no open issue'); other failures show each failed job's annotations: the tools print one
per failed phase, PHPT regression and callgrind workload (tools/annotations.py).
Status also shows the latest completed merge-tier run separately while the requested run
is unfinished or absent. Its commit identifies the coverage; the exit code still describes
the requested run.

Progress goes to stderr and the result to stdout (--json for machine output).
Exit codes: 0 passed, 1 failed, 2 no run, still running or timed out, 3 cancelled.
Needs only `gh` (authenticated) and Python.
"""
import argparse
from datetime import datetime
from pathlib import Path
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.parse

EXIT = {'passed': 0, 'failed': 1, 'none': 2, 'running': 2, 'cancelled': 3, 'conflicts': 4}
GATE_JOB = 'phpt / gate'
# gate.py's annotation title for each regressed test, whose message starts with the test.
REGRESSION = 'PHPT regression'
# The merge-tier jobs (full-gates, or its shards "full-gates (N)") whose dev.py phases the
# `phases*` artifacts list, replacing their annotations.
PHASES_JOB = 'full-gates'


# Seconds between retries of a transient gh failure, doubling up to RETRY_CAP, until
# RETRY_DEADLINE (time.monotonic()); main() sets both from --timeout and --poll.
RETRY_DEADLINE, RETRY_BASE, RETRY_CAP = 0.0, 2.0, 60.0
TRANSIENT = ('HTTP 5', 'connection reset', 'timeout', 'timed out', 'EOF', 'TLS handshake',
             'connection refused', 'no such host', 'temporary failure')


def gh(*args):
    delay = RETRY_BASE
    while True:
        result = subprocess.run(['gh', *args], capture_output=True, text=True)
        if result.returncode == 0:
            return result.stdout
        # Every agent shares one account's hourly API limit (and every job the Actions token's):
        # wait for the reset when the deadline allows, else say when it comes.
        if 'API rate limit exceeded' in result.stderr:
            bucket = 'search' if len(args) > 1 and args[1].startswith('search/') else 'core'  # their own limits
            reset = subprocess.run(['gh', 'api', 'rate_limit', '--jq', f'.resources.{bucket}.reset'],
                                   capture_output=True, text=True).stdout.strip()
            wait = int(reset) - time.time() + 1 if reset.isdigit() else None
            at = f'{datetime.fromtimestamp(int(reset)):%H:%M:%S}' if wait is not None else 'an unknown time'
            if wait is not None and time.monotonic() + wait <= RETRY_DEADLINE:
                say(f'gh {args[0]}: API rate limit exceeded; waiting until {at}')
                time.sleep(max(wait, 0))
                continue
            raise subprocess.CalledProcessError(result.returncode, ['gh', *args], result.stdout,
                                                f'{result.stderr.strip()}; it resets at {at}')
        # A 4xx is permanent even when its message (say, a branch name) contains a TRANSIENT word.
        transient = (not re.search(r'HTTP 4\d\d', result.stderr)
                     and any(text.lower() in result.stderr.lower() for text in TRANSIENT))
        if not transient or time.monotonic() + delay > RETRY_DEADLINE:
            raise subprocess.CalledProcessError(result.returncode, ['gh', *args], result.stdout, result.stderr)
        say(f"gh {args[0]} {args[1] if len(args) > 1 else ''}: {result.stderr.strip()[:80]}; retrying in {delay:g}s")
        time.sleep(delay)
        delay = min(delay * 2, RETRY_CAP)


def say(message):
    print(f'{datetime.now():%H:%M:%S} {message}', file=sys.stderr, flush=True)


def resolve(commit):
    try:
        return gh('api', 'repos/{owner}/{repo}/commits/' + (commit or 'main'), '-q', '.sha').strip()
    except subprocess.CalledProcessError:
        # Auto-merge deletes a PR's branch: resolve it through that PR's head.
        head = gh('api', f'repos/{{owner}}/{{repo}}/pulls?state=all&head={{owner}}:{commit or "main"}',
                  '-q', '.[0].head.sha // ""').strip()
        if not head:
            raise
        return head


def seconds(job):
    # GitHub reports unfinished timestamps as 0001-01-01T00:00:00Z.
    started, completed = (datetime.fromisoformat(job[key]) if job.get(key) and not job[key].startswith('0001')
                          else None for key in ('startedAt', 'completedAt'))
    return int((completed - started).total_seconds()) if started and completed else None


def status(workflow, sha):
    runs = json.loads(gh('run', 'list', '--workflow', workflow, '--commit', sha, '--limit', '1',
                         '--json', 'databaseId,status,conclusion,url'))
    if not runs:
        return {'workflow': workflow, 'commit': sha, 'state': 'none'}
    return run_status(workflow, sha, runs[0])


def run_status(workflow, sha, run):
    state = ('running' if run['status'] != 'completed' else 'passed' if run['conclusion'] == 'success'
             else 'cancelled' if run['conclusion'] == 'cancelled' else 'failed')
    jobs = json.loads(gh('run', 'view', str(run['databaseId']), '--json', 'jobs'))['jobs']
    return {'workflow': workflow, 'commit': sha, 'state': state, 'run': run['databaseId'], 'url': run['url'],
            'jobs': [{'name': job['name'], 'id': job.get('databaseId'),
                      'state': job['conclusion'] or job['status'], 'seconds': seconds(job)}
                     for job in jobs]}


def successor(workflow, sha):
    """The newest run's head on main when it contains sha: the run that replaced sha's (latest wins)."""
    runs = json.loads(gh('run', 'list', '--workflow', workflow, '--branch', 'main', '--limit', '1', '--json', 'headSha'))
    head = runs[0]['headSha'] if runs else sha
    if head != sha and gh('api', f'repos/{{owner}}/{{repo}}/compare/{sha}...{head}', '-q', '.status').strip() == 'ahead':
        return head
    return None


def conflicting(sha):
    """The open PR whose head is sha and conflicts with main, if any: GitHub runs no pull_request
    workflow for it, so waiting for one is pointless."""
    for number in json.loads(gh('api', f'repos/{{owner}}/{{repo}}/commits/{sha}/pulls',
                                '-q', '[.[] | select(.state == "open") | .number]') or '[]'):
        if json.loads(gh('api', f'repos/{{owner}}/{{repo}}/pulls/{number}')).get('mergeable_state') == 'dirty':
            return number
    return None


def wait(workflow, sha, deadline, poll):
    """Wait for the run on sha to finish, reporting each job as its state changes. A run
    replaced by a newer push to main follows the newer run, which also covers sha."""
    seen, appear_by = {}, time.monotonic() + 300
    while True:
        result = status(workflow, sha)
        newer = result['state'] == 'cancelled' and successor(workflow, sha)
        if newer:
            say(f'{workflow} {sha[:7]}: replaced by the run on {newer[:7]}, which contains it')
            sha, seen = newer, {}
            continue
        for job in result.get('jobs', []):
            if seen.get(job['name']) != job['state']:
                seen[job['name']] = job['state']
                took = f" in {job['seconds']}s" if job['seconds'] is not None else ''
                say(f"{workflow} {sha[:7]} {job['name']}: {job['state']}{took}")
        if result['state'] not in ('running', 'none'):
            return result
        if result['state'] == 'none' and workflow != 'merge.yml' and (number := conflicting(sha)):
            say(f'PR #{number} conflicts with main: rebase it; no {workflow} run starts until then')
            return {**result, 'state': 'conflicts', 'pr': number, 'pr_state': 'open'}
        if result['state'] == 'none' and time.monotonic() > appear_by:
            return result
        if time.monotonic() > deadline:
            return {**result, 'timed_out': True}
        time.sleep(poll)


def annotations(job_id):
    """A job's annotations as 'title: message' lines: what failed, without its log."""
    rows = json.loads(gh('api', f'repos/{{owner}}/{{repo}}/check-runs/{job_id}/annotations?per_page=100') or '[]')
    return [f"{row['title']}: {row['message']}" if row.get('title') else row['message'] for row in rows]


def failure_annotations(result):
    """Each failed job's annotations, even while its siblings still run."""
    found = {}
    for job in result.get('jobs', []):
        if job['state'] == 'failure' and job.get('id'):
            try:
                found[job['name']] = annotations(job['id'])
            except subprocess.CalledProcessError as error:
                found[job['name']] = ['Annotations unavailable: ' + error.stderr.strip()]
    return {**result, 'annotations': found} if found else result


def phases(run, tails=None):
    """{phase: passed} from a run's `phases*` artifacts (each shard's loop.json), or None without any.
    A phase every shard runs (the builds) passes only if it passed everywhere. Failed phases'
    log tails are added to `tails` when given."""
    with tempfile.TemporaryDirectory() as directory:
        try:
            gh('run', 'download', str(run), '-p', 'phases*', '-D', directory)
        except subprocess.CalledProcessError:
            return None
        loops = sorted(Path(directory).rglob('loop.json'))
        if not loops:
            return None
        passed = {}
        for loop in loops:
            for row in json.loads(loop.read_text())['phases']:
                passed[row['phase']] = passed.get(row['phase'], True) and not row['failed']
                if tails is not None and row.get('log_tail'):
                    tails[row['phase']] = row['log_tail']
        return passed


def job_conclusions(run):
    return {job['name']: job['conclusion'] for job in json.loads(gh('run', 'view', str(run), '--json', 'jobs'))['jobs']}


def gate_regressions(job_id):
    """The PHPTs the gate job annotated as regressions (pass on upstream, fail here)."""
    prefix = REGRESSION + ': '
    return [line[len(prefix):].split(' (')[0] for line in annotations(job_id) if line.startswith(prefix)]


def phase_changes(result):
    """Failed phases, plus those new or fixed since the previous completed run on main that has phases."""
    tails = {}
    current = phases(result['run'], tails)
    if current is None:
        return result
    result = {**result, 'failed_phases': sorted(name for name, passed in current.items() if not passed)}
    runs = json.loads(gh('run', 'list', '--workflow', result['workflow'], '--branch', 'main', '--status', 'completed',
                         '--limit', '30', '--json', 'databaseId,conclusion,url'))
    for run in runs:
        previous = run['databaseId'] < result['run'] and run['conclusion'] in ('success', 'failure') and phases(run['databaseId'])
        if previous:
            # A phase the previous run didn't have is new too: it was added and fails on its first run.
            new = [name for name in result['failed_phases'] if previous.get(name) is not False]
            # A job with no phases of its own (phpt-gate) fails without any failed phase.
            # Like a phase, it's new unless it already failed in the previous run; the
            # full-gates shards are left out, since their phases speak for them.
            before = job_conclusions(run['databaseId'])
            for job in sorted((job for job in result.get('jobs', []) if job['state'] == 'failure'),
                              key=lambda job: job['name']):
                name = job['name']
                if name.split(' (')[0] != PHASES_JOB and before.get(name) != 'failure':
                    regressed = gate_regressions(job['id']) if name == GATE_JOB and job.get('id') else []
                    new.append(f"{name} job" + (f" ({', '.join(regressed)})" if regressed else ''))
            return {**result, 'previous_run': run['url'], 'new_failures': new,
                    'fixed': sorted(name for name, passed in current.items() if passed and previous.get(name) is False),
                    'phase_logs': {name: tails[name] for name in new if name in tails},
                    'issues': {name: filed(name) for name in new}}
    return result


SEARCH_TERMS = 5  # per failure: the search API allows 30 queries a minute


def search_terms(failure):
    """What identifies a new failure in an issue: its phase, its job or the PHPT gate's regressed tests."""
    job = re.fullmatch(r'(.+) job(?: \((.+)\))?', failure)
    return [test.removeprefix('tests/') for test in job.group(2).split(', ')] if job and job.group(2) else [
        job.group(1) if job else failure]


def filed(failure):
    """Open issues mentioning the first SEARCH_TERMS of a new failure's search_terms, so Red main's
    p0 is filed once; None when the search fails."""
    found = []
    try:
        for term in search_terms(failure)[:SEARCH_TERMS]:
            query = urllib.parse.quote(f'"{term}" is:issue is:open repo:', safe=':')
            found += [number for number in json.loads(gh('api', f'search/issues?q={query}{{owner}}/{{repo}}',
                                                         '--jq', '[.items[].number]')) if number not in found]
    except subprocess.CalledProcessError:
        return None
    return found


def completed_main():
    """Newest completed success/failure, skipping replaced/cancelled pending runs."""
    runs = json.loads(gh('run', 'list', '--workflow', 'merge.yml', '--branch', 'main', '--status', 'completed',
                         '--limit', '30', '--json', 'databaseId,conclusion,status,url,headSha'))
    for run in runs:
        if run.get('conclusion') in ('success', 'failure'):
            return diagnostics(run_status('merge.yml', run['headSha'], run))
    return None


def diagnostics(result):
    if result['state'] in ('passed', 'failed'):
        result = phase_changes(result)
    result = failure_annotations(result)
    if 'failed_phases' in result:
        for job in [job for job in result.get('annotations', {}) if job.split(' (')[0] == PHASES_JOB]:
            result['annotations'].pop(job)
    return result


def report(result, as_json):
    if as_json:
        print(json.dumps(result, indent=1))
        return
    if 'pr' in result:
        print(f"PR #{result['pr']}: {result['pr_state']}")
    line = f"{result['workflow']} {result['commit'][:7]}: {result['state']}"
    print(line + (' (timed out)' if result.get('timed_out') else '') + (f"  {result['url']}" if 'url' in result else ''))
    for job in result.get('jobs', []):
        print(f"  {job['name']}: {job['state']}" + (f" in {job['seconds']}s" if job['seconds'] is not None else ''))
    if 'failed_phases' in result:
        print(f"  failed phases ({len(result['failed_phases'])}): " + (', '.join(result['failed_phases']) or 'none'))
    if 'previous_run' in result:
        issues = result.get('issues', {})
        def known(name):
            numbers, terms = issues.get(name, []), len(search_terms(name))
            # A gate failure with more regressed tests than were searched may have an issue naming a later one.
            partial = f'; searched {SEARCH_TERMS} of {terms} tests' if terms > SEARCH_TERMS else ''
            return (' (issue search failed)' if numbers is None else
                    f" (open: {', '.join(f'#{n}' for n in numbers)}{partial})" if numbers else f' (no open issue{partial})')
        print(f"  since {result['previous_run']}: new failures: "
              f"{', '.join(name + known(name) for name in result['new_failures']) or 'none'}; "
              f"fixed: {', '.join(result['fixed']) or 'none'}")
    for job, lines in result.get('annotations', {}).items():
        print(f'--- {job} annotations', *lines, sep='\n')
    for name, lines in result.get('phase_logs', {}).items():
        print(f'--- new failure {name} (last {len(lines)} log lines)', *lines, sep='\n')
    if result.get('last_completed'):
        print('Latest completed run on main (separate from requested commit):')
        report(result['last_completed'], False)


WORKFLOW_SCOPE = re.compile(r'needs (?:a token with )?workflow scope')


def workflow_scope():
    """False only when gh's token is a classic OAuth token without the workflow scope; others
    (fine-grained, app) don't list scopes."""
    for line in gh('api', '-i', 'user').splitlines():
        name, _, value = line.partition(':')
        if name.lower() == 'x-oauth-scopes':
            return 'workflow' in {scope.strip() for scope in value.split(',')}
        if not line.strip():
            break
    return True


def claimable():
    """Open, ready, unblocked issues without needs:decision: p0, p1, p2, then unprioritised,
    oldest first within each (AGENTS.md's claim order)."""
    # REST, not `gh issue list`: GraphQL is unavailable in some agent sessions. Open blockers
    # count in issue_dependencies_summary.blocked_by; the endpoint also lists pull requests.
    issues = [issue for issue in map(json.loads, gh(
        'api', '--paginate', 'repos/{owner}/{repo}/issues?state=open&labels=status:ready&per_page=100',
        '--jq', '.[]').splitlines())
        if 'pull_request' not in issue and not issue.get('issue_dependencies_summary', {}).get('blocked_by')
        and not any(label['name'] == 'needs:decision' for label in issue['labels'])]
    # Editing .github/workflows/ needs a token with workflow scope; such issues say so in their title.
    if not workflow_scope():
        workflows = [issue for issue in issues if WORKFLOW_SCOPE.search(issue['title'])]
        if workflows:
            say(f"hiding {len(workflows)} issues that need a workflow-scoped token: "
                + ', '.join(f"#{issue['number']}" for issue in workflows))
        issues = [issue for issue in issues if issue not in workflows]
    def priority(issue):
        names = {label['name'] for label in issue['labels']}
        return next((rank for rank, name in enumerate(('p0', 'p1', 'p2')) if name in names), 3)
    return [{'number': issue['number'], 'priority': ('p0', 'p1', 'p2', '-')[priority(issue)],
             'title': issue['title'], 'created': issue['created_at']}
            for issue in sorted(issues, key=lambda issue: (priority(issue), issue['created_at']))]


# Upstream's PHPT results move only with what they depend on: the pin and its build, how the tests
# are listed and run, and the packages around them (and the PHP release: --php). A baseline's
# upstream report stands in for running upstream only while these paths are as at HEAD.
UPSTREAM_KEY = ('tools/build_upstream.py', 'tools/harness.py', 'tools/phpt_expectations.py', 'corpus/upstream/run.py',
                'corpus/upstream/inventory.py', '.devcontainer/install.sh', '.github/actions/install',
                '.github/actions/upstream')
# Per baseline: the artifact holding an upstream report, the report's path in it, the jobs that made
# the reports (all must exist, an outage can drop one, and have succeeded) and the paths of its own
# build and environment. `phpt` is the merge tier's six phpt.yml shards, whose upstream.so phpt-build
# builds through .github/actions/upstream like every variant; each variant is one job running both engines.
BASELINES = {
    'phpt': ('phpt-upstream-1', 'upstream-1.json', r'(phpt / )?phpt \(\d+, upstream\)', 6,
             ('.github/actions/phpt-build', '.github/actions/phpt-env', '.github/actions/instantclient',
              '.github/workflows/phpt.yml', 'corpus/upstream/services.yml', 'corpus/upstream/lib-composer.lock')),
    **{variant: (f'phpt-{variant}', f'target/phpt/{variant}/upstream.json', rf'{variant} / phpt', 1,
                 (f'.github/workflows/{variant}.yml', 'corpus/upstream/baseline.sh',
                  *(['.devcontainer/php.sh'] if variant == 'zts' else [])))
       for variant in ('cares', 'iouring', 'threadcontext', 'zts')},
}


def unchanged(sha, paths):
    """Whether `paths` are at sha as at HEAD; a shallow checkout fetches sha first."""
    if subprocess.run(['git', 'cat-file', '-e', f'{sha}^{{commit}}'], capture_output=True).returncode:
        subprocess.run(['git', 'fetch', '-q', '--depth', '1', 'origin', sha], check=True)
    return subprocess.run(['git', 'diff', '--quiet', sha, 'HEAD', '--', *paths]).returncode == 0


def phpt_baseline(kind='phpt', php=None):
    """(run, head sha) of main's newest merge-tier run whose upstream report for `kind` (BASELINES)
    still stands for upstream at HEAD: the jobs that made it all succeeded, its key paths are
    unchanged since, and with `php` it ran on that PHP_VERSION. Searched by artifact, not by run:
    a run that reused another's reports made none, and the last that made them can be days back."""
    artifact, report, jobs, count, paths = BASELINES[kind]
    rows = [json.loads(line) for line in gh('api', f'repos/{{owner}}/{{repo}}/actions/artifacts?name={artifact}&per_page=100',
                                            '--jq', '.artifacts[] | .workflow_run + {expired}').splitlines()]
    # Only the merge tier runs on a push to main; a fork's pull request can name its branch main.
    runs = dict.fromkeys((row['id'], row['head_sha']) for row in rows if not row['expired']
                         and row['head_branch'] == 'main' and row['head_repository_id'] == row['repository_id'])
    for run, sha in runs:
        shards = [conclusion for name, conclusion in job_conclusions(run).items() if re.fullmatch(jobs, name)]
        if shards.count('success') != count:
            continue
        # Runs further back are older: a key or PHP release that changed since stays changed.
        if not unchanged(sha, UPSTREAM_KEY + paths):
            return None
        if php:
            with tempfile.TemporaryDirectory() as directory:
                gh('run', 'download', str(run), '-n', artifact, '-D', directory)
                engines = json.loads((Path(directory) / report).read_text())['engines']
            if any(engine.get('php') != php for engine in engines):
                return None
        return run, sha
    return None


# The PR tier's aggregate check, the one main's ruleset requires; it needs every blocking job.
REQUIRED = ('required',)


def new_failures(loops):
    """The phases failed in `loops` (dev.py loop.json paths) that main's latest completed merge-tier
    run with phases didn't fail: a PR's gates block only on what it breaks, not on red main."""
    failed = set()
    for loop in loops:
        failed |= {row['phase'] for row in json.loads(Path(loop).read_text())['phases'] if row['failed']}
    runs = json.loads(gh('run', 'list', '--workflow', 'merge.yml', '--branch', 'main', '--status', 'completed',
                         '--limit', '30', '--json', 'databaseId,conclusion,url'))
    for run in runs:
        baseline = run['conclusion'] in ('success', 'failure') and phases(run['databaseId'])
        if baseline:
            return run['url'], sorted(name for name in failed if baseline.get(name) is not False)
    return None, sorted(failed)


def run_failures(needs, directory, shards):
    """What a merge-tier run failed, told from inside it without the API (whose hourly limit every
    job shares): `needs` is its last job's `toJSON(needs)` and `directory` holds its `phases*`
    artifacts. A job is judged like a phase. full-gates speaks through its phases; when it
    failed and fewer than `shards` of them reported any, a shard broke before its gates ran."""
    loops = sorted(Path(directory).rglob('loop.json'))
    reported = {loop.relative_to(directory).parts[0] for loop in loops}
    return {'phases': sorted({row['phase'] for loop in loops for row in json.loads(loop.read_text())['phases'] if row['failed']}),
            'jobs': sorted(job for job, state in needs.items()
                           if job != PHASES_JOB and state['result'] not in ('success', 'skipped')),
            'unreported': (needs.get(PHASES_JOB, {}).get('result') == 'failure'
                           and sum(bool(re.fullmatch(r'phases-\d+', name)) for name in reported) < shards)}


def against(current, baseline):
    """The failures of `current` that `baseline` doesn't share (both run_failures): the merge
    queue lands a group only if it fails nothing but what main's latest completed merge-tier
    run already failed. No baseline excuses a full-gates shard that reported nothing."""
    return ([name for name in current['phases'] if name not in baseline['phases']]
            + [f'{job} job' for job in current['jobs'] if job not in baseline['jobs']]
            + ([f'{PHASES_JOB} job (a shard reported no phases)'] if current['unreported'] else []))


# Open PRs with their merge-queue state: one call, where the REST list has none of it.
QUEUE_QUERY = '''query($owner: String!, $repo: String!) { repository(owner: $owner, name: $repo) {
  pullRequests(states: OPEN, first: 100) { nodes {
    number title createdAt isDraft isInMergeQueue autoMergeRequest { enabledAt }
    timelineItems(last: 1, itemTypes: [ADDED_TO_MERGE_QUEUE_EVENT, REMOVED_FROM_MERGE_QUEUE_EVENT]) { nodes {
      __typename ... on RemovedFromMergeQueueEvent { reason } } } } } } }'''


def dequeued():
    """Open PRs the merge queue dropped: their group failed `required`, and GitHub turned their
    auto-merge off with it, so nothing merges them until someone fixes and queues them again.
    Each with the reason GitHub gives and its group's merge-tier run."""
    pulls = json.loads(gh('api', 'graphql', '-F', 'owner={owner}', '-F', 'repo={repo}', '-f', 'query=' + QUEUE_QUERY,
                          '--jq', '.data.repository.pullRequests.nodes'))
    rows = [{'number': pull['number'], 'title': pull['title'], 'created': pull['createdAt'],
             'blocking': ['left the merge queue: ' + (pull['timelineItems']['nodes'][0].get('reason') or 'no reason given')]}
            for pull in pulls
            if not (pull['isDraft'] or pull['isInMergeQueue'] or pull['autoMergeRequest'])
            and [event['__typename'] for event in pull['timelineItems']['nodes']] == ['RemovedFromMergeQueueEvent']]
    if rows:
        # A group's branch is gh-readonly-queue/main/pr-<number>-<base sha>.
        runs = json.loads(gh('run', 'list', '--workflow', 'merge.yml', '--event', 'merge_group', '--limit', '100',
                             '--json', 'headBranch,url'))
        for row in rows:
            run = next((run['url'] for run in runs if f"/pr-{row['number']}-" in run.get('headBranch', '')), None)
            row['blocking'] += [run] if run else []
    return rows


def stuck():
    """Open auto-merge PRs that won't merge by themselves: conflicting with main, with a failed
    required check on their head, or with a finished PR-tier run that never created one. And
    PRs the merge queue dropped (dequeued). Oldest first, each with what blocks it."""
    pulls = [pull for pull in map(json.loads, gh(
        'api', '--paginate', 'repos/{owner}/{repo}/pulls?state=open&per_page=100', '--jq', '.[]').splitlines())
        if pull.get('auto_merge')]
    rows = []
    for pull in sorted(pulls, key=lambda pull: pull['created_at']):
        # mergeable_state is only on the single-PR endpoint; 'dirty' means conflicts.
        detail = json.loads(gh('api', f"repos/{{owner}}/{{repo}}/pulls/{pull['number']}"))
        sha = pull['head']['sha']
        checks = [json.loads(line) for line in gh('api', f"repos/{{owner}}/{{repo}}/commits/{sha}/check-runs?per_page=100",
                                                  '--jq', '.check_runs[]').splitlines()]
        blocking = (['conflicts'] if detail.get('mergeable_state') == 'dirty' else []) + sorted(
            {check['name'] for check in checks
             if check['name'] in REQUIRED and check.get('conclusion') in ('failure', 'timed_out', 'cancelled')})
        # A PR-tier run GitHub abandoned finishes without creating its jobs, `required` among them,
        # so nothing fails and the PR waits forever; re-running it is the fix.
        if not blocking and not any(check['name'] in REQUIRED for check in checks):
            runs = json.loads(gh('run', 'list', '--workflow', 'pr.yml', '--commit', sha, '--limit', '1',
                                 '--json', 'status,conclusion'))
            if runs and runs[0]['status'] == 'completed':
                blocking = ['no required check']
        if blocking:
            rows.append({'number': pull['number'], 'title': pull['title'], 'created': pull['created_at'],
                         'blocking': blocking})
    return sorted(rows + dequeued(), key=lambda row: row['created'])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    for name in ['status', 'wait']:
        command = commands.add_parser(name)
        command.add_argument('--commit', help='sha or branch (default: main)')
        command.add_argument('--workflow', default='merge.yml')
        command.add_argument('--timeout', type=int, default=3600, help='seconds (default 3600)')
        command.add_argument('--poll', type=float, default=15, help=argparse.SUPPRESS)
        command.add_argument('--json', action='store_true')
    for name in ['next', 'stuck']:
        commands.add_parser(name).add_argument('--json', action='store_true')
    baseline = commands.add_parser('phpt-baseline')
    baseline.add_argument('--variant', choices=[kind for kind in BASELINES if kind != 'phpt'], default='phpt',
                          help="a variant workflow's upstream report (default: phpt.yml's shards)")
    baseline.add_argument('--php', help="the PHP_VERSION the baseline's upstream must have run on")
    failures = commands.add_parser('new-failures')
    failures.add_argument('loops', nargs='*', type=Path, metavar='LOOP_JSON')
    failures.add_argument('--needs', help="the run's toJSON(needs), in place of loop.json reports")
    failures.add_argument('--phases', type=Path, help="with --needs: the directory of the run's phases* artifacts")
    failures.add_argument('--shards', type=int, default=3, help="with --needs: how many shards full-gates has")
    failures.add_argument('--save', type=Path, help='with --needs: write what the run failed here, as a baseline')
    failures.add_argument('--baseline', type=Path, help='with --needs: fail on what this baseline did not fail')
    args = parser.parse_args(argv)
    global RETRY_DEADLINE, RETRY_BASE
    if args.command == 'phpt-baseline':
        try:
            baseline = phpt_baseline(args.variant, args.php)
        except subprocess.CalledProcessError as failure:
            sys.exit(f"gh {' '.join(failure.cmd[1:])}: {failure.stderr.strip()}")
        if baseline:
            print(*baseline)
        return 0 if baseline else 2
    if args.command == 'new-failures':
        if args.needs:
            current = run_failures(json.loads(args.needs), args.phases, args.shards)
            if args.save:
                args.save.parent.mkdir(parents=True, exist_ok=True)
                args.save.write_text(json.dumps({**current, 'run': os.environ.get('GITHUB_RUN_ID')}))
            if not args.baseline:
                print(f"Failed: {', '.join(current['phases'] + current['jobs']) or 'nothing'}")
                return 0
            if not args.baseline.exists():
                sys.exit('new-failures: no baseline; no merge-tier run on main has saved one yet')
            baseline = json.loads(args.baseline.read_text())
            new = against(current, baseline)
            print(f"Against main's merge-tier run {baseline.get('run')}: " +
                  (f"new failures: {', '.join(new)}" if new else 'no new failures; whatever failed fails on main too'))
            return int(bool(new))
        if not args.loops:
            sys.exit('new-failures: no loop.json given; the gates ran nothing')
        try:
            baseline, new = new_failures(args.loops)
        except subprocess.CalledProcessError as failure:
            sys.exit(f"gh {' '.join(failure.cmd[1:])}: {failure.stderr.strip()}")
        print(f"Against {baseline or 'no merge-tier run with phases'}: " +
              (f"new failures: {', '.join(new)}" if new else 'no new failures; whatever failed fails on main too'))
        return int(bool(new))
    if args.command in ('next', 'stuck'):
        try:
            queue = claimable() if args.command == 'next' else stuck()
        except subprocess.CalledProcessError as failure:
            sys.exit(f"gh {' '.join(failure.cmd[1:])}: {failure.stderr.strip()}")
        print(json.dumps(queue, indent=1) if args.json else '\n'.join(
            f"#{row['number']} {row.get('priority') or ', '.join(row['blocking'])} {row['title']}" for row in queue))
        return 0 if queue else 2
    deadline = time.monotonic() + args.timeout
    RETRY_DEADLINE, RETRY_BASE = deadline, min(RETRY_BASE, args.poll) if args.poll else 0.0
    try:
        sha = resolve(args.commit)
        result = status(args.workflow, sha) if args.command == 'status' else wait(args.workflow, sha, deadline, args.poll)
        result = diagnostics(result)
        if args.command == 'status' and args.workflow == 'merge.yml' and result['state'] not in ('passed', 'failed'):
            result['last_completed'] = completed_main()
    except subprocess.CalledProcessError as failure:
        sys.exit(f"gh {' '.join(failure.cmd[1:])}: {failure.stderr.strip()}")
    report(result, args.json)
    return 2 if result.get('timed_out') else EXIT[result['state']]


if __name__ == '__main__':
    sys.exit(main())
