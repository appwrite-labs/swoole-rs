#!/usr/bin/env python3
"""Actual add_destroy_callback with inert FIFO copies only; the destructor caller is never executed."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from ordered_gate import ROOT, PIN, block, hybrid
from tools.build_upstream import require_build_space
from tools.harness import run_compiler
from tools.harness import run_isolated
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--sanitize',action='store_true');a=p.parse_args()
if a.report.exists():raise ValueError('Choose a unique receipt path')
report=dict(scope=__doc__,assigned_to='@loks0n',pin=PIN,sources={},commands=[],results=[],passed=False,actual_reactor_destructor_executed=False)
with tempfile.TemporaryDirectory(prefix='swr-inert-queues-') as temporary:
    tree=Path(temporary);selected_tree,abi=hybrid(a.archive);report['hybrid_tree']=str(selected_tree)
    original={name:subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+name]).decode() for name in ['include/swoole_reactor.h','src/reactor/base.cc']}
    core=subprocess.check_output(['git','-C',str(a.source),'show',PIN+':include/swoole.h'],text=True)
    alias=next(line for line in core.splitlines() if line.startswith('typedef std::function<void(void *)> Callback;'))
    report['pinned_callback_alias']=alias
    manager=block(original['include/swoole_reactor.h'],original['include/swoole_reactor.h'].index('class CallbackManager'))[0]+';'
    try:
        for variant in ['native','selected']:
            selected=variant!='native';header=(selected_tree/'include/swoole_reactor.h').read_text() if selected else original['include/swoole_reactor.h'];base=(selected_tree/'src/reactor/base.cc').read_text() if selected else original['src/reactor/base.cc']
            queue='ReactorDeferredQueue' if selected else 'CallbackManager'
            field=queue+' *defer_tasks = nullptr;\n    '+queue+' destroy_callbacks;'
            method=block(base,base.index('void Reactor::add_destroy_callback('))[0]
            (tree/'callback-queues.inc').write_text('namespace swoole {\n'+alias+'\n'+manager+'\nclass Reactor {public:\n'+field+'\nvoid add_destroy_callback(const Callback&,void *);};\n'+method+'\n}\n')
            for standard in ['c++14','c++17','c++20']:
                binary=tree/(variant+'-'+standard);command=['c++','-std='+standard,'-O1','-g','-I'+str(tree),abi,ROOT/'tests/modules/reactor/callback_queues.cc',a.archive.resolve(),'-lpthread','-o',binary]
                if selected:command+=['-DSWOOLE_RS_REACTOR=1']
                if a.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
                if sys.platform.startswith('linux'):command+=['-ldl','-lm']
                for stage,cmd in [('compile',command),('execute',[binary])]:
                    require_build_space(ROOT);require_build_space(tree)
                    result=run_compiler(cmd) if stage=='compile' else vars(run_isolated(list(map(str,cmd)),cwd=tree,timeout=60))
                    report['commands'].append(dict(variant=variant,standard=standard,stage=stage,command=list(map(str,cmd)),exit=result['returncode'],stdout=result['stdout'],stderr=result['stderr']))
                    if result['returncode'] or result['stderr']:raise RuntimeError(variant+' '+stage+' failed or warned')
                report['results'].append(dict(variant=variant,standard=standard,observations=json.loads(result['stdout'])))
        if any(row['observations']!=report['results'][0]['observations'] for row in report['results']):raise RuntimeError('Inert observations differ')
        report['passed']=True
    except Exception as error:report['error']=str(error)
a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({key:report.get(key) for key in ['passed','error']}));raise SystemExit(not report['passed'])
