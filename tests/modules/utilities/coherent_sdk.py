"""Stage the private header SDK of the pinned upstream build a helper compiles against."""
import hashlib
import subprocess


def digest(data):
    return hashlib.sha256(data).hexdigest()


def stage(work, build, source, commit):
    """work/include: the pin's include/ (from the Git checkout `source` at `commit`) and the
    config.h of `build`, the upstream tree that built the engine."""
    paths = subprocess.check_output(['git', '-C', str(source), 'ls-tree', '-r', '--name-only', commit, 'include'], text=True).splitlines()
    for path in paths:
        destination = work / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(subprocess.check_output(['git', '-C', str(source), 'show', commit + ':' + path]))
    sdk = work / 'include'
    (sdk / 'config.h').write_bytes((build / 'config.h').read_bytes())
    # macro_profile.py re-verifies headers_sha256 before reusing this SDK.
    return dict(string_header_sha256=digest((sdk / 'swoole_string.h').read_bytes()),
                headers_sha256={str(p): digest(p.read_bytes()) for p in sdk.rglob('*') if p.is_file()})
