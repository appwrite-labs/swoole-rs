"""Independent artifact copies, using filesystem clones where available."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def copy_private(source, destination):
    """Publish copy2 bytes/metadata atomically; never hardlink mutable outputs."""
    source, destination = Path(source).resolve(strict=True), Path(destination)
    if destination.exists() and source.samefile(destination):
        raise shutil.SameFileError(str(source), str(destination))
    with tempfile.TemporaryDirectory(prefix='.private-copy-', dir=destination.parent) as temporary:
        staged = Path(temporary) / destination.name
        cloned = False
        if sys.platform == 'darwin':
            result = subprocess.run(['/bin/cp', '-c', '-p', str(source), str(staged)],
                                    capture_output=True)
            cloned = result.returncode == 0
        if not cloned:
            staged.unlink(missing_ok=True)
            shutil.copy2(source, staged)
        os.replace(staged, destination)
    return destination
