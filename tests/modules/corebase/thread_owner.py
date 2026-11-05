#!/usr/bin/env python3
"""Compare the actual native/hybrid Thread SDK: ownership, errors, forced unwind and termination."""
import argparse
import hashlib
import json
from pathlib import Path
import sys
import uuid

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated,write_report


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def queries(source):
    # Compile the actual SDK's query bodies with its own ThreadGlobal record.
    # These are caller fixtures, never an alternative production implementation.
    body=(source/'src/os/base.cc').read_text()
    begin=body.index('long swoole_thread_get_native_id()')
    end=body.index('\nnamespace swoole {',begin)
    return ('#include "swoole_thread.h"\n#include <sstream>\n'
            '#if defined(__linux__)\n#include <sys/syscall.h>\n#endif\n'
            'thread_local swoole::ThreadGlobal SwooleTG;\n'+body[begin:end])


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive',type=Path,required=True)
    parser.add_argument('--reference',type=Path,required=True,help='Configured unchanged native source SDK')
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args();args.output.parent.mkdir(parents=True,exist_ok=True)
    work=args.output.parent/('thread-owner-'+uuid.uuid4().hex);work.mkdir()
    report=dict(passed=False,scope=__doc__,work=str(work),commands=[],runs=[],sdk={})
    try:
        selected,flags=hybrid_tree(args.archive)
        for engine,source,extra in [('native',args.reference.resolve(),[]),('hybrid',selected,flags)]:
            if not (source/'config.h').is_file():raise ValueError('Require a configured matching SDK')
            report['sdk'][engine]={str(path.relative_to(source)):sha(path) for path in [source/'config.h',source/'include/swoole_thread.h',source/'src/os/base.cc']}
            unit=work/(engine+'-queries.cc');unit.write_text(queries(source))
            for standard in ['c++14','c++17']:
                binary=work/(engine+'-'+standard)
                command=['c++','-std='+standard,'-O1','-g','-Wall','-Wextra','-Werror','-pthread',
                         '-DHAVE_CONFIG_H','-I'+str(source),'-I'+str(source/'include'),'-I'+str(source/'thirdparty'),
                         *extra,str(Path(__file__).with_suffix('.cc')),str(unit),
                         *([str(args.archive.resolve())] if engine=='hybrid' else []),
                         '-fsanitize=address,undefined','-fno-sanitize-recover=all','-o',str(binary)]
                if sys.platform.startswith('linux'):command+=['-ldl','-lm']
                # Inspect the real SDK caller before linking: selected Thread operations
                # must consume the Rust owner ABI, while the native caller must not.
                obj=work/(engine+'-'+standard+'.o')
                caller=command[:command.index(str(Path(__file__).with_suffix('.cc')))]+[
                    '-c',str(Path(__file__).with_suffix('.cc')),'-o',str(obj)]
                compiled=run_isolated(caller,timeout=120)
                report['commands'].append(dict(engine=engine,standard=standard,scope='caller imports',argv=caller,
                    exit=compiled.returncode,stdout=compiled.stdout,stderr=compiled.stderr))
                if compiled.returncode:raise ValueError('Actual Thread SDK caller compilation failed')
                imports=run_isolated(['nm','-u',str(obj)],timeout=20)
                names=['start','join_check','join_complete','detach','move','alive','status','enter','exit','joinable','id']
                required={'swr_v1_core_thread_'+name:('swr_v1_core_thread_'+name) in imports.stdout for name in names}
                report['commands'][-1].update(undefined_symbols=imports.stdout,required_rust_calls=required)
                if imports.returncode or imports.stderr:raise ValueError('SDK caller symbol inventory failed')
                if engine=='hybrid' and not all(required.values()):raise ValueError('Selected SDK caller missed Rust Thread ownership')
                if engine=='native' and 'swr_v1_core_thread_' in imports.stdout:raise ValueError('Native SDK caller consumes Rust Thread ownership')
                result=run_isolated(command,timeout=120)
                report['commands'].append(dict(engine=engine,standard=standard,argv=command,exit=result.returncode,stdout=result.stdout,stderr=result.stderr))
                if result.returncode:raise ValueError('Actual Thread SDK compilation failed')
                for mode in ['normal','joinable-dtor','occupied-start','occupied-move','self-move','body-throw']:
                    command=[str(binary)]+([] if mode=='normal' else [mode])
                    result=run_isolated(command,timeout=25)
                    expected_exit=0 if mode=='normal' else 60
                    row=dict(engine=engine,standard=standard,mode=mode,argv=command,exit=result.returncode,
                             stdout=result.stdout,stderr=result.stderr)
                    row['passed']=result.returncode==expected_exit and not result.stderr and (
                        'thread ownership PASS; name error ' in result.stdout if mode=='normal' else result.stdout=='terminate\n')
                    report['runs'].append(row)
                    if not row['passed']:raise ValueError('Actual Thread ownership/lifetime/error witness failed')
        for standard in ['c++14','c++17']:
            for mode in ['normal','joinable-dtor','occupied-start','occupied-move','self-move','body-throw']:
                rows=[row for row in report['runs'] if row['standard']==standard and row['mode']==mode]
                if len(rows)!=2 or rows[0]['stdout']!=rows[1]['stdout']:raise ValueError('Native/hybrid observable SDK behavior differs')
        report['passed']=True
    except Exception as error:report['error']=str(error)
    write_report(args.output,report);print(json.dumps({key:report.get(key) for key in ['passed','error','work']}))
    return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
