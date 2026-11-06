#!/usr/bin/env python3
"""Exact deferred/begin/idle/exit bodies: Native vs Rust-owned deferred queue.

Compares the unchanged original assertions plus a positive null/timeout case
across C++14/17/20. Native payload effects remain Native; the
selected side is the hybrid build's tree, whose queue has no Native list. No
destructor observer, allocator hook, recursive execution,
failure/lifetime/shutdown or production/performance proof.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from ordered_gate import ROOT,PIN,block,hybrid
sys.path.insert(0,str(ROOT))
from tools.build_upstream import require_build_space
from tools.harness import run_isolated
from tools.harness import run_compiler
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--report',type=Path,required=True);p.add_argument('--sanitize',action='store_true');a=p.parse_args()
if a.report.exists():raise ValueError('Choose a unique receipt path')
report=dict(scope=__doc__,pin=PIN,commands=[],results=[],passed=False)
with tempfile.TemporaryDirectory(prefix='swr-builtins-') as tmp:
    t=Path(tmp);selected_tree,abi=hybrid(a.archive);report['hybrid_tree']=str(selected_tree)
    original={name:subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+name]).decode() for name in ['include/swoole_reactor.h','src/reactor/base.cc']}
    core=subprocess.check_output(['git','-C',str(a.source),'show',PIN+':include/swoole.h'],text=True);alias=next(line for line in core.splitlines() if line.startswith('typedef std::function<void(void *)> Callback;'))
    header=original['include/swoole_reactor.h'];manager=block(header,header.index('class CallbackManager'))[0]+';';task=block(header,header.index('struct DeferCallback'))[0]+';'
    report['pinned_callback_alias']=alias
    try:
        for variant in ['original','selected']:
            base=original['src/reactor/base.cc'] if variant=='original' else (selected_tree/'src/wrapper/event.cc').read_text()
            cls='namespace swoole {\n'+alias+'\n'+task+'\n'+manager+'''
            class Reactor {public:CallbackManager *defer_tasks=nullptr;DeferCallback idle_task{},future_task{};bool running=true,wait_exit=false,accepted=true,mutate_wait=false;unsigned queries=0;
            bool if_exit(){++queries;if(mutate_wait)wait_exit=false;return accepted;}
            void defer(const Callback&,void *data=nullptr);void execute_begin_callback()const;};\n'''
            if variant!='original':cls+='\n'.join(block(base,base.index(sig))[0] for sig in ['static void defer_into(','static void swr_reactor_run_task(','static void swr_reactor_try_exit(','static bool swr_reactor_exit_predicate('])+'\n'
            for sig in ['void Reactor::defer(','void Reactor::execute_begin_callback(']:cls+=block(base,base.index(sig))[0]+'\n'
            for sig,definition in [
                ('set_end_callback(PRIORITY_DEFER_TASK','static void defer_end(Reactor *reactor)'),
                ('set_exit_condition(EXIT_CONDITION_DEFER_TASK','static bool defer_exit(Reactor *reactor,size_t &event_num)'),
                ('set_end_callback(PRIORITY_IDLE_TASK','static void idle_end(Reactor *reactor)'),
                ('set_end_callback(PRIORITY_TRY_EXIT','static void try_exit(Reactor *reactor)'),
                ('set_exit_condition(EXIT_CONDITION_DEFAULT','static bool default_exit(Reactor *reactor,size_t &event_num)'),
            ]:
                start=base.index(sig);fragment,_=block(base,start);cls+=definition+fragment[fragment.index('{'):]+'\n'
            selected_header=(selected_tree/'include/swoole_reactor.h').read_text()
            source_header=header if variant=='original' else selected_header
            field=('CallbackManager' if variant=='original' else 'swr_v1_reactor_deferred')+' *defer_tasks = nullptr;'
            cls=cls.replace('CallbackManager *defer_tasks=nullptr;', '\n'+field+'\nint timeout_msec=17;'+block(source_header,source_header.index('int get_timeout_msec('))[0]+'\n')
            (t/'builtins.inc').write_text(cls+'}\n')
            for standard in ['c++14','c++17','c++20']:
                binary=t/(variant+'-'+standard);cmd=['c++','-std='+standard,'-O1','-g','-I'+str(t),abi,ROOT/'tests/modules/reactor/builtins.cc',a.archive.resolve(),'-lpthread','-o',binary]
                if variant!='original':cmd+=['-DSWOOLE_RS_REACTOR=1']
                if a.sanitize:cmd+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
                if sys.platform.startswith('linux'):cmd+=['-Wl,--gc-sections','-ldl','-lm']
                for stage,command in [('compile',cmd),('execute',[binary])]:
                    require_build_space(ROOT);require_build_space(t)
                    result=run_compiler(command, timeout=60) if stage=='compile' else vars(run_isolated(list(map(str,command)),cwd=t,timeout=60))
                    report['commands'].append(dict(variant=variant,standard=standard,stage=stage,command=list(map(str,command)),exit=result['returncode'],stdout=result['stdout'],stderr=result['stderr'],cache={k:result[k] for k in ['cache_hit','cache_key','cache_reason'] if k in result}))
                    if result['returncode'] or result['stderr']:raise RuntimeError(variant+' '+stage+' failed or warned')
                report['results'].append(dict(variant=variant,standard=standard,observations=json.loads(result['stdout'])))
        if any(row['observations']!=report['results'][0]['observations'] for row in report['results'][1:]):raise RuntimeError('Native/selected observations differ')
        report['passed']=True
    except Exception as error:report['error']=str(error)
a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:report.get(k) for k in ['passed','error','results']}));raise SystemExit(not report['passed'])
