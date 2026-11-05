#!/usr/bin/env python3
"""Ordinary stable counter storage: exact accessor bodies, actual Rust ABI and optional whole SDK compile."""
import argparse
import json
import re
from pathlib import Path
import shlex
import subprocess
import sys
import overlay
from socket_workflow import configured_flags
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated,write_report
from tools.build_upstream import require_build_space

def accessor(text,signature='size_t ListenPort::get_connection_num() const'):
    start=text.index(signature);end=text.index('\n}\n',start)+3
    return text[start:end]

def main(signatures=None,witness=None,scope=None,support=None,source_path="src/server/port.cc",extra_support=None,body_extractor=accessor):
    scope=scope or __doc__;signatures=signatures or ['size_t ListenPort::get_connection_num() const'];witness=witness or Path(__file__).with_suffix('.cc')
    p=argparse.ArgumentParser(description=scope);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True,help='Rust archive of the hybrid build under test');p.add_argument('--output',type=Path,required=True);p.add_argument('--core',type=Path,help='Configured pinned build tree for the original whole-SDK compile');p.add_argument('--sdk-origin',type=Path,help='Original configured src path when compiling a verified private SDK copy');p.add_argument('--php-config',default='php-config');p.add_argument('--openssl-root',type=Path);p.add_argument('--include',type=Path,action='append',default=[]);p.add_argument('--sanitize',action='store_true');p.add_argument('--rust-asan',action='store_true');a=p.parse_args();overlay.SOURCE=a.source
    if a.rust_asan and (not a.sanitize or '__asan_report_' not in subprocess.check_output(['nm','-u',str(a.archive)],text=True,stderr=subprocess.DEVNULL)):p.error('Actual Rust ASan archive and native sanitizer flag required')
    folder=a.output.resolve().with_suffix('');folder.mkdir(parents=True,exist_ok=True)
    # Original side from the pin, selected side from the hybrid build's fully patched tree,
    # where swoole_server_port.cc's remaining glue lives in swoole_server.cc.
    hybrid_source,hybrid_flags=hybrid_tree(a.archive)
    moved={'ext-src/swoole_server_port.cc':'ext-src/swoole_server.cc'}
    sides={'original':overlay.original,'selected':lambda path:(hybrid_source/moved.get(path,path)).read_text()}
    report={'scope':scope,'pin':overlay.PIN,'hybrid_tree':str(hybrid_source),'method_signatures':signatures,'native_instrumented':a.sanitize,'rust_instrumented':a.rust_asan,'support':{},'sdk':[],'passed':False}
    # Each side's files share names in their own folder; the witness includes them by name.
    for side,read in sides.items():
        (folder/side).mkdir(exist_ok=True)
        (folder/side/'body.inc').write_text('\n'.join(body_extractor(read(source_path),signature)for signature in signatures))
        for name,(source,signature)in (support or {}).items():
            raw=read(source);text=raw
            if signature:
                # A free function the hybrid defines in Rust under its upstream symbol comes from
                # the archive, as in the hybrid build. Witnesses include support files directly
                # inside their side's namespace, so the declaration steps out of it.
                function=signature.split('(')[0].split()[-1].lstrip('*')
                text=accessor(raw,signature) if side=='original' or signature in raw or '::' in function else f'}}\n{signature};\nnamespace {side} {{\nusing ::{function};\n'
            (folder/side/name).write_text(text);report['support'][name]={'source':source,'signature':signature}
        if extra_support:
            report.setdefault('extra_support',{})[side]=extra_support(folder/side,a,read)
    def checked_space():
        try:
            require_build_space(ROOT)
        except RuntimeError as error:
            report.update(execution_status='blocked_build_space', error=str(error), passed=False)
            write_report(a.output, report)
            raise
    report['execution_status'] = 'running'
    write_report(a.output, report)
    # The selected TU includes the Rust ABI headers declaring the functions its fragments call.
    called=set(re.findall(r'\bswr_v\d+_\w+',''.join(path.read_text() for path in (folder/'selected').iterdir())))
    headers=[path.name for path in sorted((hybrid_source.parent/'rust-prefix/include').glob('swoole_rs*.h')) if any(re.search(r'\b'+name+r'\s*\(',path.read_text()) for name in called)]
    # Two translation units: the original side with only its pinned fragments, the selected
    # side and the driver with the hybrid build's flags.
    common=['c++','-std=c++17','-O1','-UNDEBUG']
    if a.sanitize:common+=['-fsanitize=address,undefined','-fno-sanitize-recover=all','-g']
    original=[*common,'-c','-DPORTS_SIDE=original','-I'+str(folder/'original'),str(witness),'-o',str(folder/'original.o')]
    command=[*common,'-DPORTS_SIDE=selected','-DPORTS_DRIVER','-I'+str(folder/'selected'),*hybrid_flags,*[part for header in sorted(headers) for part in ('-include',header)],str(witness),str(folder/'original.o'),str(a.archive.resolve()),'-lpthread','-o',str(folder/'count')]
    if sys.platform=='linux':command+=['-ldl','-lm']
    checked_space();r=run_isolated(original,timeout=90)
    if not r.returncode:r=run_isolated(command,timeout=90)
    report['compile']={'commands':[original,command],'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr}
    if not r.returncode:
        r=run_isolated([str(folder/'count')],timeout=30);report['run']={'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr}
    write_report(a.output, report)
    if a.core:
        # The original sources compile against the configured pinned tree, the selected
        # ones in place against the hybrid build's own configured tree.
        includes=shlex.split(subprocess.check_output([a.php_config,'--includes'],text=True))
        for engine,core,read in [('original',a.core.resolve(),overlay.original),('selected',hybrid_source,sides['selected'])]:
            flags,configuration=configured_flags(core)
            if a.sdk_origin and engine=='original':
                origin=a.sdk_origin.parent;private=core.parent
                flags=['-I'+str(private/Path(flag[2:]).relative_to(origin)) if flag.startswith('-I') and Path(flag[2:]).is_relative_to(origin) else flag for flag in flags]
                configuration['relocation']={'original_build_root':str(origin),'private_build_root':str(private)}
            configuration['effective_argv']=flags;report['extra_includes']={str(x.resolve()):'explicit private declaration include'for x in a.include};report.setdefault('configured_compiler_flags',{})[engine]=configuration
            abi=[ROOT/'native/ports',ROOT/'native/http'] if engine=='original' else [hybrid_source.parent/'rust-prefix/include']
            base=['c++','-std=c++14','-O1','-UNDEBUG','-DHAVE_CONFIG_H','-I'+str(core),'-I'+str(core/'include'),'-I'+str(core/'thirdparty/llhttp'),'-I'+str(core/'thirdparty'),'-I'+str(core/'ext-src'),*['-I'+str(x)for x in abi],*['-I'+str(x.resolve())for x in a.include],*flags]
            if a.openssl_root:base+=['-I'+str(a.openssl_root.resolve()/'include')]
            for source in ['src/server/port.cc','ext-src/swoole_server_port.cc']:
                path=folder/(engine+'-'+Path(source).name);path.write_text(read(source))
                command=[*base,*(includes if source.startswith('ext-src') else []),'-fsyntax-only',str(path)]
                checked_space();r=run_isolated(command,cwd=core,timeout=120);report['sdk'].append({'engine':engine,'source':source,'command':command,'compile_cwd':str(core),'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr});write_report(a.output,report)
    report['execution_status']='complete'
    report['passed']=report.get('run',{}).get('exit')==0 and all(r['exit']==0 for r in report['sdk']);write_report(a.output,report);print(json.dumps({'passed':report['passed'],'run':report.get('run'),'sdk_exits':[r['exit']for r in report['sdk']]}));return int(not report['passed'])

if __name__=='__main__':sys.exit(main())
