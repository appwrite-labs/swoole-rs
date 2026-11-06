#!/usr/bin/env python3
"""Bounded framing ABI, MIME lookup and malformed-input safety; real callers are separate."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import run_isolated, write_report

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--archive',type=Path,required=True);p.add_argument('--sanitize',action='store_true');p.add_argument('--rust-asan',action='store_true');p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    if a.rust_asan and (not a.sanitize or '__asan_report_' not in subprocess.check_output(['nm','-u',str(a.archive)],text=True,stderr=subprocess.DEVNULL)):p.error('--rust-asan requires --sanitize and an instrumented archive')
    report={'scope':__doc__,'passed':False,'rust_instrumented':a.rust_asan}
    with tempfile.TemporaryDirectory(prefix='swr-wrapper-gate-') as folder:
        binary=Path(folder)/'gate';cmd=['c++','-std=c++17','-O2','-UNDEBUG','-I'+str(ROOT/'native/framing'),str(Path(__file__).with_suffix('.cc')),str(a.archive.resolve()),'-lpthread','-ldl','-lm','-o',str(binary)]
        if a.sanitize:cmd+=['-fsanitize=address,undefined','-fno-sanitize-recover=all']
        report['compile']=cmd;r=run_isolated(cmd,timeout=90);report.update(compile_exit=r.returncode,compile_stderr=r.stderr)
        if not r.returncode:
            r=run_isolated([str(binary)],timeout=90);report.update(exit=r.returncode,stdout=r.stdout,stderr=r.stderr,passed=r.returncode==0)
            if report['passed']:report['observations']=json.loads(r.stdout)
        if a.rust_asan and report['passed']:
            probe=Path(folder)/'probe.cc';probe.write_text('#include "swoole_rs_framing.h"\nint main(){auto p=new uint8_t[1]{0};int64_t out;return swr_v1_framing_unpack(67,p+1,1,&out);}\n')
            probe_binary=Path(folder)/'probe';probe_cmd=cmd.copy();probe_cmd[probe_cmd.index(str(Path(__file__).with_suffix('.cc')))]=str(probe);probe_cmd[probe_cmd.index(str(binary))]=str(probe_binary)
            compiled=run_isolated(probe_cmd,timeout=90);proof={'compile_exit':compiled.returncode,'compile_stderr':compiled.stderr,'scope':'Expected invalid-caller one-past read, separate from valid-input tests.'}
            if not compiled.returncode:
                invalid=run_isolated([str(probe_binary)],timeout=15);proof.update(exit=invalid.returncode,stderr=invalid.stderr,detected=invalid.returncode!=0 and 'AddressSanitizer: heap-buffer-overflow' in invalid.stderr and 'swr_v1_framing_unpack' in invalid.stderr)
            report['rust_asan_probe']=proof;report['passed']=bool(proof.get('detected'))
    write_report(a.output,report);print(json.dumps(report));return int(not report['passed'])
if __name__=='__main__':sys.exit(main())
