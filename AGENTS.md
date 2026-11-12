# Working rules

Complexity is the devil, and simplicity is human. Before starting, consider what
can be deleted. Aim for a net negative LoC delta where the task permits it.

Never edit target/upstream/src. It is the pinned reference, built only as the
oracle; swoole.so builds without it.

Preserve upstream test assertions and expected outputs. Never mark unsupported,
SKIP or XFAIL as passed, or weaken capability probes to hide behavior failures.
When an upstream GoogleTest is all that keeps a C++ file in the build, port it to
Rust (one Rust test per TEST, same inputs and expected values, mapped in the
module's tests README) and stop building it.
Run original PHPT tests with PHP's official runner through corpus/upstream/run.py,
not upstream's tests/start.sh: its cleanup can kill unrelated processes.
Keep module tests under tests/modules/<module>/.

A new dependency needs a sentence in the PR on why the standard library or an existing
dependency won't do. The PR tier checks dependencies with `cargo deny` (`deny.toml`:
advisories, licences, duplicate versions, no async runtime) and `cargo machete` (unused).

Rust lint levels live only in the root `Cargo.toml` (`[workspace.lints]`) and
`clippy.toml`. Never lower one in a PR that also changes code; lowering one needs
`needs:decision`. Suppress with `#[expect(lint, reason = "...")]`, never `#[allow]`,
and give a reason a reviewer can judge. Deny a lint once its count reaches zero.
The one per-crate exception: each `lib.rs` denies `undocumented_unsafe_blocks` and
`multiple_unsafe_ops_per_block` under `cfg_attr(not(test), ...)`. Every non-test
unsafe block states its invariant in a `// SAFETY:` comment and does one operation;
test code is exempt. A crate with no unsafe left adds `#![forbid(unsafe_code)]` to its
`lib.rs` (Cargo's `[lints] workspace = true` takes no per-crate additions), and
`tools/budget.py` fails a PR that removes it.

Benchmarks use optimized builds and run without concurrent build or test load.
Build, profile and benchmark output goes under the worktree's `target/` (a second
checkout is `git worktree add target/worktrees/<sha>`), never in a session scratch
directory; `dev.py prune` clears what a day leaves untouched.
See corpus/README.md, corpus/upstream/README.md and benchmarks/README.md.

# Agents

Three roles. Coordination state lives in GitHub issues and CI, never in repository
files: no ownership manifests, landing scripts or evidence archives.

## Goal agent

Driven by a person, ad hoc. It states goals and turns them into tickets, and it
runs audits that produce tickets (for example a CI waste audit or a
migration-scope review). Introspection about improving the codebase or this
workflow happens here. Large goals get a `kind:goal` issue whose children are
its sub-issues. It may reprioritise or close any ticket.

## Executor agents

Interchangeable and disposable: a checkout, `gh` with `GH_TOKEN` and the
toolchain are all you need. Executors run in cloud environments by default
(`.devcontainer/`; see docs/development.md). For Linux-only phases elsewhere, use
`python3 tools/dev.py docker -- <lane> …` rather than `docker` directly.

You own the ticket you claim, end to end, and you're trusted to use your
judgment: choose the approach, size each PR, fix what you trip over on the way,
and file what you find. Decisions on an issue are the goal agent's best guess; when
the code says otherwise, do what's right and explain it in the PR. Be bold. The
migration moves fastest when a PR moves a group of state together with the
operations over it and deletes their upstream bodies, and it only ever moves into
Rust.

1. If `python3 tools/ci.py stuck` lists a PR (conflicting, a required check
   failed, or the merge queue dropped it), fix the oldest one first, whoever opened it. If it
   reports main's merge tier waiting on a runner, run the `gh run cancel` it prints first. Otherwise claim the first
   issue `python3 tools/ci.py next` prints (see Tickets).
2. Branch from `main` and commit; `git config core.hooksPath .githooks` enables
   the quick pre-commit checks.
3. Push, open a PR whose body says `Closes #n` and what you checked, run
   `gh pr merge --auto --squash <pr>`, and go straight on to the next issue in
   the same turn. Opening the PR ends your part: once its PR tier passes, auto-merge
   puts it in `main`'s merge queue, which lands it unless it breaks the merge tier, and
   `ci.py stuck` lists a PR the queue drops, so don't wait for either. File follow-ups as `status:ready` sub-issues of the same goal as you find them.

Nobody needs to approve your next step, so keep the loop going on your own. Under
`/loop`, schedule each wakeup for 60 seconds, whatever the wakeup tool suggests:
you aren't waiting on anything, because the next issue is ready now. When
a question comes up, gather the data (read upstream, reproduce, measure) and make
the call yourself; you're trusted to choose well, and the PR is where you explain
the choice. `needs:decision` is only for exceptional cases: changing project policy,
or a choice that's hard to undo and that the data can't settle. Then add your
recommendation and work on something else until it's answered.

**Upstream security bugs stay private.** This repo is public. A memory-safety bug
or other likely vulnerability in upstream Swoole (or any dependency) never goes in
an issue, PR, commit message or code comment here. File it as an issue in the
private `loks0n/swoole-rs-security` repository with the upstream location, cause
and impact (the reproducer stays local), then carry on; a person handles
disclosure. Public code, comments and test exclusions don't mark where upstream
misbehaves either.

## Reviewer agents

Interchangeable and disposable, like executors. Each picks an area (a module, a
tool, a CI tier, recently merged PRs, stuck PRs) and checks that the work already
there is right: bugs, inconsistencies between code, tests and docs, and gaps inside work
that claims to be complete.

Judge work against what it set out to do: the issue, the PR description, the
module's entry in `migration/modules.json`. Work that is explicitly not done,
out of scope, or already tracked by an open issue is not a finding.

Verify each finding before fixing it: a failing test, a command and its output,
or the exact lines that disagree. Then fix it straight away, as an executor
would (steps 2 and 3), in a PR that states the evidence. Only a finding that
needs a decision becomes an issue, with `needs:decision`.

## Tickets

GitHub issues. Exactly one status label per open issue:

| Label | Meaning |
| --- | --- |
| `status:ready` | Claimable unless blocked |
| `status:claimed` | An executor owns it; see the latest `claim` comment |
| `status:review` | PR open |

Use GitHub's own relationships, not comments or labels:

- **Blocked:** mark the blocking issue in the issue's *Relationships* (`gh api -X
  POST repos/{owner}/{repo}/issues/<n>/dependencies/blocked_by -F issue_id=<id
  of the blocker>`). An issue is blocked while any blocker is open and is
  unblocked by GitHub the moment the last one closes. Claimable issues, in claim
  order: `python3 tools/ci.py next`.
- **Waiting on a decision:** rare (see Executor agents). Add `needs:decision`
  with the evidence and a recommendation; the goal agent or a person answers on
  the issue and removes the label.
- **Waiting on a CI run or measurement:** make the issue that produces it the
  blocker, or file one.
- **Goals:** a `kind:goal` issue's children are its sub-issues (`gh api -X POST
  repos/{owner}/{repo}/issues/<goal>/sub_issues -F sub_issue_id=<id of the child>`).

Closed means done. Priority: `p0` (breaks agents or `main`), `p1` (next up),
`p2` (when convenient). Kind: `kind:bug`, `kind:ci`, `kind:migration`,
`kind:goal`.

Claiming (all agents may share one GitHub account, and GitHub has no
compare-and-swap):

1. Comment `claim <agent-id>` and re-read the comments. The earliest `claim`
   after the last `release` wins; a loser deletes its comment and picks another.
2. The winner moves the label from `status:ready` to `status:claimed`.
3. Comment `heartbeat <agent-id>` at least every 30 minutes, with progress. A
   claim with no heartbeat or PR activity for 2 hours returns to `status:ready`.
4. Comment `release <agent-id>` and restore `status:ready` to give a ticket up.

## CI CLI

`tools/ci.py` (`wait`, `status`, `next`, `stuck`; see `--help`) is the only way agents watch
CI; GitHub's auto-merge and merge queue do the merging. It needs a checkout, `gh` and Python, so it works in every session.
The pre-commit and pre-push hooks remind you of the next command. Agents
improve it: add a subcommand when an agent would otherwise improvise (polling,
ad-hoc scripts, `gh run watch`).

## CI

Three tiers: the pre-commit hook (~5s warm), the PR tier (`pr.yml`, under 3 minutes
warm) and the merge tier (`merge.yml`, in `main`'s merge queue and again on `main`).
`main`'s ruleset requires a pull request, the PR tier's `required` check and the merge queue, which passes when every job it
`needs` passed or was skipped; add a blocking job there, and renaming `required`
means updating the ruleset in the same PR. `ext` (the Cargo-built
swoole.so with every module, extension load and corpus, a few minutes cached) runs only when a PR
touches `native/`, the build tools, `crates/phpstubs/`, `.github/` or `.devcontainer/`, and
is skipped, which counts as passing, otherwise. `gates` runs the modules a PR
touches and those depending on a touched crate. `phpt` runs the touched
modules' PHPT families on the Cargo build in parallel shards, gated on upstream's results from
main's latest merge-tier run (`ci.py phpt-baseline`). `callgrind` (`callgrind.yml`, the merge
tier's job) counts instructions against `benchmarks/callgrind_limits` when a PR touches
`crates/`, `native/` or `benchmarks/`: a PR that slows a workload past its limit fails here,
where it costs nobody else a queue rebuild. Breakage they miss is caught by
the merge queue. Keep them cached and audit
them regularly for waste and duplicate steps; slow or flaky CI is a ticket.

The merge queue takes a PR whose PR tier passed and runs the merge tier on it together
with the PRs queued ahead, on top of `main`, before any of them lands. That run's own
`required` job passes, like the PR tier's, when every job passed or was skipped. When it
fails, GitHub drops the PR from the queue and turns its auto-merge off. `ci.py stuck` lists
it with that run: fix what it broke and run `gh pr merge --auto --squash <pr>` again, or
only that when a job died on its runner.
A PR lands about a merge-tier run after it's queued.

On `main` the merge tier runs one at a time, latest wins: while a run is in progress, each
push to `main` replaces the pending run, so the next run covers `main`'s head.
Its result therefore describes the commit it names, which may be several merges
old: at most one run (about 15-30 minutes) behind `main`. Every job has a
`timeout-minutes` cap, so a hung run can't stall the queue.

### Red main

The queue tests every group on top of `main` before it lands, so `main`'s own run fails
only a phase that passes one run and fails the next. Never stop other work because of it. What must not slip through is a **new** failure:
a phase that passed in the previous completed merge-tier run, or one that didn't exist
then. Before you claim, glance at `python3 tools/ci.py status`: it lists them as `new failures`.

For each new failure, search open issues for the phase name (`ci.py status` lists
the open issues that mention it). If none mentions it, file a `p0` `kind:bug`
issue with the run link, the phase and its log path, and the commit range since
the previous run (group phases of one module into one issue). Then carry on with
your own ticket.
`p0` sorts first, so the next free executor picks it up. Failures that already
have an issue keep that issue's priority. A phase that fails in the queue whatever the PR
blocks every merge until it's fixed: fix it before anything else.
