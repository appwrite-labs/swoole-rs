#!/usr/bin/env python3
"""Complete optional Context constructor/query against exact loaded Native effects and Rust marker symbols; no switches or coroutine execution."""
import argparse
import json
from pathlib import Path
import platform
import shlex
import subprocess
import sys
import uuid
sys.path.insert(0,str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT,Engine,run_isolated,write_report
from tools import hybrid_identity as shared
from tools.hybrid_identity import attest,sha
from tools.build_upstream import require_build_space

p=argparse.ArgumentParser(description=__doc__)
for name in ['source','selected-export','native-export','output']:
    p.add_argument('--'+name,type=Path,required=True)
p.add_argument('--source-ref',required=True)
p.add_argument('--php',default='php');p.add_argument('--php-config',default='php-config')
p.add_argument('--php-extension',action='append',default=[])
a=p.parse_args()
for name in ['source','selected_export','native_export']:setattr(a,name,getattr(a,name).resolve(strict=True))
output=a.output.resolve()
if output.exists():raise ValueError('Choose a new attempt receipt')
work=output.parent/('context-marker-constructor-'+uuid.uuid4().hex);work.mkdir(parents=True)
report=dict(passed=False,full_gate_passed=False,scope=__doc__,work=str(work),bindings=[],commands=[],results=[],unrun=['Coroutine creation/run/swap','thread contexts','callbacks/observers','failure/retirement/lifetime profiles','sanitizers','whole production optional-feature configuration','performance'])
def run(command):
    require_build_space(work)
    r=run_isolated(list(map(str,command)),cwd=work,timeout=30)
    report['commands'].append(dict(command=list(map(str,command)),exit=r.returncode,stdout=r.stdout,stderr=r.stderr));write_report(output,report)
    if r.returncode or r.stderr:raise ValueError('Command failed or warned; original output retained')
    return r.stdout
try:
    fixture=Path(__file__).with_suffix('.php');cpp=Path(__file__).with_suffix('.cc')
    for label,export in [('native',a.native_export),('selected',a.selected_export)]:
        directory=export.parent;manifest=directory/'build-manifest.json';extension=directory/'swoole.so'
        binding=shared.immutable_export_input(extension,[export]);report['bindings'].append(binding)
        engine=Engine(label,extension,a.php,a.php_extension)
        identity=json.loads(run(engine.command('-r',"echo json_encode(['php'=>PHP_VERSION,'zts'=>PHP_ZTS,'rust_feature_bits'=>defined('SWOOLE_RS_FEATURE_BITS')?SWOOLE_RS_FEATURE_BITS:0]);")))
        identity.update(sha256=sha(extension),name=label)
        if identity['php']!=subprocess.check_output([a.php_config,'--version'],text=True).strip():raise ValueError('PHP compile SDK differs')
        if label=='selected':
            report['source_attestation']=attest(manifest,directory/'libswoole_rs_ffi.a',identity,a.source_ref,32768)
            inputs=json.loads(manifest.read_text())['inputs']
            if 'crates/coroutine/src/stack.rs' not in inputs['rust_sources_sha256']:raise ValueError('Production source predates Rust marker implementation')
        elif identity['rust_feature_bits']:raise ValueError('Native control selects Rust')
        mirror=work/(label+'-sdk');identity['headers']=shared.headers(a.source,export,manifest,mirror)
        raw=subprocess.check_output(['git','-C',str(a.source),'show',shared.COMMIT+':src/coroutine/context.cc'])
        original=mirror/'src/coroutine/context.cc';original.parent.mkdir(parents=True);original.write_bytes(raw)
        if label=='selected':
            # Every shipped patch's context.cc hunks, in build order: the file the build compiles.
            identity['patch_proof']=[]
            for entry in [row for row in inputs['patches'] if 'src/coroutine/context.cc' in row['upstream_paths']]:
                patch=mirror/'context.patch';patch.write_bytes(subprocess.check_output(['git','show',a.source_ref+':'+entry['path']],cwd=ROOT))
                if sha(patch)!=entry['sha256']:raise ValueError('Patch differs from the build: '+entry['path'])
                subprocess.run(['git','apply','--include=src/coroutine/context.cc',str(patch)],cwd=mirror,capture_output=True,check=True)
                identity['patch_proof'].append(entry['path'])
            if original.read_bytes()==raw:raise ValueError('No shipped patch edits context.cc')
        target=mirror/'actual-context.inc';target.write_bytes(original.read_bytes())
        addon=work/(label+'-markers.so')
        # SWOOLE_RS_COROUTINE: the addon's own switch to report the Rust marker's image.
        command=['c++','-std=c++17','-O2','-fPIC','-shared','-fvisibility=hidden','-DHAVE_CONFIG_H',*(['-DSWOOLE_RS_COROUTINE'] if label=='selected' else []),'-DSW_CONTEXT_DETECT_STACK_USAGE',*shlex.split(subprocess.check_output([a.php_config,'--includes'],text=True)),*shlex.split(subprocess.check_output(['pkg-config','--cflags','openssl'],text=True)),'-I'+str(mirror/'include'),'-I'+str(mirror),cpp,'-o',addon]
        if sys.platform=='darwin':
            command+=['-undefined','dynamic_lookup']
        else:
            # The extension keeps its boost context-switch symbols local, so the addon assembles
            # its own copy, chosen as config.m4 does.
            flavour={'x86_64':'x86_64_sysv_elf_gas','aarch64':'arm64_aapcs_elf_gas'}[platform.machine()]
            for kind in ['make','jump']:
                name='thirdparty/boost/asm/'+kind+'_'+flavour+'.S'
                asm=mirror/name;asm.parent.mkdir(parents=True,exist_ok=True)
                asm.write_bytes(subprocess.check_output(['git','-C',str(a.source),'show',shared.COMMIT+':'+name]))
                command.insert(-2,asm)
            command+=['-ldl']
        run(command)
        symbols=subprocess.check_output(['nm',str(addon)],text=True)
        identity['whole_context_definitions']=[line for line in symbols.splitlines() if any(name in line for name in ['ContextC1','ContextD1','Context15get_stack_usageEv']) and len(line.split())>=3 and line.split()[-2] in ['t','T']]
        if len(identity['whole_context_definitions'])!=3:raise ValueError('Addon lacks whole constructor/destructor/query definitions')
        undefined=subprocess.check_output(['nm','-u',str(addon)],text=True)
        # The patched Context allocates and marks its stack in Rust.
        for symbol in ['swr_v1_context_stack_alloc']:
            if (symbol in undefined)!=(label=='selected'):raise ValueError('Whole Context caller does not bind expected Rust symbols')
        result=json.loads(run(engine.command('-d','extension='+str(addon),fixture)))
        if label=='selected' and result['marker_image']!=str(extension):raise ValueError('Marker Rust symbol belongs to another image')
        identity.update(observations=result)
        report['results'].append(identity)
        if any(sha(mirror/name)!=value for name,value in identity['headers']['files_sha256'].items()):raise ValueError('Header mirror changed')
    for field in ['contexts','checks','callbacks','class_size','class_alignment']:
        if report['results'][0]['observations'][field]!=report['results'][1]['observations'][field]:raise ValueError('Native/selected observations differ: '+field)
    report['passed']=True
except Exception as error:report['error']=f'{type(error).__name__}: {error}'
finally:
    for binding in report['bindings']:
        try:shared.verify_borrowed_input(binding)
        except Exception as error:report.update(passed=False,error=str(error))
    write_report(output,report)
print(json.dumps({key:report.get(key) for key in ['passed','error','work']}))
raise SystemExit(not report['passed'])
