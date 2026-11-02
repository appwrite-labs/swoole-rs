#!/usr/bin/env python3
"""Whole Rust-owned IOVector storage: exact positive original/owned source consumers.

No allocation failures, foreign pointers, shallow copies, reentry, lifetime faults,
network or performance probes.
"""
import argparse,io,json,subprocess,sys,tarfile,tempfile,shutil
from pathlib import Path
from ordered_gate import ROOT,PIN,block,hybrid
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import apply_patch_verified,hybrid_tree
from tools.build_upstream import require_build_space
from tools.harness import run_isolated
from tools.harness import run_compiler
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--sanitize',action='store_true');a=p.parse_args()
if a.report.exists():raise ValueError('Choose a unique immutable receipt')
dest='src/network/socket.cc';header='include/swoole_socket.h'
selected_tree,abi=hybrid(a.archive);hybrid_flags=hybrid_tree(a.archive)[1]
hybrid_corrected=json.loads((selected_tree.parent/'build-manifest.json').read_text())['inputs']['fix_halfclose']
fixed=ROOT/'migration/patches/halfclose/0003-send-tls-close-notify.patch'
report=dict(assigned_to='@loks0n',scope=__doc__,pin=PIN,hybrid_tree=str(selected_tree),hybrid_corrected=hybrid_corrected,patch_application=[],sources={},syntax=[],commands=[],results=[],passed=False)
with tempfile.TemporaryDirectory(prefix='swr-iov-owned-') as temporary:
 t=Path(temporary)
 try:
  require_build_space(ROOT);require_build_space(t)
  sdk=t/'sdk';raw=subprocess.check_output(['git','-C',str(a.source),'archive',PIN,'include','thirdparty'])
  with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
   for member in archive:
    if member.isfile():path=sdk/member.name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(archive.extractfile(member).read())
  original={name:subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+name]) for name in [dest,header]}
  for name,corrected,selected in [('native',False,False),('native-corrected',True,False),('rust',hybrid_corrected,True)]:
   tree=t/name;root=selected_tree if selected else tree
   if not selected:
    shutil.copytree(sdk,tree)
    for path,data in original.items():file=tree/path;file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(data)
    if corrected:report['patch_application'].append(dict(variant=name,proof=apply_patch_verified(tree,fixed,[dest])))
   else:tree.mkdir()
   body=(root/dest).read_text();h=(root/header).read_text();report['sources'][name]={'selected':selected,'optional_native_fix':corrected}
   declaration=block(h,h.index('struct IOVector {'))[0]
   if selected:
    # The public IOVector getters, layout and fields are frozen; only length's body may differ.
    pinned=original[header].decode();old_decl=block(pinned,pinned.index('struct IOVector {'))[0]
    old_length=block(old_decl,old_decl.index('size_t length()'))[0];new_length=block(declaration,declaration.index('size_t length()'))[0]
    if declaration.replace(new_length,old_length,1)!=old_decl:raise ValueError('Unrelated IOVector public bytes changed')
   options=['-DSWOOLE_RS_REACTOR'] if selected else []
   for ssl in [False,True]:
    cmd=['c++','-std=c++14','-fsyntax-only',*(hybrid_flags if selected else []),*(['-DSW_USE_OPENSSL'] if ssl else []),'-I'+str(root/'include'),'-I'+str(root),*subprocess.check_output(['pkg-config','--cflags','openssl'],text=True).split(),str(root/dest)]
    r=run_isolated(cmd,cwd=t,timeout=60);report['syntax'].append(dict(variant=name,ssl=ssl,command=cmd,exit=r.returncode,stdout=r.stdout,stderr=r.stderr))
    if r.returncode or r.stderr:raise RuntimeError(name+' complete SDK syntax failed/warned')
   (tree/'iov_owned.inc').write_text(declaration+';\n'+'\n'.join(block(body,body.index(signature))[0] for signature in ['IOVector::IOVector(','IOVector::~IOVector(','void IOVector::update_iterator(']))
   for standard in ['c++14','c++17','c++20']:
    require_build_space(ROOT);require_build_space(t)
    binary=tree/('consumer-'+standard);cmd=['c++','-std='+standard,'-Werror','-O1','-g',*options,'-I'+str(tree),abi,str(ROOT/'tests/modules/reactor/iov_owned.cc'),str(a.archive.resolve()),'-lpthread','-o',str(binary)]
    if a.sanitize:cmd+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
    if sys.platform.startswith('linux'):cmd+=['-ldl','-lm']
    for stage,command in [('compile',cmd),('execute',[str(binary)])]:
     require_build_space(t)
     r=run_compiler(command, timeout=60) if stage=='compile' else vars(run_isolated(command,cwd=t,timeout=60))
     report['commands'].append(dict(variant=name,standard=standard,stage=stage,command=command,exit=r['returncode'],stdout=r['stdout'],stderr=r['stderr']))
     if r['returncode'] or r['stderr']:raise RuntimeError(name+' '+standard+' '+stage+' failed/warned')
    report['results'].append(dict(variant=name,standard=standard,observations=json.loads(r['stdout'])))
  if len({json.dumps(row['observations'],sort_keys=True) for row in report['results']})!=1:raise RuntimeError('Positive original/owned observations differ')
  report['passed']=True
 except Exception as error:report['error']=str(error)
a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:report.get(k) for k in ['passed','error','results']}));raise SystemExit(not report['passed'])
