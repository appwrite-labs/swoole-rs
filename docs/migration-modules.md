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
| `owned_paths` | Repository paths for the module (crate, native adapter, tests). |
| `phpt_families` | Upstream `tests/` directories whose PHPTs `dev.py candidate` gates for the module. |
| `upstream_patch_targets` | Upstream files the module replaced; nothing patches them any more (#2200). `tools/capability_inventory.py` maps upstream paths to modules with it. |
| `feature_bit_reserved` | Feature value, where one is listed. |
| `abi_contract` | Path to the module's contract document or header. |

`python3 tools/check_migration.py` checks for duplicate IDs, unknown
prerequisites and dependency cycles, and prints the resulting waves.

Module names, IDs and feature values are `MODULES` in `tools/check_migration.py`.
Every build links every module and sets its feature bit in `swr_v1_feature_bits()`. M15 (`halfclose`) is the
exception: a native correction applied only with `--fix-halfclose`.
