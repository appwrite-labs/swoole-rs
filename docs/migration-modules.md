# Migration modules

[`migration/modules.json`](../migration/modules.json) lists the migration
modules. Its top level holds `schema_version`, the pinned `upstream` (Swoole
v6.2.5, commit `76c657d88be39b7af6bf85e46bcbac0da7387760`, fetched at
`target/upstream/src/`) and `modules`. Each module entry has:

| Field | Meaning |
| --- | --- |
| `id`, `title` | Module ID (`M00`..`M29`) and description. |
| `wave` | Position in the dependency order. |
| `depends_on` | Module IDs that must be integrated first. |
| `owned_paths` | Repository paths for the module (crate, native adapter, patches, tests). |
| `phpt_families` | Upstream `tests/` directories whose PHPTs `dev.py candidate` gates for the module. |
| `upstream_patch_targets` | Upstream files the module mainly patches (informational). |
| `applies_first` | Apply this module's patches before every other module's. Only M15 (halfclose) sets it. |
| `feature_bit_reserved` | Feature value, where one is listed. |
| `abi_contract` | Path to the module's contract document or header. |

`python3 tools/check_migration.py` checks for duplicate IDs, unknown
prerequisites and dependency cycles, and prints the resulting waves.

Module names, IDs and feature values are `MODULES` in `tools/build_hybrid.py`.
Each module's patches live in `migration/patches/<module>/` and apply in
file-name order; their destinations are read from their diff headers.
`tools/build_hybrid.py` applies them to a fresh clone of the pinned reference
(see [hybrid build](hybrid-build.md)). Every Rust build applies every module and
sets its feature bit in `swr_v1_feature_bits()`. M15 (`halfclose`) is the
exception: a native correction applied only with `--fix-halfclose`.
