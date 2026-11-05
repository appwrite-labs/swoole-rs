#!/usr/bin/env python3
"""Build isolated reference/scoped Rust locks ZTS extensions from immutable pin."""
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile

ROOT=Path('/work')
import sys
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import module_patches
from tools.build_upstream import COMMIT as PIN
source=Path('/reference')
record=json.loads((ROOT/'target/linux-dev/target/hybrid/dev-rust/build-manifest.json').read_text())
prefix=ROOT/'target/linux-dev'/Path(record['extension']).parents[2].relative_to('/work')/'rust-prefix'
archive=prefix/'lib/libswoole_rs_ffi.a'
base=ROOT/'target/zts-build'
base.mkdir(parents=True,exist_ok=True)
packed=subprocess.check_output(['git','-C',str(source),'archive',PIN])
report={'pin':PIN,'php':subprocess.check_output(['php','-v'],text=True),'engines':[],'passed':False}
manifests={name:module_patches(ROOT/'migration/patches'/name) for name in ['integration','locks']}
try:
    for name in ['reference','rust']:
        tree=base/name;tree.mkdir(exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(packed)) as tar:
            for member in tar:
                name_path=Path(member.name)
                if name_path.is_absolute() or '..' in name_path.parts:raise ValueError('Unsafe archive path')
                if member.isfile():
                    path=tree/name_path;path.parent.mkdir(parents=True,exist_ok=True);data=tar.extractfile(member).read()
                    if not path.exists() or path.read_bytes()!=data:path.write_bytes(data);path.chmod(member.mode)
        if name=='rust':
            subprocess.run(['git','init','-q',str(tree)],check=True)
            for module,manifest in manifests.items():
                for patch in manifest:
                    path=ROOT/'migration/patches'/module/patch['path']
                    subprocess.run(['git','apply','--check',str(path)],cwd=tree,check=True)
                    subprocess.run(['git','apply',str(path)],cwd=tree,check=True)
        env={**os.environ}
        extra=['--with-swoole-rs='+str(prefix)] if name=='rust' else []
        for stage,cmd in [('phpize',['phpize']),('configure',['./configure','--enable-swoole','--enable-swoole-thread','--enable-sockets','--disable-brotli','--disable-zstd',*extra]),('build',['make','-j2'])]:
            log=base/(name+'-'+stage+'.log')
            with log.open('w') as out:subprocess.run(cmd,cwd=tree,env=env,stdout=out,stderr=subprocess.STDOUT,check=True)
        extension=tree/'modules/swoole.so'
        check=subprocess.check_output(['php','-n','-d','extension=sockets','-d','extension='+str(extension),'-r','echo json_encode([PHP_ZTS, extension_loaded("swoole"),class_exists("Swoole\\\\Thread\\\\Lock"),defined("SWOOLE_RS_FEATURE_BITS")?SWOOLE_RS_FEATURE_BITS:0]);'],text=True)
        if json.loads(check) != [True, True, True, 1024 if name=="rust" else 0]:raise ValueError('ZTS thread-lock capability missing: '+check)
        report['engines'].append({'name':name,'load':check,'extension':str(extension)})
    report['passed']=True
finally:
    (base/'build.json').write_text(json.dumps(report,indent=2)+'\n')
