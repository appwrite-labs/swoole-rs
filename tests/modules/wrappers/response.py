#!/usr/bin/env python3
"""Exact normal response transmission policy and optional whole wrapper SDK; no network or parser execution."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import overlay
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated,write_report
from tools.build_upstream import require_build_space

def prefix(text):
    start=text.index('bool Context::end(const char *data, size_t length) {')
    end=text.index('\n}\n',start)+2
    return text[start:end]+'\n'

def main(*, scope=None, witness=None, source_path="src/wrapper/http.cc", body_extractor=prefix, support=None):
    scope=scope or __doc__;witness=witness or Path(__file__).with_suffix(".cc")
    p=argparse.ArgumentParser(description=scope);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--core',type=Path);p.add_argument('--include',type=Path,action='append',default=[]);p.add_argument('--sanitize',action='store_true');p.add_argument('--rust-asan',action='store_true');a=p.parse_args();overlay.SOURCE=a.source
    if a.rust_asan and (not a.sanitize or '__asan_report_'not in subprocess.check_output(['nm','-u',str(a.archive)],text=True,stderr=subprocess.DEVNULL)):p.error('Actual instrumented archive and native sanitizer required')
    folder=a.output.resolve().with_suffix('');folder.mkdir(parents=True,exist_ok=True);report={'scope':scope,'pin':overlay.PIN,'sdk':[],'native_instrumented':a.sanitize,'rust_instrumented':a.rust_asan,'passed':False,'native_io_executed':False,'whole_response_lifecycle_qualified':False,'performance_qualified':False}
    # Original side from the pin, selected side from the hybrid build's fully patched tree.
    for name,text in [('original',overlay.original(source_path)),('selected',(hybrid_tree(a.archive)[0]/source_path).read_text())]:
        (folder/(name+'.inc')).write_text(body_extractor(text))
    raw=overlay.original('src/core/base.cc');start=raw.index('size_t sw_snprintf(');end=raw.index('\nsize_t sw_vsnprintf',start);(folder/'format.inc').write_text(raw[start:end])
    report['source_path']=source_path
    if support:report['support']=support(folder, a)
    def checked_space():
        try:require_build_space(ROOT)
        except RuntimeError as error:
            report.update(execution_status='blocked_build_space', error=str(error), passed=False)
            write_report(a.output, report)
            raise
    report['execution_status']='running';write_report(a.output, report)
    command=['c++','-std=c++17','-O1','-UNDEBUG','-DSWOOLE_RS_WRAPPERS','-I'+str(ROOT/'native/wrappers'),'-I'+str(folder),str(witness),str(a.archive.resolve()),'-lpthread','-o',str(folder/'response')]
    if sys.platform=='linux':command+=['-ldl','-lm']
    if a.sanitize:command+=['-fsanitize=address,undefined','-fno-sanitize-recover=all','-g']
    checked_space();r=run_isolated(command,timeout=90);report['compile']={'command':command,'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr}
    if not r.returncode:
        r=run_isolated([str(folder/'response')],timeout=30);report['run']={'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr}
    write_report(a.output, report)
    if a.core:
        core=a.core.resolve();base=['c++','-std=c++14','-O1','-UNDEBUG','-DHAVE_CONFIG_H',*['-I'+str(path)for path in [core,core/'include',core/'ext-src',core/'thirdparty',core/'thirdparty/llhttp',ROOT/'native/wrappers',core.parent/'rust-prefix/include',*[x.resolve()for x in a.include]]]]
        report['sdk_dependency_includes']=[str(x.resolve())for x in a.include]
        for engine in ['original','selected']:
            for source in ['src/wrapper/event.cc','src/wrapper/http.cc']:
                text=overlay.original(source)if engine=='original'else generated[source];path=folder/(engine+'-'+Path(source).name);path.write_text(text);command=[*base,*(['-DSWOOLE_RS_WRAPPERS']if engine=='selected'else[]),'-fsyntax-only',str(path)];checked_space();r=run_isolated(command,cwd=core,timeout=120);report['sdk'].append({'engine':engine,'source':source,'command':command,'cwd':str(core),'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr});write_report(a.output, report)
    report['execution_status']='complete'
    report['passed']=report.get('run',{}).get('exit')==0 and all(x['exit']==0 for x in report['sdk']);write_report(a.output,report);print(json.dumps({'passed':report['passed'],'run':report.get('run'),'sdk_exits':[x['exit']for x in report['sdk']]}));return int(not report['passed'])

if __name__=='__main__':sys.exit(main())
