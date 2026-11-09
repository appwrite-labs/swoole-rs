#!/usr/bin/env python3
"""Compile pinned native bodies and real Rust overlay callers; no upstream edits."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_ext import native_flags
from tools.harness import run_isolated,write_report
from importlib.util import spec_from_file_location,module_from_spec
spec=spec_from_file_location('locks_overlay',Path(__file__).with_name('overlay.py'));overlay=module_from_spec(spec);spec.loader.exec_module(overlay)


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True,help='Rust archive of the hybrid build under test');p.add_argument('--sanitize',action='store_true');p.add_argument('--rust-asan',action='store_true');p.add_argument('--output',type=Path,default=ROOT/'target/reports/locks-gate.json');a=p.parse_args();overlay.SOURCE=a.source
    if a.rust_asan:
        if not a.sanitize:p.error('--rust-asan requires --sanitize')
        symbols=subprocess.check_output(['nm','-u',str(a.archive)],text=True,stderr=subprocess.DEVNULL)
        if '__asan_report_' not in symbols:p.error('Archive lacks Rust ASan report imports')
    pin=overlay.PIN;original=overlay.original
    # Reference side from the pin; the hybrid side is native/locks/swoole_lock.h over the archive.
    hybrid_flags=native_flags(a.archive)
    report={'upstream_commit':pin,'rust_asan_declared':a.rust_asan,'scope':'Actual pinned pthread bodies vs real Rust overlay facades; thread/fork/RW/timeout/barrier/atomic/futex witnesses. C++ sanitizer; Rust instructions instrumented only when rust_asan_declared=true; prebuilt Rust stdlib and OS code excluded. Coroutine lifecycle, PHP/ZTS and optimized performance are separate gates.','passed':False}
    support='''#pragma once
#include <cstdint>
#include <cerrno>
#include <climits>
#include <pthread.h>
#include <unistd.h>
#include <sys/mman.h>
#include <functional>
#include <chrono>
using sw_atomic_t=volatile uint32_t;
#define sw_atomic_add_fetch(p,n) __sync_add_and_fetch(p,n)
#define sw_atomic_sub_fetch(p,n) __sync_sub_and_fetch(p,n)
#define sw_atomic_fetch_add(p,n) __sync_fetch_and_add(p,n)
#define sw_atomic_cmp_set(p,o,n) __sync_bool_compare_and_swap(p,o,n)
#define sw_atomic_memory_barrier() __sync_synchronize()
#define SW_LOOP while(true)
#define SW_NUM_BILLION 1000000000
#define SW_NUM_MILLION 1000000
struct Pool { static constexpr size_t page=1<<16; void *alloc(size_t n){return n<=page?mmap(nullptr,page,PROT_READ|PROT_WRITE,MAP_SHARED|MAP_ANON,-1,0):nullptr;} void free(void*p){munmap(p,page);} };
inline Pool *sw_mem_pool(){static Pool pool;return &pool;}
// Upstream's non-futex wait registry (misc.cc #else branch) needs these.
#include <thread>
#include <algorithm>
#define sw_spinlock_release(lock) __sync_lock_release(lock)
#define sw_atomic_cpu_pause() std::this_thread::yield()
#define SW_ERROR_MALLOC_FAIL 501
inline void swoole_fatal_error(int,const char*){abort();}
inline void swoole_set_last_error(int e){errno=e;}
'''
    base=original('src/core/base.cc');wait=base[base.index('bool sw_wait_for('):base.index('\nint swoole_itoa')]
    timer=original('src/os/timer.cc');realtime=timer[timer.index('void realtime_get('):timer.index('}  // namespace swoole')]
    misc=original('src/core/misc.cc');futex=misc[misc.index('#ifdef HAVE_FUTEX'):misc.index('/* {{{ DJBX33A')]
    # The archive defines these symbols too, so the pinned ones go in namespace reference,
    # with the futex headers they include hoisted out of it.
    kernel='#ifdef HAVE_FUTEX\n#include <linux/futex.h>\n#include <sys/syscall.h>\n#endif\n'
    futex=futex.replace('#include <linux/futex.h>\n','').replace('#include <sys/syscall.h>\n','')
    original_bodies=''
    for name in ['mutex','rw_lock','spin_lock','barrier']:
        path=f'src/lock/{name}.cc';original_bodies+=original(path).replace('#include "swoole_lock.h"','#include "reference.h"').replace('namespace swoole','namespace reference')+'\n'
    # The hybrid has no lock class: gate.cc drives Rust's lock ABI, and Barrier's methods are Rust.
    with tempfile.TemporaryDirectory(prefix='swr-locks-') as temp:
        tree=Path(temp);(tree/'reference').mkdir();(tree/'ported').mkdir();(tree/'support.h').write_text(support)
        (tree/'reference/reference.h').write_text(original('include/swoole_lock.h').replace('#include "swoole_memory.h"','#include "support.h"').replace('namespace swoole','namespace reference'))
        (tree/'reference/original.inc').write_text(kernel+'\nnamespace reference {\n'+futex+wait+realtime+'}\n'+original_bodies)
        (tree/'ported/swoole_lock.h').write_text((ROOT/'native/locks/swoole_lock.h').read_text().replace('#include "swoole_memory.h"','#include "support.h"'))
        # Two translation units: the reference side sees only pinned text, the ported side
        # and the driver the hybrid build's flags.
        binary=tree/'gate';witness=str(Path(__file__).with_suffix('.cc'))
        common=['c++','-std=c++17','-O2','-DHAVE_RWLOCK','-I'+str(tree)]
        if sys.platform.startswith('linux'):common+=['-DHAVE_SPINLOCK','-DHAVE_FUTEX','-DHAVE_PTHREAD_BARRIER','-DHAVE_MUTEX_TIMEDLOCK','-DHAVE_RWLOCK_TIMEDRDLOCK']
        if a.sanitize:common+=['-fsanitize=address,undefined','-fno-sanitize-recover=all']
        reference=[*common,'-c','-DLOCKS_REFERENCE','-I'+str(tree/'reference'),witness,'-o',str(tree/'reference.o')]
        cmd=[*common,'-I'+str(tree/'ported'),*hybrid_flags,witness,str(tree/'reference.o'),str(a.archive.resolve()),'-lpthread','-lm','-o',str(binary)]
        if sys.platform.startswith('linux'):cmd+=['-ldl','-lrt','-lutil']
        report['compile']=[reference,cmd];r=run_isolated(reference,timeout=90)
        if not r.returncode:r=run_isolated(cmd,timeout=90)
        report.update(compile_exit=r.returncode,compile_stderr=r.stderr)
        if not r.returncode:
            r=run_isolated([str(binary)],timeout=30);report.update(exit=r.returncode,stdout=r.stdout,stderr=r.stderr,passed=r.returncode==0)
            if r.returncode==0:report['observations']=json.loads(r.stdout)
        if a.rust_asan and report['passed']:
            # Deliberately invalid caller: only Rust dereferences this one-past pointer.
            probe=tree/'asan-probe.cc';probe.write_text('#include "swoole_rs_locks.h"\nint main(){auto p=new uint32_t[1]{0};return swr_v1_locks_coroutine_try(p+1,0,0,42,7,0);}\n')
            probe_binary=tree/'asan-probe';probe_cmd=[part for part in cmd if part!=str(tree/'reference.o')];probe_cmd[probe_cmd.index(witness)]=str(probe);probe_cmd[probe_cmd.index(str(binary))]=str(probe_binary)
            compiled=run_isolated(probe_cmd,timeout=90)
            proof={'compile_exit':compiled.returncode,'compile_stderr':compiled.stderr,'scope':'Expected invalid-caller heap overflow in Rust coroutine CAS; excluded from valid-input behavior checks.'}
            if not compiled.returncode:
                invalid=run_isolated([str(probe_binary)],timeout=15);proof.update(exit=invalid.returncode,stderr=invalid.stderr,detected=invalid.returncode!=0 and 'AddressSanitizer: heap-buffer-overflow' in invalid.stderr and 'swr_v1_locks_coroutine_try' in invalid.stderr)
            report['rust_asan_probe']=proof;report['passed']=bool(proof.get('detected'))
    write_report(a.output,report);print(json.dumps(report));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
