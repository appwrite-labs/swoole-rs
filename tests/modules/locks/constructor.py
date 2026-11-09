#!/usr/bin/env python3
"""Rust lock init for every kind, faulted only at native pthread boundaries; ownership gate."""
import argparse
import errno
import json
from pathlib import Path
import sys
import subprocess
import overlay
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_ext import native_flags
from tools.build_upstream import require_build_space
from tools.harness import run_isolated,write_report




def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True,help='Rust archive of the hybrid build under test');p.add_argument('--output',type=Path,required=True);p.add_argument('--sanitize',action='store_true');p.add_argument('--rust-asan',action='store_true');a=p.parse_args();overlay.SOURCE=a.source
    if a.rust_asan and (not a.sanitize or '__asan_report_' not in subprocess.check_output(['nm','-u',str(a.archive)],text=True,stderr=subprocess.DEVNULL)):p.error('Rust ASan requires instrumented archive imports and native sanitizer flag')
    folder=a.output.resolve().with_suffix('');folder.mkdir(parents=True,exist_ok=True)
    hybrid_flags=native_flags(a.archive)
    witness=str(Path(__file__).resolve().with_suffix('.cc'))
    common=['c++','-std=c++17','-O1','-UNDEBUG']
    if a.sanitize:common+=['-fsanitize=address,undefined','-fno-sanitize-recover=all']
    command=[*common,*hybrid_flags,witness,str(a.archive.resolve()),'-lpthread','-ldl','-lm','-o',str(folder/'constructor')]
    if sys.platform=='linux':command+=['-lrt','-lutil']
    report={'scope':__doc__,'pin':overlay.PIN,'compile':[command],'passed':False,'rust_instrumented':a.rust_asan,'native_instrumented':a.sanitize}
    require_build_space(ROOT);r=run_isolated(command,timeout=90)
    report.update(compile_exit=r.returncode,compile_stderr=r.stderr)
    if not r.returncode:
        r=run_isolated([str(folder/'constructor')],timeout=30);report.update(exit=r.returncode,stdout=r.stdout,stderr=r.stderr)
        if not r.returncode:
            direct=[json.loads(line)for line in r.stdout.splitlines()];report['direct_abi_observations']=direct
            for row in direct:
                row['passed']=row['result']=={0:errno.ENOMEM,1:errno.EAGAIN,2:errno.ENOTSUP}[row['stage']] and row['errno']==errno.EBUSY and row['allocated']==0 and row['released']==0 and row['init_calls']==int(row['stage']==0) and row['attr_init_calls']==int(row['kind']!=5) and row['shared_calls']==int(row['kind']!=5 and row['shared'] and row['stage']!=1) and row['attr_destroyed']==int(row['kind']!=5 and row['stage']!=1)
            report['direct_abi_complete']=len(direct)==(12 if sys.platform=='linux' else 10) and len({(row['kind'],row['shared'],row['stage']) for row in direct})==len(direct)
            report['passed']=report['direct_abi_complete'] and all(row['passed'] for row in direct)
    write_report(a.output,report);print(json.dumps({'passed':report['passed'],'rows':len(report.get('direct_abi_observations',[])),'compile_stderr':report['compile_stderr']}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
