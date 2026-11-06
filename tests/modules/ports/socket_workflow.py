#!/usr/bin/env python3
"""Actual create_socket-body ownership/error prefix differential with scripted native boundaries."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import sys
import overlay
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated,write_report
from tools.build_upstream import require_build_space


def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()


def configured_flags(core):
    """Read literal Make assignments only; never evaluate shell/functions/recipes."""
    makefile=core/'Makefile'
    if not makefile.is_file():
        return [],{'scope':'No configured Makefile; explicit core/header/OpenSSL paths only'}
    text=makefile.read_text().replace('\\\r\n',' ').replace('\\\n',' ')
    variables={}
    for line in text.splitlines():
        match=re.match(r'^([A-Za-z_][A-Za-z0-9_]*)\s*(\+=|\?=|:=|=)\s*(.*)$',line)
        if not match:continue
        name,operator,value=match.groups()
        if operator=='+=':variables[name]=variables.get(name,'')+' '+value
        elif operator!='?=' or name not in variables:variables[name]=value
    def expand(name,active=()):
        if name in active or len(active)>=32:raise ValueError('Recursive Makefile flag variable: '+name)
        if name not in variables:raise ValueError('Unknown Makefile flag variable: '+name)
        value=re.sub(r'\$\(([A-Za-z_][A-Za-z0-9_]*)\)|\$\{([A-Za-z_][A-Za-z0-9_]*)\}',
                     lambda m:expand(m.group(1) or m.group(2),(*active,name)),variables[name])
        if '$' in value or '`' in value:raise ValueError('Nonliteral Makefile flag expression: '+name)
        return value
    flags=[];selected={}
    for name in ['INCLUDES','CPPFLAGS']:
        if name in variables:
            selected[name]={'raw':variables[name],'expanded':expand(name)}
            flags+=shlex.split(selected[name]['expanded'],comments=True,posix=True)
    return flags,{'makefile':str(makefile),'sha256':sha(makefile),'variables':selected,'argv':flags,
                  'scope':'Literal configured INCLUDES/CPPFLAGS, compiler cwd is configured core; no shell evaluation'}


def extract(text):
    start=text.index('int ListenPort::create_socket()');end=text.index('\nvoid ListenPort::close_socket()',start)
    return text[start:end]


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--core',type=Path,required=True);p.add_argument('--openssl-root',type=Path);p.add_argument('--output',type=Path,required=True);p.add_argument('--sanitize',action='store_true');a=p.parse_args();overlay.SOURCE=a.source
    a.core=a.core.resolve();a.archive=a.archive.resolve()
    if a.openssl_root:a.openssl_root=a.openssl_root.resolve()
    folder=a.output.resolve().with_suffix('');folder.mkdir(parents=True,exist_ok=True)
    flags,configuration=configured_flags(a.core)
    report={'scope':__doc__,'pin':overlay.PIN,'platform':sys.platform,'configured_compiler_flags':configuration,'results':[],'passed':False,'native_instrumented':a.sanitize};outputs=[]
    for engine in ['upstream','rust']:
        directory=folder/engine;directory.mkdir(exist_ok=True)
        raw=extract(overlay.original('src/server/port.cc') if engine=='upstream' else overlay.selected(hybrid_tree(a.archive)[0],'src/server/port.cc'));(directory/'socket.inc').write_text(raw)
        command=['c++','-std=c++14','-O1','-UNDEBUG','-DSWOOLE_RS_PORTS','-DHAVE_CONFIG_H','-I'+str(ROOT/'native/ports'),'-I'+str(a.core),'-I'+str(a.core/'include'),'-I'+str(a.core/'thirdparty/llhttp'),*flags,'-I'+str(directory),str(Path(__file__).resolve().with_suffix('.cc')),*([str(a.archive)] if engine=='rust' else []),'-lpthread','-o',str(directory/'witness')]
        if a.openssl_root:command[1:1]=['-I'+str(a.openssl_root/'include')]
        if sys.platform=='linux':command+=['-ldl','-lm']
        if a.sanitize:command+=['-fsanitize=address,undefined','-fno-sanitize-recover=all','-g']
        require_build_space(ROOT);r=run_isolated(command,cwd=a.core,timeout=90);row={'engine':engine,'compile':command,'compile_cwd':str(a.core),'compile_exit':r.returncode,'compile_stderr':r.stderr};report['results'].append(row)
        if r.returncode:write_report(a.output,report);return 1
        r=run_isolated([str(directory/'witness')],timeout=45);(directory/'stdout.txt').write_text(r.stdout);row.update(exit=r.returncode,stderr=r.stderr)
        if r.returncode:write_report(a.output,report);return 1
        outputs.append(r.stdout)
    report['passed']=outputs[0]==outputs[1] and len(outputs[0].splitlines())==1280;report['cases']=len(outputs[0].splitlines())
    if not report['passed']:report['first_differences']=[{'index':n,'upstream':x,'rust':y} for n,(x,y) in enumerate(zip(outputs[0].splitlines(),outputs[1].splitlines())) if x!=y][:5]
    write_report(a.output,report);print(json.dumps({'passed':report['passed'],'cases':report['cases'],'differences':report.get('first_differences')}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
