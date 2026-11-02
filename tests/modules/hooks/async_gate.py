#!/usr/bin/env python3
"""Compare actual asynchronous event/lambda/sleep/DNS bodies with native effects; no real worker/context/kernel proof."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from ordered_gate import block

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
    unit='src/coroutine/system.cc'
    manifest=json.loads((ROOT/'migration/patches/hooks/manifest.json').read_text())
    paths=sorted({path for entry in manifest['patches'] for path in entry['upstream_paths']})
    originals={path:subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+path],text=True) for path in paths}
    signatures=['static void async_task_completed(', 'bool async(async::Handler handler,', 'struct AsyncLambdaTask {', 'static void async_lambda_handler(', 'static void async_lambda_callback(', 'bool async(const std::function<void()> &fn)', 'int System::sleep(', 'std::string gethostbyname_impl_with_async(', 'std::vector<std::string> System::getaddrinfo(']
    with tempfile.TemporaryDirectory(prefix='swr-hooks-async-') as directory:
        tree=Path(directory)
        for path,text in originals.items():
            out=tree/path;out.parent.mkdir(parents=True,exist_ok=True);out.write_text(text)
        for entry in manifest['patches']:
            patch=ROOT/'migration/patches/hooks'/entry['path']
            assert hashlib.sha256(patch.read_bytes()).hexdigest()==entry['sha256']
            subprocess.run(['git','apply',str(patch)],cwd=tree,check=True)
        for variant in ['original','selected']:
            texts=originals if variant=='original' else {path:(tree/path).read_text() for path in paths if (tree/path).exists()}
            bodies=[block(texts[unit],signature)+(';' if signature.startswith('struct ') else '') for signature in signatures]
            (tree/(variant+'-async.inc')).write_text('\n'.join(bodies))
        command=['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/hooks'),'-I'+str(ROOT/'tests/modules/hooks'),str(ROOT/'tests/modules/hooks/async_gate.cc'),str(a.archive.resolve()),'-lpthread','-o',str(tree/'async')]
        if a.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
        if sys.platform.startswith('linux'):command+=['-ldl','-lm']
        build=subprocess.run(command,capture_output=True,text=True)
        run=subprocess.run([str(tree/'async')],capture_output=True,text=True) if build.returncode==0 else build
        print(build.stderr,end='');print(run.stdout,end='')
        if run is not build:print(run.stderr,end='')
        report=dict(pin=PIN,scope=__doc__,manifest=manifest,command=command,compile_returncode=build.returncode,compile_stderr=build.stderr,returncode=run.returncode,stdout=run.stdout,stderr=run.stderr)
        a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n')
        raise SystemExit(run.returncode)

if __name__=='__main__':main()
