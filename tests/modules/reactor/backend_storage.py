#!/usr/bin/env python3
"""Actual epoll/kqueue constructor/destructor and positive kernel pipe readiness.

No allocation-failure, fault, callback, lifetime/fd-reuse or network probes.
No full reactor/production/performance qualification is inferred.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from ordered_gate import ROOT, PIN, block, apply_patch_verified
from tools.build_hybrid import module_patches


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',type=Path,required=True)
    parser.add_argument('--archive',type=Path,required=True)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--sanitize',action='store_true')
    args=parser.parse_args()
    backend='epoll' if sys.platform.startswith('linux') else 'kqueue'
    native='ReactorEpoll' if backend=='epoll' else 'ReactorKqueue'
    path='src/reactor/'+backend+'.cc'
    original=subprocess.check_output(['git','-C',str(args.source),'show',PIN+':'+path],text=True)
    overlay=ROOT/'migration/patches/reactor'
    entries=module_patches(overlay)
    with tempfile.TemporaryDirectory(prefix='swr-backend-storage-') as temporary:
        tree=Path(temporary)
        for entry in entries:
            for name in entry['upstream_paths']:
                output=tree/name
                if not output.exists():
                    output.parent.mkdir(parents=True,exist_ok=True)
                    output.write_bytes(subprocess.check_output(['git','-C',str(args.source),'show',PIN+':'+name]))
        patches=[]
        for entry in entries:
            patch=overlay/entry['path']
            application = apply_patch_verified(tree, patch, entry['upstream_paths'], entry['sha256'])
            patches.append(dict(path=entry['path'],application=application))
        selected=(tree/path).read_text()
        fragments=[]
        for label,text in [('Original',original),('Selected',selected)]:
            name='Backend'+label
            storage='swr_v1_reactor_'+backend+'_events *rs_events_=nullptr;' if label=='Selected' else ''
            fragments.append('struct '+name+' : ReactorImpl { int epfd_; int event_max_; NativeEvent *events_=nullptr; '+storage+' '+name+'(Reactor*,int); ~'+name+'(); bool ready(); };')
            for signature in [native+'::'+native+'(',native+'::~'+native+'(','bool '+native+'::ready(']:
                method,_=block(text,text.index(signature))
                fragments.append(method.replace(native,name))
        (tree/'backend-storage.inc').write_text('\n'.join(fragments))
        binary=tree/'backend-storage'
        command=['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/reactor'),str(ROOT/'tests/modules/reactor/backend_storage.cc'),str(args.archive.resolve()),'-lpthread','-o',str(binary)]
        if args.sanitize: command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
        if sys.platform.startswith('linux'): command+=['-ldl','-lm']
        result=subprocess.run(command,capture_output=True,text=True)
        report=dict(pin=PIN,backend=backend,scope=__doc__,patches=patches,command=command,compile_returncode=result.returncode,compile_stdout=result.stdout,compile_stderr=result.stderr)
        if result.returncode==0:
            result=subprocess.run([str(binary)],capture_output=True,text=True)
            report.update(returncode=result.returncode,stdout=result.stdout,stderr=result.stderr)
            if result.returncode==0: report['observations']=json.loads(result.stdout)
        args.report.parent.mkdir(parents=True,exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({key:report[key] for key in ['backend','compile_returncode','observations'] if key in report}))
        return result.returncode


if __name__=='__main__':
    raise SystemExit(main())
