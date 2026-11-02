#!/usr/bin/env python3
"""Compare actual Socket lifecycle/connection bodies with native callbacks and effects; no kernel/context proof."""
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
    paths=sorted({path for entry in manifest for path in entry['upstream_paths']})
    originals={path:subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+path],text=True) for path in paths}
    signatures=['bool Socket::shutdown(', 'bool Socket::cancel(', 'bool Socket::close()', 'bool Socket::connect(const sockaddr *', 'bool Socket::check_liveness()', 'const char *Socket::get_event_str(']
    inline=['bool is_closed() const', 'bool is_connected() const', 'Coroutine *get_bound_co(', 'bool has_bound(', 'void set_err(const int e)', 'bool is_available(', 'void check_bound_co(', 'long get_bound_cid(']
    with tempfile.TemporaryDirectory(prefix='swr-hooks-lifecycle-') as directory:
        tree=Path(directory)
        for path,text in originals.items():
            out=tree/path;out.parent.mkdir(parents=True,exist_ok=True);out.write_text(text)
        for entry in manifest:
            patch=ROOT/'migration/patches/hooks'/entry['path']
            subprocess.run(['git','apply',str(patch)],cwd=tree,check=True)
        for variant in ['original','selected']:
            texts=originals if variant=='original' else {path:(tree/path).read_text() for path in paths if (tree/path).exists()}
            # cancel/close are Rust calls in the selected tree (#443); its lifecycle_ops passes the effects.
            selected_only=['swr_v1_hook_socket_ops Socket::lifecycle_ops('] if variant=='selected' else []
            bodies=[block(texts[unit],signature)+(';' if signature.startswith('struct ') else '') for signature in signatures+selected_only]
            (tree/(variant+'-lifecycle.inc')).write_text('\n'.join(bodies))
            # The selected header's bound-coroutine accessors over its Rust state (#439).
            inline_selected=['Coroutine *event_co(', 'void bind_co('] if variant=='selected' else []
            (tree/(variant+'-lifecycle-inline.inc')).write_text('\n'.join(block(texts['include/swoole_coroutine_socket.h'],signature) for signature in inline+inline_selected))
        command=['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/hooks'),'-I'+str(ROOT/'tests/modules/hooks'),str(ROOT/'tests/modules/hooks/lifecycle_gate.cc'),str(a.archive.resolve()),'-lpthread','-o',str(tree/'lifecycle')]
        if a.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
        if sys.platform.startswith('linux'):command+=['-ldl','-lm']
        build=subprocess.run(command,capture_output=True,text=True)
        run=subprocess.run([str(tree/'lifecycle')],capture_output=True,text=True) if build.returncode==0 else build
        print(build.stderr,end='');print(run.stdout,end='')
        if run is not build:print(run.stderr,end='')
        report=dict(pin=PIN,scope=__doc__,manifest=manifest,command=command,compile_returncode=build.returncode,compile_stderr=build.stderr,returncode=run.returncode,stdout=run.stdout,stderr=run.stderr)
        a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n')
        raise SystemExit(run.returncode)

if __name__=='__main__':main()
