#!/usr/bin/env python3
"""Check module IDs and that module dependencies are known and acyclic."""
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.harness import ROOT

# The migration's modules: name -> (migration/modules.json id, the feature bit swoole.so reports).
MODULES = {'websocket': ('M02', 1), 'buffer': ('M01', 2), 'timer': ('M03', 4),
           'base64': ('M09', 8), 'crc32': ('M10', 16), 'error': ('M11', 32), 'protocols': ('M13', 128), 'memory': ('M12', 64),
           'http': ('M04', 256), 'utilities': ('M14', 512), 'locks': ('M16', 1024), 'wrappers': ('M17', 2048), 'framing': ('M18', 4096), 'corebase': ('M19', 8192), 'reactor': ('M05', 16384), 'coroutine': ('M06', 32768), 'hooks': ('M07', 65536), 'ossupport': ('M20', 131072), 'tls': ('M21', 262144), 'process': ('M08', 524288), 'clients': ('M22', 1048576), 'httpbindings': ('M23', 2097152), 'http2': ('M24', 4194304), 'ports': ('M25', 8388608), 'phphelpers': ('M26', 16777216), 'phpcollections': ('M27', 33554432), 'phpevents': ('M28', 67108864), 'phpcoroutines': ('M29', 134217728)}
# Every Rust build has every module: none has a configure switch or a cargo feature.
# Graduated modules (docs/migration.md#graduation) have their gates run on every dev.py run.
GRADUATED = ('base64', 'buffer', 'corebase', 'crc32', 'error', 'framing', 'http', 'protocols', 'websocket', 'timer')


def validate(manifest):
    modules = manifest['modules']
    ids = [module['id'] for module in modules]
    if len(set(ids)) != len(ids):
        raise ValueError('Duplicate module IDs')
    known = set(ids)
    for module in modules:
        if not set(module['depends_on']) <= known:
            raise ValueError(f'{module["id"]}: unknown prerequisite')
    done, waves = set(), []
    while len(done) != len(modules):
        ready = [m['id'] for m in modules if m['id'] not in done and set(m['depends_on']) <= done]
        if not ready:
            raise ValueError('Migration dependency cycle')
        waves.append(ready)
        done.update(ready)
    return waves


def main():
    manifest = json.loads((ROOT / 'migration/modules.json').read_text())
    waves = validate(manifest)
    print(f'{len(manifest["modules"])} modules; dependencies resolve.')
    for number, wave in enumerate(waves):
        print(f'Wave {number}: {", ".join(wave)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
