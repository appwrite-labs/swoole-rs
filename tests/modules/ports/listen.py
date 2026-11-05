#!/usr/bin/env python3
"""Actual listen-body effect/warning prefix differential with scripted native boundaries."""
import argparse
import json
from pathlib import Path
import sys
import overlay
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated,write_report
from tools.build_upstream import require_build_space


def extract(text):
    start=text.index('int ListenPort::listen()');end=text.index('\nvoid ListenPort::init_protocol()',start)
    return text[start:end]


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--sanitize',action='store_true');a=p.parse_args();overlay.SOURCE=a.source
    folder=a.output.resolve().with_suffix('');folder.mkdir(parents=True,exist_ok=True)
    report={'scope':__doc__,'pin':overlay.PIN,'platform':sys.platform,'results':[],'passed':False,'native_instrumented':a.sanitize};outputs=[]
    for engine in ['upstream','rust']:
        directory=folder/engine;directory.mkdir(exist_ok=True)
        raw=extract(overlay.original('src/server/port.cc') if engine=='upstream' else overlay.selected(hybrid_tree(a.archive)[0],'src/server/port.cc'));(directory/'listen.inc').write_text(raw)
        command=['c++','-std=c++14','-O1','-UNDEBUG','-DSWOOLE_RS_PORTS','-I'+str(ROOT/'native/ports'),'-I'+str(directory),str(Path(__file__).with_suffix('.cc')),*([str(a.archive.resolve())] if engine=='rust' else []),'-lpthread','-o',str(directory/'witness')]
        if sys.platform=='linux':command+=['-ldl','-lm']
        if a.sanitize:command+=['-fsanitize=address,undefined','-fno-sanitize-recover=all','-g']
        require_build_space(ROOT);r=run_isolated(command,timeout=90);row={'engine':engine,'compile':command,'compile_exit':r.returncode,'compile_stderr':r.stderr};report['results'].append(row)
        if r.returncode:write_report(a.output,report);return 1
        r=run_isolated([str(directory/'witness')],timeout=45);(directory/'stdout.txt').write_text(r.stdout);row.update(exit=r.returncode,stderr=r.stderr)
        if r.returncode:write_report(a.output,report);return 1
        outputs.append(r.stdout)
    report['passed']=outputs[0]==outputs[1] and len(outputs[0].splitlines())==1536;report['cases']=len(outputs[0].splitlines())
    if not report['passed']:report['first_differences']=[{'index':n,'upstream':x,'rust':y} for n,(x,y) in enumerate(zip(outputs[0].splitlines(),outputs[1].splitlines())) if x!=y][:5]
    write_report(a.output,report);print(json.dumps({'passed':report['passed'],'cases':report['cases'],'differences':report.get('first_differences')}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
