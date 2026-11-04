#!/usr/bin/env python3
"""Pinned actual helper-body comparisons; no Zend runtime/parity claim."""
import argparse
import hashlib
import json
from pathlib import Path
import os
import subprocess
import sys
import uuid
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.build_upstream import require_build_space
from tools.harness import run_isolated,write_report
from tools.build_upstream import COMMIT as PIN


def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def body(text,signature):
    start=text.index(signature);end=text.index('{',start)+1;depth=1
    while depth:
        depth+=(text[end]=='{')-(text[end]=='}');end+=1
    return text[start:end]


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--archive',type=Path,required=True,help='<build>/rust-prefix/lib/libswoole_rs_ffi.a of the hybrid build');p.add_argument('--sanitize',action='store_true',help='Instrument native fixtures with address/undefined sanitizers');p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    output=a.output.resolve();work=output.parent/('phphelpers-'+uuid.uuid4().hex);work.mkdir(parents=True)
    report={'passed':False,'pin':PIN,'scope':__doc__,'work':str(work),'commands':[],'checks':[],
            'native_sanitizers':['address','undefined'] if a.sanitize else []}
    def checkpoint():write_report(output,report)
    def run(command,timeout=90,environment=None):
        require_build_space(ROOT)
        result=run_isolated(list(map(str,command)),timeout=timeout,**({'env':environment} if environment else {}))
        report['commands'].append(dict(command=list(map(str,command)),returncode=result.returncode,stdout=result.stdout,stderr=result.stderr));checkpoint();return result
    try:
        require_build_space(ROOT)
        report['toolchains']={}
        for tool in ['rustc','c++']:
            info=subprocess.run([tool,'--version'],text=True,capture_output=True,check=True)
            report['toolchains'][tool]=info.stdout.strip()
        hybrid=hybrid_tree(a.archive)[0];report['hybrid_tree']=str(hybrid)
        archive=a.archive.resolve();report['archive']=str(archive)
        sanitizer_flags=['-fsanitize=address,undefined','-fno-sanitize-recover=all','-fno-omit-frame-pointer'] if a.sanitize else []
        report['passed']=True
        for engine in ['reference','candidate']:
            common=(subprocess.check_output(['git','-C',str(a.source),'show',PIN+':ext-src/php_swoole_cxx.cc'],text=True) if engine=='reference' else (hybrid/'ext-src/php_swoole_cxx.cc').read_text())
            extracted=body(common,'Callable::Callable(')+'\nnamespace function {\n'+body(common,'bool call(')+'\n'+body(common,'Variable call(')+'\n}\n'
            (work/(engine+'-effects.inc')).write_text(extracted)
        effects=work/'effects'
        command=['c++','-std=c++17','-O2','-UNDEBUG','-DSWOOLE_RS_PHPHELPERS','-Wall','-Wextra','-Werror','-I'+str(work),'-I'+str(ROOT/'native/phphelpers'),Path(__file__).with_name('effects.cc'),archive,'-lpthread','-lm','-o',effects]
        command+=sanitizer_flags
        if sys.platform.startswith('linux'):command+=['-ldl','-lrt','-lutil']
        if run(command).returncode:raise ValueError('Complete native-effect fixture compilation failed')
        effect_result=run([effects]);report['passed']=report['passed'] and effect_result.returncode==0
        if not effect_result.returncode:report['effect_observations']=json.loads(effect_result.stdout)
        public_header=subprocess.check_output(['git','-C',str(a.source),'show',PIN+':ext-src/php_swoole_cxx.h'],text=True)
        macro=public_header[public_header.index('#define SW_ZEND_KNOWN_STRINGS'):public_header.index('#define SW_ZSTR_KNOWN')]
        (work/'known-macro.inc').write_text(macro)
        for engine in ['reference','candidate']:
            common=(subprocess.check_output(['git','-C',str(a.source),'show',PIN+':ext-src/php_swoole_cxx.cc'],text=True) if engine=='reference' else (hybrid/'ext-src/php_swoole_cxx.cc').read_text())
            table=(common[common.index('static const char *sw_known_strings[]'):common.index('SW_API zend_string **sw_zend_known_strings')] if engine=='reference' else '')
            pieces=[table,'zend_string **sw_zend_known_strings=nullptr;',body(common,'void known_strings_init() {'),body(common,'void known_strings_dtor() {'),body(common,'Callable::~Callable() {')]
            (work/(engine+'-cache.inc')).write_text('\n'.join(pieces))
        cache=work/'cache'
        command=['c++','-std=c++17','-O2','-UNDEBUG','-DSWOOLE_RS_PHPHELPERS','-Wall','-Wextra','-Werror','-I'+str(work),'-I'+str(ROOT/'native/phphelpers'),Path(__file__).with_name('cache.cc'),archive,'-lpthread','-lm','-o',cache]
        command+=sanitizer_flags
        if sys.platform.startswith('linux'):command+=['-ldl','-lrt','-lutil']
        if run(command).returncode:raise ValueError('Complete cache/callable fixture compilation failed')
        cache_result=run([cache]);report['passed']=report['passed'] and cache_result.returncode==0
        if not cache_result.returncode:report['cache_observations']=json.loads(cache_result.stdout)
        for family,name in [('facade','String'),('valuefacade','Variable')]:
            for engine in ['reference','candidate']:
                content=public_header if engine=='reference' else (hybrid/'ext-src/php_swoole_cxx.h').read_text()
                start=content.index('class '+name+' {');end=content.index('\n};',start)+3
                (work/(engine+'-'+family+'.inc')).write_text(content[start:end])
            facade=work/family
            command=['c++','-std=c++17','-O2','-UNDEBUG','-DSWOOLE_RS_PHPHELPERS','-Wall','-Wextra','-Werror','-I'+str(work),'-I'+str(ROOT/'native/phphelpers'),Path(__file__).with_name(family+'.cc'),archive,'-lpthread','-lm','-o',facade]+sanitizer_flags
            if sys.platform.startswith('linux'):command+=['-ldl','-lrt','-lutil']
            if run(command).returncode:raise ValueError('Complete '+name+' lifecycle fixture compilation failed')
            facade_result=run([facade]);report['passed']=report['passed'] and facade_result.returncode==0
            if not facade_result.returncode:report[family+'_observations']=json.loads(facade_result.stdout)
        for engine in ['reference','candidate']:
            content=public_header if engine=='reference' else (hybrid/'ext-src/php_swoole_cxx.h').read_text()
            start=content.index('class Callable {');end=content.index('\n};',start)+3
            piece='namespace zend {\n'+content[start:end]+'\nCallable::Callable(zval *value) { construct(&zfn,&fcc,&fn_name,value); }\nCallable::~Callable() { destroy(&zfn,fn_name); }\n}\n'
            piece+='\n'.join(body(content,signature) for signature in ['static inline zend::Callable *sw_callable_create(zval *zfn) {','static inline zend::Callable *sw_callable_create_ex('])
            (work/(engine+'-callablefacade.inc')).write_text(piece)
        callablefacade=work/'callablefacade'
        command=['c++','-std=c++17','-O2','-UNDEBUG','-DSWOOLE_RS_PHPHELPERS','-Wall','-Wextra','-Werror','-I'+str(work),'-I'+str(ROOT/'native/phphelpers'),Path(__file__).with_name('callablefacade.cc'),archive,'-lpthread','-lm','-o',callablefacade]+sanitizer_flags
        if sys.platform.startswith('linux'):command+=['-ldl','-lrt','-lutil']
        if run(command).returncode:raise ValueError('Inline Callable/factory fixture compilation failed')
        callable_result=run([callablefacade]);report['passed']=report['passed'] and callable_result.returncode==0
        if not callable_result.returncode:report['callablefacade_observations']=json.loads(callable_result.stdout)
    except Exception as error:
        report['passed']=False;report['error']=str(error)
        if isinstance(error,subprocess.CalledProcessError):report['error_stderr']=str(error.stderr)
    checkpoint();print(json.dumps({k:report.get(k) for k in ['passed','error','work']}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
