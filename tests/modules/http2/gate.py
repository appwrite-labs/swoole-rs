#!/usr/bin/env python3
"""Compare actual pinned/patched HTTP2 core TUs; native Socket send is recorded.

No PHP, network, HPACK, or server/client lifetime parity is implied. Native
alignment checks are excluded because pinned inline codecs use unaligned casts.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.build_upstream import COMMIT as PIN


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--include', action='append', default=[])
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    args.source = args.source.resolve()
    args.output = args.output.resolve()
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    commands = []
    report = {'passed': False, 'upstream_commit': PIN, 'scope': 'actual core production translation unit; Socket::send recording double',
              'not_covered': ['PHP client/server production TUs and runtime', 'HPACK', 'reentrant stream/session lifetimes', 'performance'],
              'native_sanitizers': ['address', 'undefined except alignment'] if args.sanitize else [],
              'rust_sanitized': False, 'commands': commands}

    def run(command, cwd=None):
        command = list(map(str, command))
        result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
        record = {'command': command, 'cwd': str(cwd or Path.cwd()), 'returncode': result.returncode,
                  'stdout': result.stdout, 'stderr': result.stderr}
        commands.append(record)
        (work / f'{len(commands):03d}.json').write_text(json.dumps(record, indent=2) + '\n')
        result.check_returncode()
        return result.stdout

    try:
        # The patched side is the hybrid build's fully patched tree, headers included.
        hybrid_source, hybrid_flags = hybrid_tree(args.archive)
        report['hybrid_tree'] = str(hybrid_source)
        original = work / 'original.cc'
        name = 'src/protocol/http2.cc'
        original.write_text(run(['git', 'show', f'{PIN}:{name}'], args.source))
        archive = args.archive.resolve()
        report['archive'] = str(archive)
        report['archive_sha256'] = digest(archive)
        compiler = os.environ.get('CXX', 'c++')
        base = [compiler, '-std=c++17', '-I', ROOT / 'native/http2']
        for include in args.include:
            base += ['-I', include]
        if args.sanitize:
            base += ['-fsanitize=address,undefined', '-fno-sanitize=alignment', '-fno-omit-frame-pointer', '-g']
        # The upstream side builds from the pin, the Rust side and its harness from the hybrid tree.
        pinned = base + ['-I', args.source / 'include']
        hybrid = base + [*hybrid_flags, '-DSW_USE_RUST_HTTP2']
        functions = ['put_default_setting', 'get_default_setting', 'pack_setting_frame', 'unpack_setting_data',
                     'get_frame_length', 'send_setting_frame', 'get_type', 'get_type_color']
        run(pinned + [f'-D{name}=original_{name}' for name in functions] + ['-c', original, '-o', work / 'original.o'])
        # The calls link from the hybrid's http2.cc while it exists, then from the archive.
        objects = [work / 'original.o']
        if (hybrid_source / name).is_file():
            run(hybrid + ['-c', hybrid_source / name, '-o', work / 'rust.o'])
            objects.append(work / 'rust.o')
        executable = work / 'core-calls'
        harness = ROOT / 'tests/modules/http2/core_calls.cpp'
        run(hybrid + [harness, *objects, archive, '-o', executable])
        report['core_calls'] = json.loads(run([executable]))
        report['passed'] = report['core_calls']['passed']
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output), 'error': report.get('error')}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
