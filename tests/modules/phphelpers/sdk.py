#!/usr/bin/env python3
"""Compile both actual PHP helper TUs, pinned (native) and from the hybrid build tree (hybrid),
with stdext enabled/disabled; no runtime claim."""
import argparse
import io
import json
import os
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
import uuid
from gate import ROOT,PIN,sha,body
from tools.build_hybrid import hybrid_tree
from tools.build_upstream import require_build_space
from tools.harness import run_isolated,write_report

HEADERS={'.h','.hpp','.inc'}
# stdext deliberately includes this vendored C fragment.
STDEXT_FRAGMENT='thirdparty/php/zend/zend_execute.c'


def dependency_flags():
    """Preserve every optional probe; one failure must not erase other flags."""
    probes={}
    for package in ['openssl','libpcre2-8','libcurl','libpq']:
        try:
            result=subprocess.run(['pkg-config','--cflags',package],text=True,capture_output=True)
            probes[package]={'returncode':result.returncode,'stdout':result.stdout,'stderr':result.stderr}
        except OSError as error:
            probes[package]={'returncode':None,'stdout':'','stderr':str(error),'error_type':type(error).__name__}
    return {'returncode':int(any(row['returncode']!=0 for row in probes.values())),
            'stdout':' '.join(row['stdout'].strip() for row in probes.values() if row['returncode']==0),
            'stderr':'\n'.join(package+': '+row['stderr'].strip() for package,row in probes.items() if row['stderr']),
            'probes':probes}


def facade_surface(native,selected):
    """Pinned-format source guard, in addition to actual compiler/layout checks."""
    classes={}
    for name in ['String','KeyValue','ArrayIterator','Array','Variable','CharPtr','Callable']:
        def declarations(text):
            piece=body(text,'class '+name+' {')
            methods=list(re.finditer(r'^    (\S[^\n{};]*\([^\n{};]*\)[^\n{};]*\{)',piece,re.M))
            skeleton=piece
            for match in reversed(methods):
                end=match.start()+len(body(piece,match.group(0)))
                skeleton=skeleton[:match.end()]+skeleton[end-1:]
            return skeleton,len(methods)
        original,count=declarations(native);candidate,selected_count=declarations(selected)
        if original!=candidate or count!=selected_count:
            raise ValueError('Common facade declarations changed: '+name)
        classes[name]={'inline_methods':count}
    def macros(text):
        lines=iter(text.splitlines(keepends=True));definitions=[]
        for line in lines:
            if re.match(r'^\s*#\s*define\s',line):
                value=line
                while line.rstrip('\r\n').endswith('\\'):
                    line=next(lines);value+=line
                definitions.append(value)
        return definitions
    original=macros(native)
    if original!=macros(selected):raise ValueError('Public macro definitions changed')
    return {'matched':True,'classes':classes,'macro_definitions':len(original)}


def copy_headers(root,mirror):
    """Snapshot a tree's headers (and stdext's vendored C fragment) so dependency hashes stay put."""
    for directory,subdirectories,files in os.walk(root):
        subdirectories[:]=[name for name in subdirectories if name!='.git']
        for name in files:
            path=Path(directory)/name;relative=path.relative_to(root)
            if path.suffix in HEADERS or relative.as_posix()==STDEXT_FRAGMENT:
                target=mirror/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(path,target)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source',type=Path,required=True)
    p.add_argument('--sdk-export',type=Path,required=True,help='Immutable completed export.json with config.h identity')
    p.add_argument('--php-config',default='php-config')
    p.add_argument('--include',type=Path,action='append',default=[])
    p.add_argument('--output',type=Path,required=True)
    a=p.parse_args();output=a.output.resolve();work=output.parent/('phphelpers-sdk-'+uuid.uuid4().hex);work.mkdir(parents=True)
    report={'passed':False,'pin':PIN,'scope':__doc__,'work':str(work),'commands':[]}
    try:
        require_build_space(ROOT)
        config=a.sdk_export.parent/'config.h';export=json.loads(a.sdk_export.read_text())
        config_sha256=export['files']['config.h']['sha256']
        if sha(config)!=config_sha256:raise ValueError('Captured SDK config differs from immutable export')
        shutil.copy2(config,work/'config.h');shutil.copy2(a.sdk_export,work/'export.json')
        report['selected_source_ref']=export['source_ref']
        # The hybrid side is the export's own build: its patched tree, Rust ABI headers and switches.
        archive=Path(export['files']['libswoole_rs_ffi.a']['source'])
        if sha(archive)!=export['files']['libswoole_rs_ffi.a']['sha256']:raise ValueError('Hybrid build archive differs from immutable export')
        hybrid,_=hybrid_tree(archive)
        if sha(hybrid/'config.h')!=config_sha256:raise ValueError('Hybrid tree config differs from immutable export')
        report['hybrid_source']=str(hybrid)
        mirrors={engine:work/('headers-'+engine) for engine in ['native','hybrid']}
        names=subprocess.check_output(['git','-C',str(a.source),'ls-tree','-r','--name-only',PIN],text=True).splitlines()
        names=[name for name in names if Path(name).suffix in HEADERS or name==STDEXT_FRAGMENT]
        pinned=subprocess.check_output(['git','-C',str(a.source),'archive',PIN,'--',*names])
        with tarfile.open(fileobj=io.BytesIO(pinned)) as bundle:
            for item in bundle:
                if not item.isfile():continue
                path=Path(item.name)
                if path.is_absolute() or '..' in path.parts:raise ValueError('Unsafe pinned header path')
                target=mirrors['native']/path;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(bundle.extractfile(item).read())
        copy_headers(hybrid,mirrors['hybrid'])
        shutil.copytree(archive.parents[1]/'include',mirrors['hybrid']/'rust-prefix/include')
        for mirror in mirrors.values():shutil.copy2(config,mirror/'config.h')
        headers={engine:mirror/'ext-src/php_swoole_cxx.h' for engine,mirror in mirrors.items()}
        # entrypoint.py checks this against the pinned public header ABI.
        report['mirror_sha256']={'ext-src/php_swoole_cxx.h':sha(headers['native'])}
        manifest=json.loads((ROOT/'migration/patches/phphelpers/manifest.json').read_text())
        report['sources']=sorted(name for patch in manifest['patches'] for name in patch['upstream_paths'])
        report['declaration_surface']=facade_surface(headers['native'].read_text(),headers['hybrid'].read_text())
        includes=shlex.split(subprocess.check_output([a.php_config,'--includes'],text=True))
        report['php_version']=subprocess.check_output([a.php_config,'--version'],text=True).strip()
        report['php_includes']=includes
        report['pkg_config']=dependency_flags()
        report['cpp_standards']=['c++14','c++17']
        def base(engine):
            mirror=mirrors[engine]
            return ['c++','-std=c++14','-fsyntax-only','-DENABLE_PHP_SWOOLE','-DHAVE_CONFIG_H',
                    *['-I'+str(path) for path in [mirror,mirror/'include',mirror/'ext-src',mirror/'thirdparty',mirror/'thirdparty/llhttp',
                                                  *([mirror/'rust-prefix/include'] if engine=='hybrid' else []),*a.include]],
                    *includes,*shlex.split(report['pkg_config']['stdout'])]
        feature_names=sorted(set(re.findall(r'^#define (SWOOLE_RS_[A-Z0-9_]+)\b',config.read_text(),re.M)))
        if 'SWOOLE_RS_PHPHELPERS' not in feature_names:raise ValueError('Selected export does not enable phphelpers')
        # Native undefines every switch; hybrid keeps its build's own. A phphelpers-only
        # profile over the fully patched tree would trip other modules' #error guards.
        report['feature_profiles']={'native':[],'hybrid':feature_names}
        report['selected_header_sha256']=sha(headers['hybrid'])
        escapes=[str(a.source.resolve()),str(hybrid.resolve())]
        for engine,features in report['feature_profiles'].items():
            for enabled in [False,True]:
                profile=work/(engine+'-profile-'+str(enabled));profile.mkdir()
                shutil.copy2(mirrors[engine]/'php_swoole.h',profile/'php_swoole.h')
                (profile/'config.h').write_text(config.read_text()+'\n/* Private syntax-only optional-feature profile. */\n#undef SW_STDEXT\n'
                                               +('#define SW_STDEXT 1\n' if enabled else '')
                                               +('' if features else ''.join('#undef '+name+'\n' for name in feature_names)))
                for name in report['sources']:
                    if not name.endswith('.cc'):continue
                    unit=work/(engine+'-'+str(enabled)+'-'+Path(name).name)
                    unit.write_bytes(subprocess.check_output(['git','-C',str(a.source),'show',PIN+':'+name]) if engine=='native' else (hybrid/name).read_bytes())
                    for standard in report['cpp_standards']:
                        dep=unit.with_suffix('.'+standard+'.d');engine_base=base(engine)
                        command=[engine_base[0],'-std='+standard,*engine_base[2:5],'-I'+str(profile),*engine_base[5:],
                                 '-MD','-MF',str(dep),str(unit)]
                        require_build_space(ROOT)
                        result=run_isolated(command,timeout=120)
                        row={'cpp_standard':standard,'engine':engine,'stdext_enabled':enabled,'unit':name,'command':command,
                             'feature_macros':features,'config_sha256':sha(profile/'config.h'),
                             'returncode':result.returncode,'stdout':result.stdout,'stderr':result.stderr}
                        report['commands'].append(row)
                        if result.returncode==0:
                            paths=shlex.split(dep.read_text().replace('\\\n',' ').split(':',1)[1])
                            row['dependencies_sha256']={str(Path(path).resolve()):sha(path) for path in paths}
                            if (enabled or name.endswith('php_swoole_cxx.cc')) and str(headers[engine].resolve()) not in row['dependencies_sha256']:raise ValueError('Actual TU did not consume its assigned facade header')
                            if any(escape in path for path in row['dependencies_sha256'] for escape in escapes):raise ValueError('Compiler escaped mirror into a source tree')
                        write_report(output,report)
        # Observe only public sizes/alignment: no Zend objects or VM calls.
        layout=work/'layout.cc'
        classes=['String','KeyValue','ArrayIterator','Array','Variable','CharPtr','Callable']
        layout.write_text('#include "php_swoole_cxx.h"\n#include <cstdio>\nint main() {\n'+''.join(
            'std::printf("'+name+':%zu:%zu\\n",sizeof(zend::'+name+'),alignof(zend::'+name+'));\n' for name in classes)+'}\n')
        report['layout_commands']=[]
        for engine in report['feature_profiles']:
            profile=work/(engine+'-profile-True');binary=work/(engine+'-layout');engine_base=base(engine)
            command=[engine_base[0],'-std=c++14',*engine_base[3:5],'-I'+str(profile),*engine_base[5:],'-O2',str(layout),'-o',str(binary)]
            require_build_space(ROOT);compiled=run_isolated(command,timeout=120)
            row={'engine':engine,'command':command,'exit':compiled.returncode,'stdout':compiled.stdout,'stderr':compiled.stderr}
            report['layout_commands'].append(row)
            if not compiled.returncode:
                observed=run_isolated([str(binary)],timeout=10)
                row.update(run_exit=observed.returncode,layout=observed.stdout,run_stderr=observed.stderr)
        # Force optimized code generation for the whole actual inline helper.
        # This compiles a caller only; it never executes fn or coroutine::async.
        async_unit=work/'async-codegen.cc'
        async_unit.write_text('#include "php_swoole_cxx.h"\n'
                              'bool swr_sdk_force_async(bool blocking,const std::function<void(void)>& fn) {\n'
                              '    return php_swoole_async(blocking,fn);\n}\n')
        report['async_codegen_commands']=[]
        for engine in report['feature_profiles']:
            profile=work/(engine+'-profile-True');engine_base=base(engine)
            for standard in report['cpp_standards']:
                obj=work/(engine+'-async-'+standard+'.o');dep=obj.with_suffix('.d')
                command=[engine_base[0],'-std='+standard,*engine_base[3:5],'-I'+str(profile),*engine_base[5:],
                         '-O2','-Wall','-Werror','-MD','-MF',str(dep),'-c',str(async_unit),'-o',str(obj)]
                require_build_space(ROOT);compiled=run_isolated(command,timeout=120)
                row={'engine':engine,'cpp_standard':standard,'command':command,'exit':compiled.returncode,
                     'stdout':compiled.stdout,'stderr':compiled.stderr}
                report['async_codegen_commands'].append(row)
                if not compiled.returncode:
                    paths=shlex.split(dep.read_text().replace('\\\n',' ').split(':',1)[1])
                    if str(headers[engine].resolve()) not in {str(Path(path).resolve()) for path in paths}:
                        raise ValueError('Optimized async caller missed assigned whole header')
                    symbols=run_isolated(['nm','-u',str(obj)],timeout=20)
                    row.update(nm_exit=symbols.returncode,undefined_symbols=symbols.stdout,nm_stderr=symbols.stderr)
                    imported='swr_v1_phphelpers_async' in symbols.stdout
                    row['expected_async_import']=symbols.returncode==0 and not symbols.stderr and imported==(engine!='native')
                write_report(output,report)
        report['async_codegen_passed']=len(report['async_codegen_commands'])==4 and all(
            row['exit']==0 and row.get('expected_async_import') for row in report['async_codegen_commands'])
        report['layout_matched']=all(row['exit']==0 and row.get('run_exit')==0 and not row.get('run_stderr') for row in report['layout_commands']) and len({row.get('layout') for row in report['layout_commands']})==1
        report['passed']=len(report['commands'])==16 and all(row['returncode']==0 for row in report['commands']) and report['layout_matched'] and report['async_codegen_passed']
        if sha(config)!=config_sha256:raise ValueError('Input SDK config changed')
    except Exception as error:
        report['passed']=False;report['error']=str(error)
        if isinstance(error,subprocess.CalledProcessError):report['error_stderr']=str(error.stderr)
    write_report(output,report);print(json.dumps({k:report.get(k) for k in ['passed','error','work']}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
