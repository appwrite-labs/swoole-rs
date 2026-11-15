#!/usr/bin/env python3
"""Watch this repository's CI. GitHub merges pull requests itself: `gh pr merge --auto --squash`
puts a PR in main's merge queue once its PR tier passes.

  ci.py wait [--commit SHA]  wait for a workflow run (default: the merge tier on main, which runs
                             hourly); a run replaced by a newer one on main follows the newer run
  ci.py status [--commit SHA]  the same, without waiting
  ci.py next                 the claimable issues in claim order (exit 2 when there are none),
                             without those whose title says they need workflow scope when gh's
                             token lacks it
  ci.py stuck                auto-merge PRs that conflict with main, failed a required check, have
                             a review requesting changes (answer it), or whose finished PR-tier run has none (GitHub abandoned its jobs:
                             re-run it), and PRs the merge queue dropped (fix what their run
                             broke, then queue them again), oldest first (exit 2 when there are none);
                             first, a merge-tier run on main that a job waiting for a runner holds
                             open (cancel it: every later hourly run waits behind it)
  ci.py phpt-baseline [--variant NAME] [--php VERSION] [--saved DIR]
                             `RUN SHA` of main's newest merge-tier run whose upstream PHPT reports
                             still stand for upstream at HEAD, so it needn't run (exit 2: none)

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
from datetime import datetime, timedelta, timezone
from pathlib import Path
import json
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
            bucket = ('search' if len(args) > 1 and args[1].startswith('search/')  # their own limits
                      else 'graphql' if 'graphql' in args else 'core')
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
    rev = commit or 'main'
    found = graphql(RESOLVE_QUERY, rev=rev)[0]['object']
    if found:
        return found['oid']
    # Auto-merge deletes a PR's branch: resolve it through that PR's head.
    head = gh('api', f'repos/{{owner}}/{{repo}}/pulls?state=all&head={{owner}}:{rev}',
              '-q', '.[0].head.sha // ""').strip()
    if not head:
        raise subprocess.CalledProcessError(1, ['gh', 'resolve', rev], '', f'no commit or PR branch named {rev}')
    return head


def seconds(job):
    # GitHub reports unfinished timestamps as 0001-01-01T00:00:00Z.
    started, completed = (datetime.fromisoformat(job[key].replace('Z', '+00:00')) if job.get(key) and not job[key].startswith('0001')
                          else None for key in ('startedAt', 'completedAt'))
    return int((completed - started).total_seconds()) if started and completed else None


def status(workflow, sha, run=None):
    """The newest `workflow` run on sha (or run `run`), with its jobs and their annotations: one
    GraphQL query over the commit's check suites, where REST took a run list, a job list and a call
    per failed job's annotations."""
    commit = graphql(RUNS_QUERY, rev=sha)[0]['object'] or {'checkSuites': {'nodes': []}}
    suites = [suite for suite in commit['checkSuites']['nodes'] if suite['workflowRun']
              and suite['workflowRun']['workflow']['resourcePath'].endswith('/' + workflow)
              and run in (None, suite['workflowRun']['databaseId'])]
    if not suites:
        return {'workflow': workflow, 'commit': sha, 'state': 'none'}
    suite = max(suites, key=lambda suite: suite['workflowRun']['databaseId'])
    state = ('running' if suite['status'] != 'COMPLETED' else 'passed' if suite['conclusion'] == 'SUCCESS'
             else 'cancelled' if suite['conclusion'] == 'CANCELLED' else 'failed')
    return {'workflow': workflow, 'commit': sha, 'state': state, 'run': suite['workflowRun']['databaseId'],
            'url': suite['workflowRun']['url'],
            'jobs': [{'name': job['name'], 'id': job['databaseId'], 'state': (job['conclusion'] or job['status']).lower(),
                      'seconds': seconds(job),
                      'annotations': [f"{row['title']}: {row['message']}" if row.get('title') else row['message']
                                      for row in job['annotations']['nodes']]}
                     for job in suite['checkRuns']['nodes']]}


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
    replaced by a newer run on main follows the newer run, which also covers sha. The merge tier
    runs on main hourly, so its run may take up to an hour to appear."""
    seen, appear_by = {}, time.monotonic() + (4500 if workflow == 'merge.yml' else 300)
    while True:
        result = status(workflow, sha)
        # An hourly run on main may start after a later merge, so sha may never get one of its own.
        newer = (result['state'] == 'cancelled' or (result['state'] == 'none' and workflow == 'merge.yml')) \
            and successor(workflow, sha)
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


def failure_annotations(result):
    """Each failed job's annotations, even while its siblings still run."""
    found = {job['name']: job['annotations'] for job in result.get('jobs', []) if job['state'] == 'failure'}
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


def completed(workflow):
    """main's 30 newest completed `workflow` runs, newest first, listed once per invocation."""
    if workflow not in COMPLETED:
        COMPLETED[workflow] = json.loads(gh('run', 'list', '--workflow', workflow, '--branch', 'main', '--status',
                                            'completed', '--limit', '30', '--json', 'databaseId,conclusion,url,headSha'))
    return COMPLETED[workflow]


COMPLETED = {}


def job_conclusions(run):
    return {job['name']: job['conclusion'] for job in json.loads(gh('run', 'view', str(run), '--json', 'jobs'))['jobs']}


def gate_regressions(job):
    """The PHPTs the gate job annotated as regressions (pass on upstream, fail here)."""
    prefix = REGRESSION + ': '
    return [line[len(prefix):].split(' (')[0] for line in job['annotations'] if line.startswith(prefix)]


def phase_changes(result):
    """Failed phases, plus those new or fixed since the previous completed run on main that has phases."""
    tails = {}
    current = phases(result['run'], tails)
    if current is None:
        return result
    result = {**result, 'failed_phases': sorted(name for name, passed in current.items() if not passed)}
    for run in completed(result['workflow']):
        previous = run['databaseId'] < result['run'] and run['conclusion'] in ('success', 'failure') and phases(run['databaseId'])
        if previous:
            # A phase the previous run didn't have is new too: it was added and fails on its first run.
            new = [name for name in result['failed_phases'] if previous.get(name) is not False]
            # A job with no phases of its own (phpt-gate) fails without any failed phase.
            # Like a phase, it's new unless it already failed in the previous run; the
            # full-gates shards are left out, since their phases speak for them.
            before = {job['name']: job['state']
                      for job in status(result['workflow'], run['headSha'], run['databaseId']).get('jobs', [])}
            for job in sorted((job for job in result.get('jobs', []) if job['state'] == 'failure'),
                              key=lambda job: job['name']):
                name = job['name']
                if name.split(' (')[0] != PHASES_JOB and before.get(name) != 'failure':
                    regressed = gate_regressions(job) if name == GATE_JOB else []
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
    """Open non-goal issues (a goal's history names old failures) mentioning the first SEARCH_TERMS
    of a new failure's search_terms, so Red main's p0 is filed once; None when the search fails."""
    found = []
    try:
        for term in search_terms(failure)[:SEARCH_TERMS]:
            query = urllib.parse.quote(f'"{term}" is:issue is:open -label:kind:goal repo:', safe=':')
            found += [number for number in json.loads(gh('api', f'search/issues?q={query}{{owner}}/{{repo}}',
                                                         '--jq', '[.items[].number]')) if number not in found]
    except subprocess.CalledProcessError:
        return None
    return found


def completed_main():
    """Newest completed success/failure, skipping replaced/cancelled pending runs."""
    for run in completed('merge.yml'):
        if run.get('conclusion') in ('success', 'failure'):
            return diagnostics(status('merge.yml', run['headSha'], run['databaseId']))
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


def graphql(query, **variables):
    """A query over this repository and the token's scopes: (data, scopes, or None when the
    token is fine-grained or an app's and lists none). GraphQL has its own budget, which the
    REST one every agent shares can't spare (#2508); -i adds the response headers."""
    text = gh('api', '-i', 'graphql', '-F', 'owner={owner}', '-F', 'repo={repo}',
              *(arg for name, value in variables.items() for arg in ('-f', f'{name}={value}')), '-f', 'query=' + query)
    head, body = re.split(r'\r?\n\r?\n', text, maxsplit=1)
    scopes = next((value for name, _, value in (line.partition(':') for line in head.splitlines())
                   if name.lower() == 'x-oauth-scopes'), None)
    return json.loads(body)['data']['repository'], scopes


RESOLVE_QUERY = '''query($owner: String!, $repo: String!, $rev: String!) { repository(owner: $owner, name: $repo) {
  object(expression: $rev) { oid } } }'''
# A commit's workflow runs (check suites), each with its jobs and their annotations.
RUNS_QUERY = '''query($owner: String!, $repo: String!, $rev: String!) { repository(owner: $owner, name: $repo) {
  object(expression: $rev) { ... on Commit { checkSuites(first: 20) { nodes {
    status conclusion workflowRun { databaseId url workflow { resourcePath } }
    checkRuns(first: 100) { nodes { name databaseId status conclusion startedAt completedAt
      annotations(first: 50) { nodes { title message } } } } } } } } } }'''
READY_QUERY = '''query($owner: String!, $repo: String!) { repository(owner: $owner, name: $repo) {
  issues(states: OPEN, labels: ["status:ready"], first: 100) { nodes {
    number title createdAt labels(first: 20) { nodes { name } } issueDependenciesSummary { blockedBy } } } } }'''


def claimable():
    """Open, ready, unblocked issues without needs:decision: p0, p1, p2, then unprioritised,
    oldest first within each (AGENTS.md's claim order)."""
    data, scopes = graphql(READY_QUERY)
    issues = [{**issue, 'labels': {label['name'] for label in issue['labels']['nodes']}}
              for issue in data['issues']['nodes'] if not issue['issueDependenciesSummary']['blockedBy']]
    issues = [issue for issue in issues if 'needs:decision' not in issue['labels']]
    # Editing .github/workflows/ needs a token with workflow scope; such issues say so in their title.
    if scopes is not None and 'workflow' not in {scope.strip() for scope in scopes.split(',')}:
        workflows = [issue for issue in issues if WORKFLOW_SCOPE.search(issue['title'])]
        if workflows:
            say(f"hiding {len(workflows)} issues that need a workflow-scoped token: "
                + ', '.join(f"#{issue['number']}" for issue in workflows))
        issues = [issue for issue in issues if issue not in workflows]
    def priority(issue):
        return next((rank for rank, name in enumerate(('p0', 'p1', 'p2')) if name in issue['labels']), 3)
    return [{'number': issue['number'], 'priority': ('p0', 'p1', 'p2', '-')[priority(issue)],
             'title': issue['title'], 'created': issue['createdAt']}
            for issue in sorted(issues, key=lambda issue: (priority(issue), issue['createdAt']))]


# Upstream's PHPT results move only with what they depend on: the pin and its build, how the tests
# are listed and run, and the packages around them (and the PHP release: --php). A baseline's
# upstream report stands in for running upstream only while these paths are as at HEAD.
UPSTREAM_KEY = ('tools/build_upstream.py', 'tools/harness.py', 'tools/phpt_expectations.py', 'corpus/upstream/run.py',
                'corpus/upstream/inventory.py', '.devcontainer/install.sh', '.github/actions/install',
                '.github/actions/upstream')
# Per baseline: the artifact holding an upstream report, the report's path in it, the jobs that made
# the reports (all must exist, an outage can drop one, and have succeeded) and the paths of its own
# build and environment. `phpt` is the merge tier's phpt.yml shards, as many as its `shards` input's
# default (phpt.yml is part of the key, so a baseline's run had as many), whose upstream.so phpt-build
# builds through .github/actions/upstream like every variant; each variant is one job running both engines.
PHPT_SHARDS = len(json.loads(re.search(r"default: '(\[[\d, ]+\])'", (
    Path(__file__).resolve().parents[1] / '.github/workflows/phpt.yml').read_text())[1]))
BASELINES = {
    'phpt': ('phpt-upstream-1', 'upstream-1.json', r'phpt / upstream \d+', PHPT_SHARDS,
             ('.github/actions/phpt-build', '.github/actions/phpt-env', '.github/actions/instantclient',
              '.github/workflows/phpt.yml', 'corpus/upstream/services.yml', 'corpus/upstream/lib-composer.lock')),
    **{variant: (f'phpt-{variant}', 'results/upstream.json', rf'{variant} / phpt', 1,
                 ('.github/workflows/variant.yml', 'corpus/upstream/baseline.sh', 'corpus/upstream/families.sh',
                  *(['.devcontainer/php.sh'] if variant == 'zts' else [])))
       for variant in ('cares', 'iouring', 'threadcontext', 'zts')},
}


def unchanged(sha, paths):
    """Whether `paths` are at sha as at HEAD; a shallow checkout fetches sha first."""
    if subprocess.run(['git', 'cat-file', '-e', f'{sha}^{{commit}}'], capture_output=True).returncode:
        subprocess.run(['git', 'fetch', '-q', '--depth', '1', 'origin', sha], check=True)
    return subprocess.run(['git', 'diff', '--quiet', sha, 'HEAD', '--', *paths]).returncode == 0


def saved_baseline(directory, php=None):
    """phpt_baseline('phpt', php) from the cache main's PHPT gate saves (phpt.yml), with no API call:
    meta.json names the run and its commit, and the upstream reports beside it the PHP they ran on."""
    saved = json.loads((directory / 'meta.json').read_text())
    if not unchanged(saved['sha'], UPSTREAM_KEY + BASELINES['phpt'][4]):
        return None
    engines = json.loads((directory / BASELINES['phpt'][1]).read_text())['engines']
    if php and any(engine.get('php') != php for engine in engines):
        return None
    return saved['run'], saved['sha']


def phpt_baseline(kind='phpt', php=None):
    """(run, head sha) of main's newest merge-tier run whose upstream report for `kind` (BASELINES)
    still stands for upstream at HEAD: the jobs that made it all succeeded, its key paths are
    unchanged since, and with `php` it ran on that PHP_VERSION. Searched by artifact, not by run:
    a run that reused another's reports made none, and the last that made them can be days back."""
    artifact, report, jobs, count, paths = BASELINES[kind]
    rows = [json.loads(line) for line in gh('api', f'repos/{{owner}}/{{repo}}/actions/artifacts?name={artifact}&per_page=100',
                                            '--jq', '.artifacts[] | .workflow_run + {expired}').splitlines()]
    # Only the merge tier runs on main; a fork's pull request can name its branch main.
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
REQUIRED = 'required'


# Open PRs with their merge-queue state, mergeability and head's `required` check: one query,
# where REST took three calls a PR and has no queue state at all.
PULLS_QUERY = '''query($owner: String!, $repo: String!) { repository(owner: $owner, name: $repo) {
  pullRequests(states: OPEN, first: 100) { nodes {
    number title createdAt isDraft isInMergeQueue mergeable reviewDecision autoMergeRequest { enabledAt }
    timelineItems(last: 1, itemTypes: [ADDED_TO_MERGE_QUEUE_EVENT, REMOVED_FROM_MERGE_QUEUE_EVENT]) { nodes {
      __typename ... on RemovedFromMergeQueueEvent { reason } } }
    commits(last: 1) { nodes { commit { checkSuites(first: 50) { nodes {
      status workflowRun { workflow { name } }
      checkRuns(first: 1, filterBy: { checkName: "%s" }) { nodes { conclusion } } } } } } } } } } }''' % REQUIRED
# The PR tier's workflow name (pr.yml).
PR_WORKFLOW = 'PR'


def blocked(pull):
    """What keeps an auto-merge PR from merging by itself: conflicts with main, a failed required
    check on its head, a review requesting changes, or a finished PR-tier run that never created one."""
    suites = [suite for commit in pull['commits']['nodes'] for suite in commit['commit']['checkSuites']['nodes']]
    required = [run['conclusion'] for suite in suites for run in suite['checkRuns']['nodes']]
    blocking = (['conflicts'] if pull['mergeable'] == 'CONFLICTING' else []) + (
        [REQUIRED] if any(conclusion in ('FAILURE', 'TIMED_OUT', 'CANCELLED') for conclusion in required) else []) + (
        ['changes requested'] if pull.get('reviewDecision') == 'CHANGES_REQUESTED' else [])
    # A PR-tier run GitHub abandoned finishes without creating its jobs, `required` among them,
    # so nothing fails and the PR waits forever; re-running it is the fix.
    pr_tier = [suite['status'] for suite in suites
               if ((suite['workflowRun'] or {}).get('workflow') or {}).get('name') == PR_WORKFLOW]
    if not blocking and not required and pr_tier and all(status == 'COMPLETED' for status in pr_tier):
        blocking = ['no required check']
    return blocking


def dequeued(pull):
    """Whether the merge queue dropped the PR: its group failed `required`, and GitHub turned its
    auto-merge off with it, so nothing merges it until someone fixes and queues it again."""
    return (not (pull['isDraft'] or pull['isInMergeQueue'] or pull['autoMergeRequest'])
            and [event['__typename'] for event in pull['timelineItems']['nodes']] == ['RemovedFromMergeQueueEvent'])


def stuck():
    """Open PRs that won't merge by themselves: auto-merge ones that are blocked, and those the
    merge queue dropped, with the reason GitHub gives and their group's merge-tier run. Oldest
    first, each with what blocks it."""
    rows = []
    for pull in graphql(PULLS_QUERY)[0]['pullRequests']['nodes']:
        if pull['autoMergeRequest']:
            blocking = blocked(pull)
        elif dequeued(pull):
            blocking = ['left the merge queue: ' + (pull['timelineItems']['nodes'][0].get('reason') or 'no reason given')]
        else:
            continue
        if blocking:
            rows.append({'number': pull['number'], 'title': pull['title'], 'created': pull['createdAt'],
                         'blocking': blocking, 'dequeued': not pull['autoMergeRequest']})
    if any(row['dequeued'] for row in rows):
        # A group's branch is gh-readonly-queue/main/pr-<number>-<base sha>; only GitHub's run list has it.
        runs = json.loads(gh('run', 'list', '--workflow', 'merge.yml', '--event', 'merge_group', '--limit', '100',
                             '--json', 'headBranch,url'))
        for row in (row for row in rows if row['dequeued']):
            run = next((run['url'] for run in runs if f"/pr-{row['number']}-" in run.get('headBranch', '')), None)
            row['blocking'] += [run] if run else []
    return sorted(({key: value for key, value in row.items() if key != 'dequeued'} for row in rows),
                  key=lambda row: row['created'])


# A merge-tier run is done well inside this; one still waiting for a runner after it is stuck.
STALL_MINUTES = 45


def stalled_main():
    """A merge-tier run on main that a job waiting for a runner keeps unfinished (#3553). It holds
    main's hourly concurrency group, so each later hourly run queues a `pending` run the next one
    replaces, and no merge-tier run on main completes. A job's timeout-minutes only counts once a
    runner takes it. Only the newest unfinished run older than a pending one holds the group: GitHub
    leaves an abandoned run unfinished too, and it blocks nothing."""
    # By status: the hourly runs behind a stalled run each leave a cancelled one, which would bury it in
    # a plain window of the newest runs (34 in five hours on 2026-11-12).
    runs = {status: json.loads(gh('run', 'list', '--workflow', 'merge.yml', '--branch', 'main', '--event', 'schedule',
                                  '--status', status, '--limit', '5', '--json', 'databaseId,status,createdAt,url'))
            for status in ('pending', 'queued', 'in_progress')}
    # GitHub fails a job that has waited a day for a runner, so an unfinished run older than that is
    # one it abandoned, in a concurrency group no run uses any more.
    recent = (datetime.now(timezone.utc) - timedelta(days=1)).strftime('%Y-%m-%dT%H:%M:%SZ')
    runs = {status: [run for run in rows if run['createdAt'] > recent] for status, rows in runs.items()}
    if not runs['pending']:
        return None
    waiting = min(run['createdAt'] for run in runs['pending'])
    holders = [run for run in runs['queued'] + runs['in_progress'] if run['createdAt'] < waiting]
    if not holders:
        return None
    run = max(holders, key=lambda run: run['createdAt'])
    age = datetime.now(timezone.utc) - datetime.fromisoformat(run['createdAt'].replace('Z', '+00:00'))
    minutes = int(age.total_seconds() // 60)
    if minutes < STALL_MINUTES:
        return None
    jobs = json.loads(gh('run', 'view', str(run['databaseId']), '--json', 'jobs'))['jobs']
    queued = [job['name'] for job in jobs if job['status'] in ('queued', 'waiting', 'pending')]
    return {'run': run['databaseId'], 'url': run['url'], 'minutes': minutes, 'waiting': queued} if queued else None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    for name in ['status', 'wait']:
        command = commands.add_parser(name)
        command.add_argument('--commit', help='sha or branch (default: main)')
        command.add_argument('--workflow', default='merge.yml')
        command.add_argument('--timeout', type=int, help='seconds (default '
                             + ('6300 for merge.yml, else 3600)' if name == 'wait' else '3600)'))
        command.add_argument('--poll', type=float, default=15, help=argparse.SUPPRESS)
        command.add_argument('--json', action='store_true')
    for name in ['next', 'stuck']:
        commands.add_parser(name).add_argument('--json', action='store_true')
    baseline = commands.add_parser('phpt-baseline')
    baseline.add_argument('--variant', choices=[kind for kind in BASELINES if kind != 'phpt'], default='phpt',
                          help="a variant workflow's upstream report (default: phpt.yml's shards)")
    baseline.add_argument('--php', help="the PHP_VERSION the baseline's upstream must have run on")
    baseline.add_argument('--saved', type=Path, help="the restored phpt-baseline cache: answer from it, with no API "
                          "call, when it has a meta.json")
    args = parser.parse_args(argv)
    global RETRY_DEADLINE, RETRY_BASE
    if args.command == 'phpt-baseline':
        try:
            saved = args.variant == 'phpt' and args.saved and (args.saved / 'meta.json').exists()
            baseline = saved_baseline(args.saved, args.php) if saved else phpt_baseline(args.variant, args.php)
        except subprocess.CalledProcessError as failure:
            sys.exit(f"gh {' '.join(failure.cmd[1:])}: {failure.stderr.strip()}")
        if baseline:
            print(*baseline)
        return 0 if baseline else 2
    if args.command in ('next', 'stuck'):
        try:
            queue = claimable() if args.command == 'next' else stuck()
            stall = stalled_main() if args.command == 'stuck' else None
        except subprocess.CalledProcessError as failure:
            sys.exit(f"gh {' '.join(failure.cmd[1:])}: {failure.stderr.strip()}")
        if stall and not args.json:
            print(f"main's merge tier has waited {stall['minutes']} min for a runner for "
                  f"{', '.join(stall['waiting'])}; cancel it so main's next run starts: "
                  f"gh run cancel {stall['run']}  {stall['url']}")
        print(json.dumps({'stalled_main': stall, 'pulls': queue} if stall else queue, indent=1) if args.json else
              '\n'.join(f"#{row['number']} {row.get('priority') or ', '.join(row['blocking'])} {row['title']}"
                        for row in queue))
        return 0 if queue or stall else 2
    # An hourly run on main can take 75 minutes to appear, then 30 to finish.
    timeout = args.timeout if args.timeout is not None else 6300 if args.command == 'wait' and args.workflow == 'merge.yml' else 3600
    deadline = time.monotonic() + timeout
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
