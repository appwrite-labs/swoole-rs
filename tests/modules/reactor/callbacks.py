#!/usr/bin/env python3
"""Exact original/selected ordered callback methods with ordinary Native std::function effects.

No custom destruction observers, failure, retirement, recursive execution, fork, shutdown or
allocation-failure probes. Storage order/mutation/copy outcomes only; no full
production, lifetime, exception-prefix or performance claim.
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
p.add_argument('--source',type=Path,required=True)
p.add_argument('--archive',type=Path,required=True)
p.add_argument('--report',type=Path,required=True)
p.add_argument('--sanitize',action='store_true')
a=p.parse_args()
if a.report.exists():raise ValueError('Choose a unique new receipt path')
hybrid_source=hybrid(a.archive)[0]
report=dict(scope=__doc__,pin=PIN,hybrid_tree=str(hybrid_source),commands=[],results=[],passed=False)
with tempfile.TemporaryDirectory(prefix='swr-callbacks-') as temporary:
    tree=Path(temporary)
    original={name:subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+name],text=True) for name in ['include/swoole_reactor.h','src/reactor/base.cc']}
    try:
        for variant in ['original','selected']:
            header=original['include/swoole_reactor.h'] if variant=='original' else (hybrid_source/'include/swoole_reactor.h').read_text()
            base=original['src/reactor/base.cc'] if variant=='original' else (hybrid_source/'src/reactor/base.cc').read_text()
            fields='std::map<int,std::function<void(Reactor*)>> end_callbacks; std::map<int,std::function<bool(Reactor*,size_t&)>> exit_conditions;' if variant=='original' else 'ReactorCallbacks callbacks_;'
            cls='''namespace swoole { class Reactor { public:
                using EndCallback=int;using ExitCondition=int;
                bool timed_out=false;size_t get_event_num() const { return 5; }
                void set_end_callback(EndCallback,const std::function<void(Reactor*)>&);
                void erase_end_callback(EndCallback);
                void set_exit_condition(ExitCondition,const std::function<bool(Reactor*,size_t&)>&);
                void execute_end_callbacks(bool);bool if_exit();
                '''+'\n'.join(block(header,header.index(signature))[0] for signature in ['size_t remove_exit_condition(','bool isset_exit_condition('])+'\nprivate:\n'+fields+'\n};\n'
            if variant=='selected':
                start=base.index('template<class Function>');_,end=block(base,base.index('ReactorCallbacks &ReactorCallbacks::operator='));cls+=base[start:end]+'\n'
            for signature in ['void Reactor::set_end_callback(','void Reactor::erase_end_callback(','void Reactor::set_exit_condition(','void Reactor::execute_end_callbacks(','bool Reactor::if_exit(']:cls+=block(base,base.index(signature))[0]+'\n'
            (tree/'callbacks.inc').write_text(cls+'}\n')
            binary=tree/('callbacks-'+variant)
            command=['c++','-std=c++14','-O1','-g','-I'+str(tree),'-I'+str(ROOT/'native/reactor'),str(ROOT/'tests/modules/reactor/callbacks.cc'),str(a.archive.resolve()),'-lpthread','-o',str(binary)]
            if a.sanitize:command+=['-fsanitize=address,undefined','-fno-omit-frame-pointer']
            if sys.platform.startswith('linux'):command+=['-ldl','-lm']
            for stage,cmd in [('compile',command),('execute',[str(binary)])]:
                require_build_space(tree)
                result=run_compiler(cmd, timeout=60) if stage=='compile' else vars(run_isolated(cmd,cwd=tree,timeout=60))
                report['commands'].append(dict(variant=variant,stage=stage,command=cmd,exit=result['returncode'],stdout=result['stdout'],stderr=result['stderr'],cache={k:result[k] for k in ['cache_hit','cache_key','cache_reason'] if k in result}))
                if result['returncode'] or result['stderr']:raise RuntimeError(variant+' '+stage+' failed or warned')
            report['results'].append(dict(variant=variant,observations=json.loads(result['stdout'])))
        if report['results'][0]['observations']!=report['results'][1]['observations']:raise RuntimeError('Native/selected observations differ')
        report['passed']=True
    except Exception as error:report['error']=str(error)
a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({key:report.get(key) for key in ['passed','error','results']}))
raise SystemExit(not report['passed'])
