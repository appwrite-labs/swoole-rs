#!/usr/bin/env python3
"""Exercise actual core config/thread views, snapshots, stable addresses and allocator calls."""
import argparse
import json
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated

SOURCE = r'''
#include "swoole.h"
#include <cassert>
#include <cstdlib>
#include <iostream>
int main() {
    swoole::Global global;
    auto *allocator_address=&global.std_allocator;
    global.max_sockets=73;auto limit=global.max_sockets;global.max_sockets=91;
    assert(limit==73 && global.max_sockets==91);
    swr_v1_core_config_allocator(global.config_handle());
    auto allocator=global.std_allocator;
    assert(allocator.malloc==::malloc && allocator.calloc==::calloc && allocator.realloc==::realloc && allocator.free==::free);
    void *bytes=allocator.malloc(17);assert(bytes);allocator.free(bytes);
    global.task_tmpfile=std::string(100,'x');auto snapshot=global.task_tmpfile;
    global.task_tmpfile="replacement";
    assert(snapshot==std::string(100,'x'));
    const std::string &reference=global.task_tmpfile;
    auto *reference_address=&reference;
    global.task_tmpfile="updated";
    assert(reference=="updated" && &static_cast<const std::string &>(global.task_tmpfile)==reference_address);
    global={};
    assert(&global.std_allocator==allocator_address && global.std_allocator.malloc==nullptr && global.max_sockets==0 && global.task_tmpfile.empty());
    assert(reference.empty());
    swoole::ThreadGlobal first;
    first.id=19;first.type=3;first.main_thread=true;first.error=71;
    auto *id_address=&first.id;auto id=first.id;
    swoole::ThreadGlobal copy=first;
    assert(&copy.id!=&first.id && copy.id==19 && copy.type==3 && copy.main_thread && copy.error==71);
    first.id=23;assert(id==19 && copy.id==19);
    first={};
    assert(&first.id==id_address && first.id==0 && !first.main_thread && first.error==0 && copy.id==19 && copy.error==71);
    std::cout<<"core config/thread views PASS\n";
}
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    source, flags = hybrid_tree(args.archive)
    report = dict(passed=False, scope=__doc__, hybrid_tree=str(source), commands=[])
    with tempfile.TemporaryDirectory(dir=args.output.parent, prefix='core-records-') as directory:
        tree = Path(directory)
        cpp = tree / 'probe.cc'
        cpp.write_text(SOURCE)
        binary = tree / 'probe'
        command = ['c++', '-std=c++14', '-O1', '-g', '-Wall', '-Wextra', '-Werror', *flags,
                   '-DHAVE_CONFIG_H', '-I' + str(source), str(cpp), str(args.archive.resolve()),
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
