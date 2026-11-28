#!/usr/bin/env python3
"""Exercise the Rust-owned SwooleG and SwooleTG through their C++ views: init's defaults, stable
addresses, allocator calls, per-thread records and clean's reset."""
import argparse
import json
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_ext import native_flags
from tools.harness import run_isolated

SOURCE = r'''
#include "swoole.h"
#include <cassert>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <thread>
void swoole_clean();  // Rust's, declared by no header: native code never calls it
int main() {
    auto *allocator_address=&SwooleG.std_allocator;
    assert(SwooleG.std_allocator.malloc==nullptr && SwooleG.max_sockets==0 && !SwooleTG.main_thread);
    swr_v1_core_swoole_init();
    auto allocator=SwooleG.std_allocator;
    assert(allocator.malloc==::malloc && allocator.calloc==::calloc && allocator.realloc==::realloc && allocator.free==::free);
    void *bytes=allocator.malloc(17);assert(bytes);allocator.free(bytes);
    assert(SwooleG.max_sockets>0 && SwooleG.running && SwooleG.enable_coroutine && SwooleG.pagesize>0);
    SwooleG.max_sockets=91;SwooleG.dns_cache_refresh_time=2.5;
    assert(SwooleG.max_sockets==91 && SwooleG.dns_cache_refresh_time==2.5);
    assert(swr_v1_core_set_task_tmpdir("/tmp") && !swr_v1_core_set_task_tmpdir("relative"));
    assert(SwooleTG.main_thread && swoole_is_main_thread());
    SwooleTG.id=19;SwooleTG.type=3;
    auto *id_address=&SwooleTG.id;
    std::thread([id_address] {
        assert(&SwooleTG.id!=id_address && SwooleTG.id==0 && SwooleTG.type==0 && !SwooleTG.main_thread);
        SwooleTG.id=23;
        assert(SwooleTG.id==23);
    }).join();
    assert(&SwooleTG.id==id_address && SwooleTG.id==19 && SwooleTG.type==3);
    swoole_clean();
    assert(&SwooleG.std_allocator==allocator_address && SwooleG.std_allocator.malloc==nullptr && SwooleG.max_sockets==0);
    assert(SwooleG.dns_cache_refresh_time==0 && !SwooleG.running);
    std::cout<<"core config/thread views PASS\n";
}
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    flags = native_flags(args.archive)
    report = dict(passed=False, scope=__doc__, commands=[])
    with tempfile.TemporaryDirectory(dir=args.output.parent, prefix='core-records-') as directory:
        tree = Path(directory)
        cpp = tree / 'probe.cc'
        cpp.write_text(SOURCE)
        binary = tree / 'probe'
        command = ['c++', '-std=c++14', '-O1', '-g', '-Wall', '-Wextra', '-Werror', *flags,
                   '-DHAVE_CONFIG_H', str(cpp), str(args.archive.resolve()),
                   '-fsanitize=address,undefined', '-fno-sanitize-recover=all', '-pthread', '-o', str(binary)]
        if sys.platform.startswith('linux'):
            command += ['-ldl', '-lm']
        for command in [command, [str(binary)]]:
            result = run_isolated(command, timeout=90)
            report['commands'].append(dict(argv=command, exit=result.returncode,
                                           stdout=result.stdout, stderr=result.stderr))
            if result.returncode:
                break
        else:
            report['passed'] = result.stdout == 'core config/thread views PASS\n'
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed']}))
    return not report['passed']


if __name__ == '__main__':
    raise SystemExit(main())
