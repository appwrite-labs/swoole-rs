#!/usr/bin/env python3
"""Positive 24-byte overload contract: pinned bodies against the hybrid build's, over scripted draws."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from gate import PIN, body
from tools.build_hybrid import hybrid_tree
from tools.build_upstream import require_build_space
from tools.harness import run_isolated, write_report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test')
    parser.add_argument('--output', default='target/reports/http-boundary-rng.json')
    args = parser.parse_args()
    report = {'passed': False, 'scope': __doc__, 'pin': PIN, 'native_asan_ubsan': True, 'rust_instrumented': False,
              'unrun': ['actual entropy/thread-local RNG initialization', 'allocation/partial/exception/lifetime profiles',
                        'originals/configuration/default activation/performance']}
    try:
        # Upstream side from the pin, selected side from the hybrid build's fully patched tree.
        hybrid_source, hybrid_flags = hybrid_tree(args.archive)
        report['hybrid_tree'] = str(hybrid_source)
        original = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN+':src/core/base.cc'], text=True)
        selected = (hybrid_source/'src/core/base.cc').read_text()
        first = original.index('static constexpr char characters[]')
        characters = original[first:original.index('};', first)+2]
        generated = '#include "swoole_rs_corebase.h"\n#include <cstdlib>\n#include <string>\n#include <vector>\n#include <utility>\n'
        for namespace, source in [('original', original), ('selected', selected)]:
            generated += 'namespace '+namespace+' {\nsize_t seed=0,position=0;std::vector<std::pair<int,int>> draws;\nint swoole_rand(int low,int high){draws.emplace_back(low,high);return (seed+position++)%62;}\n'
            if namespace == 'original': generated += characters+'\n'
            generated += body(source, 'void swoole_random_string(char *buf, size_t len)')+'\n'
            generated += body(source, 'void swoole_random_string(std::string &str, size_t len)')+'\n}\n'
        with tempfile.TemporaryDirectory(prefix='swr-http-boundary-rng-') as temporary:
            tree = Path(temporary)
            (tree/'rng_bodies.h').write_text(generated)
            binary = tree/'gate'
            command = ['c++', '-std=c++14', '-O1', '-g', '-fsanitize=address,undefined', '-fno-sanitize-recover=all',
                       *hybrid_flags, '-I'+str(tree), str(Path(__file__).with_suffix('.cc')), str(args.archive.resolve()), '-lm', '-o', str(binary)]
            if sys.platform.startswith('linux'): command += ['-ldl', '-lpthread', '-lrt', '-lutil']
            report['free_before_compile_bytes'] = require_build_space(tree)
            for label, command in [('compile', command), ('run', [str(binary)])]:
                result = run_isolated(command, timeout=60)
                report[label] = {'command': command, 'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
                if result.returncode: return 1
            report['outcome'] = json.loads(report['run']['stdout'].splitlines()[-1])
            report['passed'] = True
            print(json.dumps(report['outcome']))
            return 0
    except Exception as error:
        report['prerequisite_or_tooling_error'] = str(error)
        return 1
    finally:
        write_report(args.output, report)


if __name__ == '__main__':
    sys.exit(main())
