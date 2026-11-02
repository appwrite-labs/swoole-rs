#!/usr/bin/env python3
"""Complete pinned/selected wait bodies with positive Native effect facades.

Actual target poll and kqueue on macOS; poll and epoll on Linux.
No native error/fault, allocation, callback reentry, fd-reuse or lifetime probes.
Not actual kernel/Socket production or whole-module parity qualification.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from ordered_gate import ROOT,PIN,block, apply_patch_verified
from tools.build_hybrid import module_patches


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',type=Path,required=True)
    parser.add_argument('--archive',type=Path,required=True)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--sanitize',action='store_true')
    args=parser.parse_args()
    if args.report.exists():raise ValueError('Choose a unique wait receipt path')
    overlay=ROOT/'migration/patches/reactor';entries=module_patches(overlay)
    originals={}
    with tempfile.TemporaryDirectory(prefix='swr-wait-') as temporary:
        tree=Path(temporary)
        for entry in entries:
            for name in entry['upstream_paths']:
                path=tree/name
                if not path.exists():
                    path.parent.mkdir(parents=True,exist_ok=True)
                    content=subprocess.check_output(['git','-C',str(args.source),'show',PIN+':'+name])
                    path.write_bytes(content);originals[name]=content.decode()
        patches=[]
        for entry in entries:
            patch=overlay/entry['path']
            application = apply_patch_verified(tree, patch, entry['upstream_paths'], entry['sha256'])
            patches.append(dict(path=entry['path'],application=application))
        fragments=[]
        for backend in ['poll', 'epoll' if sys.platform.startswith('linux') else 'kqueue']:
            original_type={'poll':'ReactorPoll','epoll':'ReactorEpoll','kqueue':'ReactorKqueue'}[backend]
            path='src/reactor/'+backend+'.cc'
            selected=(tree/path).read_text()
            for label,text in [('Original',originals[path]),('Selected',selected)]:
                name=backend.capitalize()+label
                methods=[]
                body,_=block(text,text.index('int '+original_type+'::wait()'))
                body=body.replace('int '+original_type+'::wait()', 'int wait()')
                body=body.replace('struct kevent','NativeKevent').replace('::kevent(', 'native_kevent(').replace('epoll_wait(', 'native_epoll_wait(').replace('poll(events_', 'native_poll(events_').replace('::poll(', '::native_poll(')
                if backend=='kqueue':
                    helpers=['bool fetch_event(', 'void del_once_socket('] if label=='Original' else []
                    body+='\n'+'\n'.join(block(text,text.index(signature))[0] for signature in helpers)
                    if label=='Selected':
                        signal=block(text,text.index('static void swr_reactor_dispatch_signal('))[0]
                        fragments.append(signal)
                array={'poll':'pollfd *events_=poll_events; Socket **event_sockets_=poll_sockets;', 'kqueue':'NativeKevent *events_=kqueue_events;', 'epoll':'epoll_event *events_=epoll_events;'}[backend]
                fragments.append('struct '+name+' : WaitBase { using WaitBase::WaitBase;'+array+'\n'+body+'\n};')
        (tree/'wait.inc').write_text('\n'.join(fragments))
        binary=tree/'wait'
        command=['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/reactor'),str(ROOT/'tests/modules/reactor/wait.cc'),str(args.archive.resolve()),'-lpthread','-o',str(binary)]
        if args.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
        if sys.platform.startswith('linux'):command+=['-ldl','-lm']
        result=subprocess.run(command,capture_output=True,text=True)
        report=dict(pin=PIN,scope=__doc__,patches=patches,command=command,compile_returncode=result.returncode,compile_stdout=result.stdout,compile_stderr=result.stderr)
        if result.returncode==0:
            result=subprocess.run([str(binary)],capture_output=True,text=True)
            report.update(returncode=result.returncode,stdout=result.stdout,stderr=result.stderr)
            if result.returncode==0:report['observations']=json.loads(result.stdout)
        args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({key:report[key] for key in ['compile_returncode','observations'] if key in report}))
        return result.returncode


if __name__=='__main__':raise SystemExit(main())
