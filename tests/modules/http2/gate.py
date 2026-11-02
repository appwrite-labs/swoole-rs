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
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree, module_patches
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
              'rust_sanitized': False, 'commands': commands,
              'module_header_sha256': digest(ROOT / 'native/http2/rust_http2.h')}

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
        manifest = module_patches(ROOT / 'migration/patches/http2')
        # The patched side is the hybrid build's fully patched tree, headers included.
        hybrid_source, hybrid_flags = hybrid_tree(args.archive)
        report['hybrid_tree'] = str(hybrid_source)
        scratch = work / 'overlay'
        for name in manifest[0]['upstream_paths']:
            path = scratch / name
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(hybrid_source / name, path)
        report['patched_sources'] = {name: digest(scratch / name) for name in manifest[0]['upstream_paths']}
        original = work / 'original.cc'
        name = 'src/protocol/http2.cc'
        original.write_text(run(['git', 'show', f'{PIN}:{name}'], args.source))
        if original.read_text() == (scratch / name).read_text():
            raise ValueError('Hybrid tree lacks the http2 patch')
        rust_sources = {str(path.relative_to(ROOT)): digest(path)
                        for path in sorted((ROOT / 'crates/http2/src').rglob('*.rs'))}
        report['rust_sources'] = rust_sources
        archive = args.archive.resolve()
        unit = work / 'policy-unit'
        run(['rustc', '--edition=2021', '--test', ROOT / 'crates/http2/src/lib.rs', '-o', unit])
        report['rust_unit_output'] = run([unit])
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
        run(hybrid + ['-c', scratch / name, '-o', work / 'rust.o'])
        executable = work / 'core-calls'
        harness = ROOT / 'tests/modules/http2/core_calls.cpp'
        run(hybrid + [harness, work / 'original.o', work / 'rust.o', archive, '-o', executable])
        report['core_calls'] = json.loads(run([executable]))
        if digest(ROOT / 'native/http2/rust_http2.h') != report['module_header_sha256']:
            raise ValueError('Module header changed during checks')
        if rust_sources != {str(path.relative_to(ROOT)): digest(path)
                            for path in sorted((ROOT / 'crates/http2/src').rglob('*.rs'))}:
            raise ValueError('Rust module sources changed during checks')
        report['passed'] = report['core_calls']['passed']
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output), 'error': report.get('error')}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
