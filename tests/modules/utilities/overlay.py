"""Stage current utility overlays with changed-file evidence."""
from pathlib import Path

from tools.build_hybrid import apply_patch_verified, module_patches


def apply_owned(source, patch):
    patch = Path(patch)
    entry = next(row for row in module_patches(patch.parent) if row['path'] == patch.name)
    return apply_patch_verified(source, patch, entry['upstream_paths'])
