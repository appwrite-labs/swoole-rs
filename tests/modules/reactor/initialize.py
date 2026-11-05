#!/usr/bin/env python3
"""Exact constructor bodies with positive ready Native-effect facades.

Backend/type selection, field setup, hook/callback installation and fresh main
thread observations only. No kernel backend creation, failure, payload execution,
shutdown, allocator-hook, lifetime or production/performance qualification.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from ordered_gate import ROOT,PIN,block,hybrid
sys.path.insert(0,str(ROOT))
from tools.harness import run_isolated
from tools.harness import run_compiler
from tools.build_upstream import require_build_space
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True)
p.add_argument('--report',type=Path,required=True);p.add_argument('--sanitize',action='store_true');a=p.parse_args()
if a.report.exists():raise ValueError('Choose a new receipt path')
report=dict(scope=__doc__,pin=PIN,results=[],commands=[],passed=False)
with tempfile.TemporaryDirectory(prefix='swr-initialize-') as tmp:
    t=Path(tmp);selected_tree,abi=hybrid(a.archive);report['hybrid_tree']=str(selected_tree)
    original={name:subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+name]).decode() for name in ['include/swoole_reactor.h','src/reactor/base.cc']}
    header=original['include/swoole_reactor.h']
    enums='\n'.join(block(header,header.index('enum '+name))[0]+';' for name in ['Type','EndCallback','ExitCondition'])
    cls='''class Reactor {public:ENUMS
        Reactor(int,Type);Type type_;ReactorImpl *impl=nullptr;bool ready_=false,running=false,wait_exit=false;
        int32_t timeout_msec=0;Task idle_task{},future_task{};CallbackManager *defer_tasks=nullptr;
        time_t last_malloc_trim_time=0;size_t event_num=0;
        using Effect=void(*)();Effect write=nullptr,writev=nullptr,close=nullptr,default_write_handler=nullptr;
        static void _write(){}static void _writev(){}static void _close(){}static void _writable_callback(){}
        void set_default_write_handler(Effect h){default_write_handler=h;}Reactor *handle(){return this;}
        std::map<int,std::function<void(Reactor*)>> ends;
        std::map<int,std::function<bool(Reactor*,size_t&)>> exits;
        void set_end_callback(EndCallback key,const std::function<void(Reactor*)>&f){trace.push_back(40+key);ends[key]=f;}
        void set_exit_condition(ExitCondition key,const std::function<bool(Reactor*,size_t&)>&f){trace.push_back(50+key);exits[key]=f;}
        bool if_exit(){return true;}
    };'''.replace('ENUMS',enums)
    (t/'initialize-class.inc').write_text(cls)
    try:
        for variant in ['original','selected']:
            base=original['src/reactor/base.cc'] if variant=='original' else (selected_tree/'src/reactor/base.cc').read_text()
            helpers=''
            if variant=='selected' and 'static void swr_reactor_run_defer(' in base:
                helpers='\n'.join(block(base,base.index(sig))[0] for sig in ['static void swr_reactor_run_defer(','static void swr_reactor_run_task(','static void swr_reactor_try_exit(','static bool swr_reactor_exit_predicate('])+'\n'
            (t/'initialize-method.inc').write_text(helpers+block(base,base.index('Reactor::Reactor(int max_event, Type _type)'))[0])
            # Independent compile-time capability profiles; these are source
            # facades, never pretend non-target kernel implementations.
            for caps in range(4):
                for trim in range(2):
                    label=f'{variant}-{caps}-{trim}';binary=t/label
                    cmd=['c++','-std=c++14','-O1','-g','-I'+str(t),abi,str(ROOT/'tests/modules/reactor/initialize.cc'),str(a.archive.resolve()),'-lpthread','-o',str(binary)]
                    if caps&1:cmd+=['-DHAVE_EPOLL']
                    if caps&2:cmd+=['-DHAVE_KQUEUE']
                    if trim:cmd+=['-DSW_USE_MALLOC_TRIM']
                    if a.sanitize:cmd+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
                    if sys.platform.startswith('linux'):cmd+=['-ldl','-lm']
                    for stage,command in [('compile',cmd),('execute',[str(binary)])]:
                        require_build_space(t)
                        result=run_compiler(command, timeout=60) if stage=='compile' else vars(run_isolated(command,cwd=t,timeout=60))
                        report['commands'].append(dict(profile=label,stage=stage,command=command,exit=result['returncode'],stdout=result['stdout'],stderr=result['stderr'],cache={k:result[k] for k in ['cache_hit','cache_key','cache_reason'] if k in result}))
                        if result['returncode'] or result['stderr']:raise RuntimeError(label+' '+stage+' failed or warned')
                    report['results'].append(dict(variant=variant,caps=caps,trim=trim,observations=json.loads(result['stdout'])))
                    binary.unlink()  # Disposable scratch: retain identity/outcome, not all 16 binaries at once.
        first=report['results'][:8];second=report['results'][8:]
        if [v['observations'] for v in first]!=[v['observations'] for v in second]:raise RuntimeError('Constructor observations differ')
        report['passed']=True
    except Exception as error:report['error']=str(error)
a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({k:report.get(k) for k in ['passed','error','results']}));raise SystemExit(not report['passed'])
