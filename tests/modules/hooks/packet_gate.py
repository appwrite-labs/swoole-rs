#!/usr/bin/env python3
"""Compare actual vector all-transfer and packet length/EOF/dispatch bodies with native buffers and protocol callbacks; no kernel/context proof."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from ordered_gate import block

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import module_patches
from tools.build_upstream import COMMIT as PIN

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source',type=Path,required=True)
    p.add_argument('--archive',type=Path,required=True)
    p.add_argument('--report',type=Path,required=True)
    p.add_argument('--sanitize',action='store_true')
    a=p.parse_args()
    unit='src/coroutine/socket.cc'
    manifest=module_patches(ROOT/'migration/patches/hooks')
    paths=manifest[0]['upstream_paths']
    originals={path:subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+path],text=True) for path in paths}
    # recv_packet and readv_all/writev_all are Rust calls in the selected tree (#443):
    # upstream's runs its two packet helpers, the selected tree its vector_all glue.
    helpers=['ssize_t Socket::recv_packet_with_length_protocol()', 'ssize_t Socket::recv_packet_with_eof_protocol()']
    glue=['ssize_t Socket::vector_all(']
    common=['ssize_t Socket::readv_all(', 'ssize_t Socket::writev_all(', 'ssize_t Socket::recv_packet(double timeout)']
    with tempfile.TemporaryDirectory(prefix='swr-hooks-packet-') as directory:
        tree=Path(directory)
        for path,text in originals.items():
            out=tree/path;out.parent.mkdir(parents=True,exist_ok=True);out.write_text(text)
        for entry in manifest:
            patch=ROOT/'migration/patches/hooks'/entry['path']
            subprocess.run(['git','apply',str(patch)],cwd=tree,check=True)
        for variant in ['original','selected']:
            texts=originals if variant=='original' else {path:(tree/path).read_text() for path in paths}
            signatures=common+(helpers if variant=='original' else glue)
            bodies=[block(texts[unit],signature) for signature in signatures]
            (tree/(variant+'-packet.inc')).write_text('\n'.join(bodies))
        command=['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/hooks'),'-I'+str(ROOT/'tests/modules/hooks'),str(ROOT/'tests/modules/hooks/packet_gate.cc'),str(a.archive.resolve()),'-lpthread','-o',str(tree/'packet')]
        if a.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
        if sys.platform.startswith('linux'):command+=['-ldl','-lm']
        build=subprocess.run(command,capture_output=True,text=True)
        run=subprocess.run([str(tree/'packet')],capture_output=True,text=True) if build.returncode==0 else build
        print(build.stderr,end='');print(run.stdout,end='')
        if run is not build:print(run.stderr,end='')
        report=dict(pin=PIN,scope=__doc__,manifest=manifest,command=command,compile_returncode=build.returncode,compile_stderr=build.stderr,returncode=run.returncode,stdout=run.stdout,stderr=run.stderr)
        a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n')
        raise SystemExit(run.returncode)

if __name__=='__main__':main()
