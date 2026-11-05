#!/usr/bin/env python3
"""Compare exact pinned native bodies and the hybrid build's real callers at fake IO boundaries."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.build_hybrid import hybrid_tree, module_patches
from tools.build_upstream import COMMIT as PIN
from tools.harness import ROOT, run_isolated, write_report

# The upstream files the framing overlay patches.
PATHS = [path for patch in module_patches(ROOT / 'migration/patches/framing') for path in patch['upstream_paths']]


def original(path):
    return subprocess.check_output(['git', '-C', str(ROOT / 'target/upstream/src'), 'show', PIN + ':' + path], text=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    own = ROOT / 'tests/modules/framing'
    report = {'upstream_pin': PIN, 'scope': 'Exact pinned C++ bodies vs the hybrid build\'s actual M18 callers; instrumented IO/storage, not real socket or PHP validation', 'passed': False, 'commands': [], 'engines': []}
    try:
        # Upstream engine from the pin, Rust engine from the hybrid build's fully patched tree.
        hybrid_source, hybrid_flags = hybrid_tree(args.archive)
        report['hybrid_tree'] = str(hybrid_source)
        abi = [flag for flag in hybrid_flags if flag != '-I' + str(hybrid_source / 'include')]
        for engine, read in [('upstream', original), ('rust', lambda path: (hybrid_source / path).read_text())]:
            build = ROOT / 'target/framing-traces' / engine
            build.mkdir(parents=True, exist_ok=True)
            for name in ['swoole.h', 'swoole_string.h', 'swoole_socket.h']:
                (build / name).write_text('#include "trace.h"\n')
            for name in ['swoole_protocol.h', 'swoole_mime_type.h']:
                (build / name).write_text(read('include/' + name))
            # String::split is Rust's (crates/buffer, with its own gates); both engines drive
            # the framing callers through the pinned body over trace.h's String.
            string = original('src/core/string.cc')
            start = string.index('ssize_t String::split(')
            (build / 'split.cpp').write_text('#include "trace.h"\n#include <stdexcept>\nnamespace swoole {\n' + string[start:string.index('\n}  // namespace', start)] + '\n}\n')
            for path in PATHS:
                (build / Path(path).name).write_text(read(path))
            binary = build / engine
            command = ['c++', '-std=c++14', '-O2', '-UNDEBUG', '-I' + str(build), '-I' + str(own), *(abi if engine == 'rust' else []),
                       str(own / 'trace.cpp'), str(build / 'split.cpp'), *[str(build / Path(path).name) for path in PATHS],
                       *([str(args.archive.resolve()), '-lpthread', '-ldl', '-lm'] if engine == 'rust' else []), '-o', str(binary)]
            result = run_isolated(command, timeout=120)
            report['commands'].append({'argv': command, 'exit': result.returncode, 'stderr': result.stderr})
            if result.returncode:
                raise RuntimeError(engine + ' compilation failed')
            result = run_isolated([str(binary)], timeout=120)
            output = build / (engine + '.txt')
            output.write_text(result.stdout)
            (build / (engine + '.stderr')).write_text(result.stderr)
            report['engines'].append({'name': engine, 'exit': result.returncode, 'output': str(output), 'lines': len(result.stdout.splitlines())})
            if result.returncode:
                raise RuntimeError(engine + ' trace failed')
        upstream = (ROOT / 'target/framing-traces/upstream/upstream.txt').read_text().splitlines()
        rust = (ROOT / 'target/framing-traces/rust/rust.txt').read_text().splitlines()
        differences = [{'line': n, 'upstream': a, 'rust': b} for n, (a, b) in enumerate(zip(upstream, rust), 1) if a != b]
        report.update(differences=differences[:50], total_differences=len(differences), passed=len(upstream) == len(rust) and not differences)
    except Exception as error:
        report['error'] = str(error)
    write_report(args.output, report)
    print(json.dumps(report))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
