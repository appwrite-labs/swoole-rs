#!/usr/bin/env python3
"""Event catalog differential against the pinned definitions; not a PHP runtime gate."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import overlay
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_ext import native_flags
from tools.build_upstream import require_build_space
from tools.harness import run_isolated,write_report


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True,help='Rust archive of the hybrid build under test');p.add_argument('--output',type=Path,required=True);p.add_argument('--sanitize',action='store_true');p.add_argument('--rust-asan',action='store_true');a=p.parse_args();overlay.SOURCE=a.source
    if a.rust_asan and (not a.sanitize or '__asan_report_' not in subprocess.check_output(['nm','-u',str(a.archive)],text=True,stderr=subprocess.DEVNULL)):p.error('Rust ASan requires an instrumented archive and native sanitizer flag')
    folder=a.output.resolve().with_suffix('');folder.mkdir(parents=True,exist_ok=True)
    header=overlay.original('ext-src/php_swoole_server.h');php=overlay.original('ext-src/swoole_server_port.cc');native=overlay.original('include/swoole.h')
    enum=header[header.index('enum php_swoole_server_port_callback_type {'):];enum=enum[:enum.index('};')+2]
    catalog=php[php.index('struct ServerPortEvent {'):php.index('// clang-format on')]
    compare=native[native.index('static inline unsigned int swoole_strcaseeq('):];compare=compare[:compare.index('\n}')+2]
    reference=enum+'\n'+catalog+'\n'+compare+'\n';(folder/'reference.inc').write_text(reference)
    # The pinned reference against the hybrid build's Rust ABI.
    hybrid_flags=native_flags(a.archive)
    cmd=['c++','-std=c++17','-O1','-UNDEBUG',*hybrid_flags,'-I'+str(folder),str(Path(__file__).with_suffix('.cc')),str(a.archive.resolve()),'-lpthread','-o',str(folder/'gate')]
    # The archive members this pulls in reference the rest of the extension; drop the unused sections.
    cmd+=['-Wl,--gc-sections','-ldl','-lm'] if sys.platform=='linux' else ['-Wl,-dead_strip']
    if a.sanitize:cmd+=['-fsanitize=address,undefined','-fno-sanitize-recover=all','-g']
    report={'scope':__doc__,'pin':overlay.PIN,'compile':cmd,'passed':False,'native_instrumented':a.sanitize,'rust_instrumented':a.rust_asan}
    require_build_space(ROOT);r=run_isolated(cmd,timeout=90);report.update(compile_exit=r.returncode,compile_stderr=r.stderr)
    if not r.returncode:
        r=run_isolated([str(folder/'gate')],timeout=45);report.update(exit=r.returncode,stdout=r.stdout,stderr=r.stderr,passed=r.returncode==0)
    write_report(a.output,report);print(json.dumps({'passed':report['passed'],'stdout':report.get('stdout'),'compile_stderr':report.get('compile_stderr')}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
