# Hybrid build

The hybrid build links a Rust static library into upstream Swoole's PHP
extension through [ABI v1](abi-v1.md). Every Rust build applies every module's
patches; there are no partial builds. One Swoole extension is loaded per PHP
process.

The reference is Swoole v6.2.5, commit
`76c657d88be39b7af6bf85e46bcbac0da7387760`, at `target/upstream/src/`. The
builder refuses a different commit or tracked modifications there, and applies
checksum-verified patches from `migration/patches/<module>/` to a separate Git
clone. Do not edit the reference to develop a patch.

## Editing patches

Each module's patches are a branch of the pinned upstream, one commit per patch
file named after it, over a `base: …` commit holding the patches the build
applies first (every module ahead of it in build order), so a module can edit
lines an earlier module already changed:

```sh
python3 tools/patches.py checkout phphelpers   # target/patches/phphelpers, branch patches/phphelpers
# edit, then amend or fixup the commit for that patch
python3 tools/patches.py export phphelpers     # rewrites the patch files
rm -rf target/patches/phphelpers
```

Upstream changes then surface as ordinary rebase conflicts. Export keeps the
bytes of every patch whose commit diff is unchanged, writes the others as
`git diff` output and drops patch files that no longer have a commit; add a
commit to add a patch.

Patches apply in file-name order. Older ones are numbered (`0001-topic.patch`);
export names a new commit `<author time, UTC>-<subject>.patch`
(`20261102T153012-buffer-handle.patch`), which sorts after them, so two branches
adding patches to one module never conflict. There is no manifest: each patch's
sha256 and upstream destinations come from the file itself
(`tools.build_hybrid.module_patches`).

## Following an upstream release

Rebase every module branch onto the new tag, export, then refresh what the repo
records about upstream files:

```sh
OLD=$(python3 -c 'from tools.build_upstream import COMMIT; print(COMMIT)')
TAG=v6.2.5
modules=$(ls migration/patches)
for m in $modules; do python3 tools/patches.py checkout $m; done
for m in $modules; do
  git -C target/patches/$m fetch -q --depth 1 https://github.com/swoole/swoole-src.git tag $TAG
  git -C target/patches/$m rebase --onto $TAG $OLD   # resolve conflicts as for any rebase
done
# Set VERSION and COMMIT in tools/build_upstream.py to the tag and its commit.
for m in $modules; do python3 tools/patches.py export $m && rm -rf target/patches/$m; done
python3 tools/patches.py rehash $OLD
git grep -n "$OLD"                                  # pins and tags left to update by hand
rm -rf target/upstream && python3 tools/build_upstream.py
```

`rehash` rewrites each recorded sha256 or git blob id of an upstream file that
changed between the two commits, wherever the repo records it, and lists
upstream files that were deleted. It leaves commit and tag references (some are
history) and recorded line counts alone. Then add any new original PHPTs to
`corpus/upstream/manifest.json` (`git diff --stat $OLD $TAG -- tests`) and run
the merge tier.

## Build variants

Needs the PHP toolchain from the [corpus instructions](../corpus/README.md) plus
Rust/Cargo. Run from the repository root.

```sh
python3 tools/check_migration.py
python3 tools/build_upstream.py

python3 tools/build_hybrid.py                      # every module
```

`--backend rust` is the default. Module names and feature values are listed in
[migration modules](migration-modules.md).

Shared options (also on `build_upstream.py`): `--jobs N`, `--php-config PATH`,
`--phpize PATH`, `--openssl-dir PREFIX`, `--stdext`, `--php-sockets`,
`--ccache PATH` / `--no-ccache` (ccache is auto-detected and uses its own
settings), and repeatable `--configure=ARG` for extra `./configure` arguments. The runners' `--php` must match the build
toolchain. Neither builder installs the extension or edits php.ini.

Stable paths point to the latest build of each variant:

| Variant | PHP extension |
| --- | --- |
| Reference | `target/upstream/src/modules/swoole.so` |
| Hybrid | `target/hybrid/rust/modules/swoole.so` |
| Reference with `--stdext`, `--php-sockets` or `--configure` | `target/upstream-builds/<profile>/src/modules/swoole.so` (`build_upstream.tree()`) |

Builds live in `target/hybrid/builds/<input-sha256>/` with `src`, `modules`,
`rust-prefix/` (archive and headers), `state.json`, `build-manifest.json` and
`build.log`. Variant directories symlink into them, so a rebuild can move a
stable path; record the resolved path and SHA-256 that runners print. The
clones share Git objects with the reference checkout, so keep it in place.

The input hash covers backend, pinned commit, ordered patch hashes and
destinations, builder hashes, configure arguments and pkg-config CPPFLAGS,
exported headers, the Rust archive, toolchain versions and compiler environment.
A cached build is reused only if its source diff, pin and artifact checksum
still match; otherwise the builder refuses and leaves the directory for
inspection. Patch hunk offsets must be exact; a patch that only applies with
an offset is rejected. Builds are traceable but not byte-identical: upstream
embeds `__DATE__`/`__TIME__`.

`--export /absolute/new-directory` copies the DSO, Rust archive, config,
manifest, Makefile and build log of an immutable build into a new directory
with an `export.json` receipt. It never replaces an existing export and is
rejected with `--dev`.

`--dev` uses an incremental tree; see [development](development.md).

`--fix-halfclose` (default off, alias suffix `-halfclose`) adds the M15 native
patches: drain queued output before close. It changes behavior relative to the
reference and does not make general TCP half-close work.

## Running the corpus and PHPTs against builds

```sh
python3 corpus/run.py \
  --engine hybrid=target/hybrid/rust/modules/swoole.so \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --require-all --output target/reports/corpus-hybrid.json

python3 corpus/upstream/run.py \
  --engine hybrid=target/hybrid/rust/modules/swoole.so \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --require-all --output target/reports/phpt-hybrid.json
```

Runners use `php -n`. Where PHP modules are dynamic, pass
`--php-extension pdo --php-extension pdo_sqlite --php-extension curl
--php-extension sockets` (and `tokenizer` for the Hyperf project) consistently
to every command. Do not pass these for modules built into PHP (Homebrew):
duplicate-module warnings break PHPT output. PHPTs run through PHP's official
runner; never use upstream's custom runner or `tests/start.sh`.

Benchmarks are in [benchmarks/README.md](../benchmarks/README.md) and
module-specific gates in [validation](validation-suite.md).
