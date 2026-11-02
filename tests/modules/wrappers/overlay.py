"""This module's patches applied to the pinned sources, as the hybrid build compiles
them. Edit the patches with tools/patches.py; this module only applies them."""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import apply_patch_verified, module_patches
from tools.build_upstream import COMMIT as PIN

SOURCE = ROOT / 'target/upstream/src'
PATCHES = ROOT / 'migration/patches' / Path(__file__).parent.name


def original(path):
    return subprocess.check_output(['git', '-C', str(SOURCE), 'show', f'{PIN}:{path}'], text=True)


def generated():
    patches = module_patches(PATCHES)
    paths = list(dict.fromkeys(path for patch in patches for path in patch['upstream_paths']))
    with tempfile.TemporaryDirectory() as directory:
        tree = Path(directory)
        for path in paths:
            (tree / path).parent.mkdir(parents=True, exist_ok=True)
            (tree / path).write_text(original(path))
        for patch in patches:
            apply_patch_verified(tree, PATCHES / patch['path'], patch['upstream_paths'])
        return {path: (tree / path).read_text() for path in paths}
