#!/usr/bin/env python3
"""Compare actual Socket inline and timeout bodies; native effect/lifetime facades, not real context execution."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from ordered_gate import block, sources

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT as PIN

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source',type=Path,required=True)
    p.add_argument('--archive',type=Path,required=True)
    p.add_argument('--report',type=Path,required=True)
    p.add_argument('--sanitize',action='store_true')
    a=p.parse_args()
    header='include/swoole_coroutine_socket.h'
    unit='src/coroutine/socket.cc'
    originals,selected,hybrid=sources(a.source,a.archive,[header,unit])
    signatures=['Coroutine *get_bound_co(', 'long get_bound_cid(', 'void check_bound_co(', 'void set_err(const int e)', 'void check_return_value(', 'bool is_available(', 'void set_timeout(const timeval *timeout,']
    methods=['const char *Socket::get_event_str(', 'bool Socket::TimeoutController::has_timedout(']
    with tempfile.TemporaryDirectory(prefix='swr-hooks-inline-') as directory:
        tree=Path(directory)
        for variant in ['original','selected']:
            texts=originals if variant=='original' else selected
            # The selected header's bound-coroutine accessors over its Rust state (#439).
            selected_inline=['Coroutine *event_co(', 'void bind_co('] if variant=='selected' else []
            (tree/(variant+'-inline.inc')).write_text('\n'.join(block(texts[header],signature) for signature in signatures+selected_inline))
            (tree/(variant+'-methods.inc')).write_text('\n'.join(block(texts[unit],signature) for signature in methods))
        command=['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/hooks'),'-I'+str(ROOT/'tests/modules/hooks'),str(ROOT/'tests/modules/hooks/socket_policy_gate.cc'),str(a.archive.resolve()),'-lpthread','-o',str(tree/'inline')]
        if a.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
        if sys.platform.startswith('linux'):command+=['-ldl','-lm']
        build=subprocess.run(command,capture_output=True,text=True)
        run=subprocess.run([str(tree/'inline')],capture_output=True,text=True) if build.returncode==0 else build
        print(build.stderr,end='');print(run.stdout,end='')
        if run is not build:print(run.stderr,end='')
        report=dict(pin=PIN,scope=__doc__,hybrid_tree=hybrid,command=command,compile_returncode=build.returncode,compile_stderr=build.stderr,returncode=run.returncode,stdout=run.stdout,stderr=run.stderr)
        a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n')
        raise SystemExit(run.returncode)

if __name__=='__main__':main()
