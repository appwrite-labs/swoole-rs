# Development loops

`tools/dev.py` has three lanes, from fastest to complete:

```sh
# One crate or one named Rust test; no extension build.
python3 tools/dev.py edit --test rollback

# Cargo build, ABI/native/PHP gates and the module's PHPT selection.
python3 tools/dev.py candidate

# Reproduce one registered PHPT (candidate only).
python3 tools/dev.py candidate \
  --case tests/swoole_websocket_server/pack.phpt \
  --hypothesis 'Header changes preserve the PHP frame representation' \
  --expect 'Original pack assertions pass'

# Immutable build, fmt/Clippy, module gates, all applications and all original
# PHPTs, with upstream as control. Every build has every module; --module only
# chooses whose gates run (graduated modules' always do).
python3 tools/dev.py integrate --module timer
python3 tools/dev.py integrate --all-modules   # every module's gates; used by CI
```

- `--plan` prints the commands without running them.
- `--test` is edit-only; `--case` is candidate-only.
- Candidate runs only the Cargo build; `--compare` also runs upstream. The WebSocket
  differential always runs upstream. Candidate gates the chosen modules'
  `phpt_families` (`migration/modules.json`); `--changed-since REF` chooses the
  modules a diff touches.
- `--benchmark` runs benchmarks after the correctness phases (and implies
  `--compare`); skipped if any phase failed. Stop other builds first.
- `--php-extension NAME` passes dynamic PHP modules through to runners.
- `--fix-halfclose`, `--match-child-php` and the build profile switches
  (`--stdext`, `--php-sockets`) are forwarded to the builder or runners.

Edit and candidate stop at the first failure; integrate runs every phase and
fails if any failed. Each run writes logs, exit codes, phase durations,
hypothesis and expectation to `target/reports/dev/<UTC-time>-<id>/loop.json`.
For integration subsets, include the phases that produce reports consumed later
in the same run; the planner refuses missing prerequisites.

## Builds

The `cargo-build` phase runs `tools/build_ext.py --native`: Cargo builds
`target/ext/swoole.so` with no swoole-src checkout, the engine every PHP gate
loads, and lays out
`target/ext/sdk/` (the Zend-free `libswoole_rs_ffi.a`, crates/ext's native C and
C++ archives and the ABI headers) for the native gates, which link fixtures
outside PHP. Cargo rebuilds only what changed. Integrate's upstream control is
the unpatched pin built by `tools/build_upstream.py` (`reference-build`).

## Caches

Cargo caches the extension build. `tools/build_upstream.py` and the C/C++ test
harnesses compile through `ccache` when it is installed on the host (the dev image
has none); `build_upstream.py --no-ccache` disables it for the oracle.

## PHPT runner

```sh
python3 corpus/upstream/run.py --engine upstream=target/upstream/src/modules/swoole.so \
  --jobs 4 --module channels --module table
```

`--jobs` batches only an audited list of process-local cases (including HTTP
request parser and WebSocket pack/unpack); timer, fork, listener and
shared-file cases always run in their own process group. `--isolate-cases` runs
one official runner per test. `--case` selects exact paths. Missing, duplicate
or foreign results fail; XFAIL and "passed on retry" warnings count as failures.
Results are saved atomically after each batch with `execution_status` of
`running`, `complete` or `interrupted`.

## Linux environment

`.devcontainer/` is the same OS and packages as the CI runners (`ubuntu-24.04`,
PHP 8.3); `.devcontainer/install.sh` is shared with the merge tier, so the two
cannot drift. Open the folder in any devcontainer host (VS Code, Codespaces,
cloud agent environments), or run it by hand:

```sh
docker build -t swoole-rs-dev .devcontainer
docker run --rm -it -v "$PWD:/work" -w /work swoole-rs-dev bash
```

CI jobs run on gcrunner, appwrite-labs' ephemeral GCE runners: each job names a
preset from `.github/gcrunner.yml` (`runs-on: gcrunner=${{ github.run_id }}/runner=standard`).
This repo is public, so a fork's pull request runs only once a maintainer approves it
(the repository's `all_external_contributors` approval policy); read the diff first.

Tooling tests: `tests/dev/run.sh` (parallel; prints only failing files), or `python3 -m unittest discover -b -s tests/dev` (`-b` hides output from passing tests).

### One command: `dev.py docker`

```sh
python3 tools/dev.py docker -- integrate --module utilities --phase utilities-hash-originals
```

Runs any `dev.py` command in the dev image for this host's architecture, with no
manual setup:

- builds the image the first time (tagged `swoole-rs-dev:<arch>-<.devcontainer hash>`,
  so arm64 and amd64 never mix and a changed `.devcontainer/` gets a new image)
- mounts this worktree, and its git metadata, at their host paths, and points
  `core.hooksPath` at the worktree's `.githooks`, so a commit made inside runs the hook
  (as the host's `user.name` and `user.email`)
- keeps the toolchain and crate registry in named volumes (`swr-rustup-<arch>`,
  `swr-cargo-<arch>`), and cargo output in `target/docker/` of the worktree cargo
  runs in, so a worktree under this one never reuses this one's builds
- inside the container, installs the pinned toolchain, runs `cargo fetch --locked`,
  builds `target/upstream` (cached; concurrent entries in one worktree take turns at
  these steps, under `flock target/upstream/prepare.lock`, then run together), and passes the image's PHP extensions to
  `candidate`/`integrate` unless the command names its own
- adds `--with-prerequisites` to a `--phase` selection, so the builds and reports a
  phase needs run first
- runs a repository script instead of a lane when the first argument ends in `.py`,
  for example `dev.py docker -- benchmarks/callgrind.py --engine … --profiles DIR`
  (the image has valgrind), and a `cargo` command as itself, building into the same
  `target/docker/` dir

It warns when the disk has less than 20 GB free. `dev.py prune` also removes
`target/worktrees/*` older than its window. It keeps Docker images shared with
other worktrees.

To reproduce an x86 CI failure on an ARM host, add `--platform linux/amd64`
before `--`. `linux/arm64` is also supported. The selected architecture has its
own image, toolchain volumes and `target/docker/<arch>/` output. Docker must
support running that architecture; emulated timing is not native CI timing.

#### io_uring: `dev.py docker --iouring`

Docker's default seccomp profile answers `io_uring_setup` with EPERM, so in a default
`dev.py docker` run every original io_uring test fails at ring creation (the
`io_uring is available` assertion, `Err(Create)`). That is the expected, visible result
of an unsupported run. To run them against a real ring, opt in:

```sh
python3 tools/dev.py docker --iouring -- cargo test --locked -p swoole-rs-hooks --features iouring,upstream-symbols uring
```

`--iouring` adds exactly `--security-opt seccomp=tools/docker-seccomp-iouring.json` and
`--cap-add SYS_RESOURCE` to `docker run`. The profile is Docker's default (moby/profiles
`seccomp/default.json`) plus one rule allowing `io_uring_setup`, `io_uring_enter` and
`io_uring_register`. `CAP_SYS_RESOURCE` lets the ring's sendfile grow its pipe to 1 MiB
as upstream does. A container's root shares the host VM's per-user pipe quota
(`fs.pipe-user-pages-soft`), and on a busy host (self-hosted runners, dind) that quota
runs out. Pipes then shrink to two pages and the resize fails with EPERM, so a spliced
file no longer fits a socketpair and the sendfile tests block (#4230). AppArmor, devices,
ulimits and the other capabilities stay Docker's defaults; it is not `--privileged` and
not `seccomp=unconfined`. Ring memory is charged to the container's memory cgroup, so no
`memlock` ulimit is needed. The flag applies to whatever command the run executes.

The host must still allow io_uring: a Linux kernel built with `CONFIG_IO_URING` and
`kernel.io_uring_disabled` at 0 (`1` limits it to the `kernel.io_uring_group` group, `2`
disables it for everyone), and a VM whose kernel refuses it (#4167 saw EPERM on an
OrbStack aarch64 kernel without this flag; check `--iouring` there) is no better. On such
a host the tests still fail at ring creation with `--iouring`, visibly; native CI's
iouring variant remains the qualifying run.

#### Timing on a VM

On a Docker VM with idle vCPUs (OrbStack's aarch64 kernel, #4261), a sleep wakes late:
plain PHP's `usleep` of 10 to 100 ms overshot by 6.5 ms on average, with 184 of 300 calls
outside 10%. Under full CPU load the overshoot was 0.1 ms. Tests that check a sleep with
`time_approximate` (`tests/swoole_runtime/sleep.phpt` failed 11 of 20 idle runs on
upstream and 12 on Cargo, and none under load) then fail on both engines. Native CI
runners don't do this, and they are the qualifying run.

Any other image a tool or README builds gets one fixed name per purpose (for
example `swoole-protocols-core`), never a hash or `:local` tag, so a rebuild
replaces it instead of adding one; `docker image prune` clears the dangling one.

## Cloud executors

Executor agents (AGENTS.md) run in a cloud environment by default, not on a shared
laptop. The image above is all an executor needs: the toolchain, and `gh` for the
GitHub work. Nothing is installed by hand. In a fresh environment:

```sh
export GH_TOKEN=<token with repo scope>   # the cloud host usually injects this
git clone https://github.com/appwrite-labs/swoole-rs && cd swoole-rs
git config core.hooksPath .githooks        # pre-commit/pre-push checks
```

Then run the executor loop (claim, branch, PR, `gh pr merge --auto --squash`,
next issue). A GitHub Codespace or any devcontainer host
builds `.devcontainer/` automatically and injects `GH_TOKEN`, so the clone and the
hooks line are the only steps. Keep builds, profiles and clones under the
environment's own disk; `python3 tools/dev.py prune` clears a day's leavings.
