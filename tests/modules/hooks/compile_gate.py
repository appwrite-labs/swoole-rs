#!/usr/bin/env python3
"""Compile all eight unchanged and selected complete native units, plus disabled guards.

Actual io_uring units require --config selecting a configured liburing2.8+
source. A compiled-out unit is recorded UNSUPPORTED, never enabled-backend proof.
"""
import argparse
import io
import json
import os
import re
import shutil
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import module_patches
from tools.build_upstream import COMMIT as PIN

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--config', type=Path, required=True)
    p.add_argument('--php-config', default='php-config')
    p.add_argument('--report', type=Path, required=True)
    a = p.parse_args()
    manifest = module_patches(ROOT/'migration/patches/hooks')
    paths = sorted({path for entry in manifest for path in entry['upstream_paths']})
    config = a.config.read_bytes()
    baseline_config = re.sub(rb'^#define SWOOLE_RS_HOOKS[^\n]*\n', b'', config, flags=re.MULTILINE)
    rows = []
    pkg_env = os.environ.copy()
    discovered_prefixes = []
    if sys.platform == 'darwin' and shutil.which('brew'):
        for package in ['openssl@3', 'libpq', 'curl']:
            query = subprocess.run(['brew', '--prefix', package], capture_output=True, text=True)
            if query.returncode == 0:
                directory = Path(query.stdout.strip())/'lib/pkgconfig'
                if directory.is_dir():
                    discovered_prefixes.append(str(directory))
    pkg_paths = [pkg_env['PKG_CONFIG_PATH']] if pkg_env.get('PKG_CONFIG_PATH') else []
    pkg_paths.extend(discovered_prefixes)
    if pkg_paths:
        pkg_env['PKG_CONFIG_PATH'] = os.pathsep.join(pkg_paths)
    with tempfile.TemporaryDirectory(prefix='swr-hooks-compile-') as temp:
        tree = Path(temp)
        data = subprocess.check_output(['git', '-C', str(a.source), 'archive', PIN, 'include', 'ext-src', 'thirdparty', 'php_swoole.h', 'php_swoole_api.h', *paths])
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            for member in archive:
                if member.isfile():
                    out = tree/member.name
                    out.parent.mkdir(parents=True, exist_ok=True)
                    out.write_bytes(archive.extractfile(member).read())
        (tree/'config.h').write_bytes(config)
        flags = ['-std=c++14', '-O1', '-DHAVE_CONFIG_H', '-DZEND_ENABLE_STATIC_TSRMLS_CACHE=1', '-I'+str(tree), '-I'+str(tree/'include'), '-I'+str(tree/'ext-src'), '-I'+str(ROOT/'native/hooks')]
        flags.extend(shlex.split(subprocess.check_output([a.php_config, '--includes'], text=True)))
        flags.extend(shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'openssl', 'libpq', 'libcurl'], text=True, env=pkg_env)))
        for variant in ['original', 'selected', 'disabled']:
            (tree/'config.h').write_bytes(baseline_config + (b'\n#define SWOOLE_RS_HOOKS 1\n' if variant == 'selected' else b''))
            if variant == 'selected':
                for entry in manifest:
                    patch = ROOT/'migration/patches/hooks'/entry['path']
                    subprocess.run(['git', 'apply', str(patch)], cwd=tree, check=True)
            for path in paths:
                command = ['c++', *flags, '-fsyntax-only', str(tree/path)]
                result = subprocess.run(command, capture_output=True, text=True)
                passed = result.returncode == 0 if variant != 'disabled' else result.returncode != 0 and 'Hooks overlay requires --enable-swoole-rs-hooks' in result.stderr
                ring = path.endswith(('iouring.cc', 'uring_socket.cc'))
                compiled_out = ring and b'#define SW_USE_IOURING 1' not in config
                status = 'UNSUPPORTED' if passed and compiled_out and variant != 'disabled' else 'PASS' if passed else 'FAIL'
                print(variant, path, result.returncode, status, flush=True)
                rows.append(dict(variant=variant, path=path, command=command, returncode=result.returncode, passed=passed, status=status, stderr=result.stderr))
    report = dict(pin=PIN, scope=__doc__, config=str(a.config.resolve()), pkg_config_path=pkg_env.get('PKG_CONFIG_PATH', ''), discovered_prefixes=discovered_prefixes, compiler=subprocess.check_output(['c++', '--version'], text=True).splitlines()[0], rows=rows)
    a.report.parent.mkdir(parents=True, exist_ok=True)
    a.report.write_text(json.dumps(report, indent=2)+'\n')
    raise SystemExit(any(not row['passed'] for row in rows))

if __name__ == '__main__':
    main()
