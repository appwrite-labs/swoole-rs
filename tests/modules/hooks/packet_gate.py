#!/usr/bin/env python3
"""Compare actual vector all-transfer and packet length/EOF/dispatch bodies with native buffers and protocol callbacks; no kernel/context proof."""
import argparse
import json
import shlex
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
    # Upstream's socket.cc; the hybrid keeps the vector glue in system.cc.
    unit,home='src/coroutine/socket.cc','src/coroutine/system.cc'
    originals,selected,hybrid=sources(a.source,a.archive,[unit,home])
    # Packet framing calls the record API through the recording native host;
    # only the vector cases still use the selected tree's vector_all glue.
    helpers=['ssize_t Socket::recv_packet_with_length_protocol()', 'ssize_t Socket::recv_packet_with_eof_protocol()']
    glue=['ssize_t Socket::vector_all(']
    common=['ssize_t Socket::readv_all(', 'ssize_t Socket::writev_all(']
    a.report.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='swr-hooks-packet-',dir=a.report.parent) as directory:
        tree=Path(directory)
        for variant in ['original','selected']:
            text=originals[unit] if variant=='original' else selected[home]
            signatures=common+(helpers+['ssize_t Socket::recv_packet(double timeout)'] if variant=='original' else glue)
            bodies=[block(text,signature) for signature in signatures]
            if variant=='selected':bodies.append('#include "packet_host.h"')
            (tree/(variant+'-packet.inc')).write_text('\n'.join(bodies))
        command=['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/hooks'),'-I'+str(ROOT/'native/include'),'-I'+str(ROOT/'native/reactor'),'-I'+str(a.source/'include'),'-I'+str(a.source),*shlex.split(subprocess.check_output(['pkg-config','--cflags','openssl'],text=True)),'-I'+str(ROOT/'tests/modules/hooks'),str(ROOT/'tests/modules/hooks/packet_gate.cc'),str(a.archive.resolve()),'-lssl','-lcrypto','-lpthread','-o',str(tree/'packet')]
        if a.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
        # --gc-sections: the archive's C++ references (Socket::send_async's event loop) are unreachable here.
        if sys.platform.startswith('linux'):command+=['-Wl,--gc-sections','-ldl','-lm']
        build=subprocess.run(command,capture_output=True,text=True)
        run=subprocess.run([str(tree/'packet')],capture_output=True,text=True) if build.returncode==0 else build
        print(build.stderr,end='');print(run.stdout,end='')
        if run is not build:print(run.stderr,end='')
        report=dict(pin=PIN,scope=__doc__,hybrid_tree=hybrid,command=command,compile_returncode=build.returncode,compile_stderr=build.stderr,returncode=run.returncode,stdout=run.stdout,stderr=run.stderr)
        a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n')
        raise SystemExit(run.returncode)

if __name__=='__main__':main()
