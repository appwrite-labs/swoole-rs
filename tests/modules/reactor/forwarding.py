#!/usr/bin/env python3
"""Positive native Socket forwarding effects and full-TU capability checks.

The queued-output policies are tested directly by output::tests in the Rust
crate. This remaining fixture covers forwarding decisions that still live in
C++; it does not execute clean (covered by socket_io's socketpair test).
"""
import argparse,io,json,subprocess,sys,tarfile,tempfile
from pathlib import Path
from ordered_gate import ROOT,PIN,block,hybrid
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import apply_patch_verified,hybrid_tree
from tools.build_upstream import require_build_space
from tools.harness import run_isolated
from tools.harness import run_compiler
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--sanitize',action='store_true');a=p.parse_args()
if a.report.exists():raise ValueError('Choose a unique receipt path')
fixed=ROOT/'migration/patches/halfclose/0003-send-tls-close-notify.patch'
dest='src/network/socket.cc'
selected_tree,abi=hybrid(a.archive);hybrid_flags=hybrid_tree(a.archive)[1]
hybrid_corrected=json.loads((selected_tree.parent/'build-manifest.json').read_text())['inputs']['fix_halfclose']
fixture_name='forwarding'
report=dict(hybrid_tree=str(selected_tree),hybrid_corrected=hybrid_corrected,scope=__doc__,pin=PIN,patch_application=[],syntax=[],commands=[],results=[],passed=False)
a.report.parent.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix='swr-forwarding-',dir=a.report.parent) as tmp:
 t=Path(tmp)
 try:
  require_build_space(ROOT);require_build_space(t)
  source=subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+dest])
  headers=subprocess.check_output(['git','-C',str(a.source),'archive',PIN,'include','thirdparty'])
  sdk=t/'sdk'
  with tarfile.open(fileobj=io.BytesIO(headers)) as archive:
   for member in archive:
    if member.isfile():path=sdk/member.name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(archive.extractfile(member).read())
  for variant,corrected,selected in [('native',False,False),('native-corrected',True,False),('rust',hybrid_corrected,True)]:
   tree=t/variant
   if selected:file=selected_tree/dest;overlaid=selected_tree
   else:
    file=tree/dest;file.parent.mkdir(parents=True);file.write_bytes(source);overlaid=sdk
    if corrected:report['patch_application'].append(dict(variant=variant,proof=apply_patch_verified(tree,fixed,[dest])))
   body=file.read_text()
   for ssl in [False,True]:
    command=['c++','-std=c++14','-fsyntax-only',*(hybrid_flags if selected else []),'-I'+str(overlaid/'include'),'-I'+str(overlaid),*subprocess.check_output(['pkg-config','--cflags','openssl'],text=True).split()]
    if ssl:command+=['-DSW_USE_OPENSSL']
    command+=[str(file)];result=run_isolated(command,cwd=t,timeout=60)
    report['syntax'].append(dict(variant=variant,ssl=ssl,command=command,exit=result.returncode,stdout=result.stdout,stderr=result.stderr))
    if result.returncode or result.stderr:raise RuntimeError(variant+' full Socket syntax failed/warned')
   if selected:
    disabled=[x for x in command if x!='-DSWOOLE_RS_REACTOR'];result=run_isolated(disabled,cwd=t,timeout=60)
    refused=result.returncode!=0 and 'Reactor overlay requires --enable-swoole-rs-reactor' in result.stderr
    report['syntax'].append(dict(variant=variant+'-disabled',command=disabled,exit=result.returncode,stderr=result.stderr,expected_refusal=refused))
    if not refused:raise RuntimeError('Disabled selected Socket not refused')
   extracted='\n'.join(block(body,body.index(sig))[0] for sig in ['ssize_t Socket::sendfile(','ssize_t Socket::send_async(','bool Socket::check_liveness('])
   tree.mkdir(exist_ok=True);(tree/(fixture_name+'.inc')).write_text(extracted);binary=tree/'consumer';command=['c++','-std=c++14','-O1','-g','-I'+str(tree),abi,str(ROOT/('tests/modules/reactor/'+fixture_name+'.cc')),str(a.archive.resolve()),'-lpthread','-o',str(binary)]
   if a.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
   if sys.platform.startswith('linux'):command+=['-ldl','-lm']
   for stage,cmd in [('compile',command),('execute',[str(binary)])]:
    require_build_space(ROOT);require_build_space(t)
    result=run_compiler(cmd, timeout=60) if stage=='compile' else vars(run_isolated(cmd,cwd=t,timeout=60))
    report['commands'].append(dict(variant=variant,stage=stage,command=cmd,exit=result['returncode'],stdout=result['stdout'],stderr=result['stderr'],cache={k:result[k] for k in ['cache_hit','cache_key','cache_reason'] if k in result}))
    if result['returncode'] or result['stderr']:raise RuntimeError(variant+' '+stage+' failed/warned')
   report['results'].append(dict(variant=variant,observations=json.loads(result['stdout'])))
  if len({json.dumps(row['observations'],sort_keys=True) for row in report['results']})!=1:raise RuntimeError('Positive observations differ')
  report['passed']=True
 except Exception as error:report['error']=str(error)
a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:report.get(k) for k in ['passed','error','results']}));raise SystemExit(not report['passed'])
