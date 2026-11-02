#!/usr/bin/env python3
"""Exact writable/drain bodies with positive Native effect facades only.

No DATA destroy callbacks, native faults, lifetime/fork/fd-reuse/network probes.
This is orchestration qualification, not actual socket/Buffer production parity.
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    overlay = ROOT/'migration/patches/reactor'
    entries = module_patches(overlay)
    original = subprocess.check_output(['git','-C',str(args.source),'show',PIN+':src/reactor/base.cc'],text=True)
    with tempfile.TemporaryDirectory(prefix='swr-flush-') as temporary:
        tree = Path(temporary)
        for entry in entries:
            for name in entry['upstream_paths']:
                path = tree/name
                if not path.exists():
                    path.parent.mkdir(parents=True,exist_ok=True)
                    path.write_bytes(subprocess.check_output(['git','-C',str(args.source),'show',PIN+':'+name]))
        patches = []
        for entry in entries:
            patch = overlay/entry['path']
            application = apply_patch_verified(tree, patch, entry['upstream_paths'], entry['sha256'])
            patches.append(dict(path=entry['path'],application=application))
        selected = (tree/'src/reactor/base.cc').read_text()
        fragments = []
        for label, text in [('Original',original),('Selected',selected)]:
            name = 'Reactor'+label
            methods = []
            if label == 'Selected':
                # The selected bodies' native effects for Rust's flush and drain.
                start = text.index('namespace {\n// The native effects of Rust')
                methods.append(text[start:text.index('}  // namespace\n', start) + 16].replace('Reactor', name))
            for signature in ['int Reactor::_writable_callback(', 'void Reactor::drain_write_buffer(']:
                body, _ = block(text,text.index(signature))
                methods.append(body.replace('Reactor',name))
            fragments.append('struct '+name+''' {
                static int _writable_callback('''+name+'''*, Event*);
                void drain_write_buffer(Socket*);
                int close('''+name+'''*, Socket *socket) { effects.emplace_back("close"); socket->removed=true; socket->out_buffer->clear(); return SW_OK; }
                int trigger_close_event(Event*) { effects.emplace_back("trigger-close"); return SW_OK; }
                int remove_write_event(Socket*) { effects.emplace_back("remove-write"); return SW_OK; }
            };\n'''+ '\n'.join(methods))
        (tree/'flush.inc').write_text('\n'.join(fragments))
        binary = tree/'flush'
        command = ['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/reactor'),str(ROOT/'tests/modules/reactor/flush.cc'),str(args.archive.resolve()),'-lpthread','-o',str(binary)]
        if args.sanitize: command += ['-fsanitize=address,undefined','-fno-omit-frame-pointer']
        if sys.platform.startswith('linux'): command += ['-ldl','-lm']
        result = subprocess.run(command,capture_output=True,text=True)
        report = dict(pin=PIN,scope=__doc__,patches=patches,command=command,compile_returncode=result.returncode,compile_stdout=result.stdout,compile_stderr=result.stderr)
        if result.returncode == 0:
            result = subprocess.run([str(binary)],capture_output=True,text=True)
            report.update(returncode=result.returncode,stdout=result.stdout,stderr=result.stderr)
            if result.returncode == 0: report['observations']=json.loads(result.stdout)
        args.report.parent.mkdir(parents=True,exist_ok=True)
        args.report.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({key:report[key] for key in ['compile_returncode','observations'] if key in report}))
        return result.returncode


if __name__ == '__main__':
    raise SystemExit(main())
