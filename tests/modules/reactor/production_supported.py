#!/usr/bin/env python3
"""Separately named supported positive Reactor/Socket consumer; no fault/lifetime/performance claim."""
import argparse
import json
import re
from pathlib import Path
import shlex
import subprocess
import sys
import uuid
sys.path.insert(0,str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT,Engine,parse_engines,run_isolated,write_report
from tools import hybrid_identity as shared
from tools.hybrid_identity import attest,sha
from tools.build_upstream import require_build_space
import buffer_typed_identity as typed_buffer

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--source',type=Path,required=True)
p.add_argument('--source-ref',required=True)
p.add_argument('--engine',action='append',required=True)
p.add_argument('--build-manifest',type=Path,required=True)
p.add_argument('--archive',type=Path,required=True)
p.add_argument('--selected-export',type=Path,required=True)
p.add_argument('--native-export',type=Path,required=True)
p.add_argument('--php',default='php')
p.add_argument('--php-config',default='php-config')
p.add_argument('--php-extension',action='append',default=[])
p.add_argument('--output',type=Path,required=True)
p.add_argument('--require-owned-defer',action='store_true',help='Require the selected Rust deferred queue imports/origins and a Native control; builtins profile only')
p.add_argument('--require-typed-buffer',action='store_true',help='Require literal typed Buffer request/SDK/source for every profile; Buffer execution credit only in existing socket-output profile')
p.add_argument('--require-batch',action='store_true',help='Refuse older production sources before compiling/running the consumer')
profiles=p.add_mutually_exclusive_group()
profiles.add_argument('--callbacks',action='store_true',help='Separate genuine ordered callback-storage profile; require new matching production source')
profiles.add_argument('--builtins',action='store_true',help='Separate actual deferred/begin/idle/exit profile; require matching new source')
profiles.add_argument('--socket-output',action='store_true',help='Separate actual ordinary Socket DATA/small-positive-file profile; matching current output/parameters source required')
profiles.add_argument('--blocking',action='store_true',help='Genuine blocking Socket/writable wait/IOVector positive consumer')
a=p.parse_args();output=a.output.resolve()
if output.exists():raise ValueError('Receipt exists; choose a unique new attempt path')
work=output.parent/('reactor-supported-'+uuid.uuid4().hex);work.mkdir(parents=True)
report=dict(compiler_identity=subprocess.check_output(['c++','--version'],text=True),openssl_sdk_version=subprocess.check_output(['pkg-config','--modversion','openssl'],text=True).strip(),passed=False,scope=__doc__,assigned_to='@loks0n',work=str(work),profile='ordinary-blocking-socket-iov-positive' if a.blocking else 'ordinary-socket-output-positive' if a.socket_output else 'installed-builtins-positive' if a.builtins else 'ordered-callback-storage-positive' if a.callbacks else 'installed-filter-single-kqueue-record-positive',commands=[],bindings=[],results=[],limits=['Private AF_UNIX positive readiness/timeout/ONCE with live tokens only. No error, allocation failure, reentry, fd reuse, lifetime, external network or timing probes.','Compiled source identities are separate from the later fixture identity; old artifacts never receive new decoder credit.'])
def run(command):
    require_build_space(work)
    r=run_isolated(list(map(str,command)),cwd=work,timeout=30)
    report['commands'].append(dict(command=list(map(str,command)),exit=r.returncode,stdout=r.stdout,stderr=r.stderr));write_report(output,report)
    if r.returncode or r.stderr:raise ValueError('Command failed or warned; initial output retained')
    return r.stdout
try:
    report['require_typed_buffer']=a.require_typed_buffer
    report['typed_buffer_execution_requested']=bool(a.require_typed_buffer and a.socket_output)
    if a.require_typed_buffer:
        inputs=json.loads(a.build_manifest.read_text())['inputs']
        report['typed_buffer_requests']={'selected':typed_buffer.requested(inputs,True),'native':typed_buffer.requested(json.loads((a.native_export.parent/'build-manifest.json').read_text())['inputs'],False)}
        files={name:subprocess.check_output(['git','-C',str(ROOT),'show',a.source_ref+':'+name]) for name in (*typed_buffer.SOURCES,*typed_buffer.PATCHES)}
        typed_buffer.prepared(inputs,files)
    if a.require_owned_defer:
        if not a.builtins:raise ValueError('--require-owned-defer requires --builtins')
    report['require_owned_defer']=a.require_owned_defer
    bindings={}
    for value in a.engine:
        _,separator,path=value.partition('=')
        if not separator:raise ValueError('Use NAME=/canonical/export/swoole.so')
        binding=shared.immutable_export_input(path,[a.selected_export,a.native_export]);bindings[binding['extension']]=binding;report['bindings'].append(binding)
    engines=parse_engines(a.engine,a.php,a.php_extension)
    identities=[]
    for engine in engines:
        metadata=json.loads(run(engine.command('-r',"echo json_encode(['php'=>PHP_VERSION,'zts'=>PHP_ZTS,'rust_feature_bits'=>defined('SWOOLE_RS_FEATURE_BITS')?SWOOLE_RS_FEATURE_BITS:0]);")))
        metadata.update(name=engine.name,extension=str(engine.extension),sha256=sha(engine.extension));identities.append(metadata)
    if len(identities)!=2 or sum(bool(row['rust_feature_bits']&16384) for row in identities)!=1:raise ValueError('Require actual native/selected M05 pair')
    if any(row['rust_feature_bits'] and not row['rust_feature_bits']&16384 for row in identities):raise ValueError('Control must be the actual all-native profile')
    report['engines']=identities
    selected=next(row for row in identities if row['rust_feature_bits']&16384)
    report['source_attestation']=attest(a.build_manifest,a.archive,selected,a.source_ref,16384)
    selected_inputs=json.loads(a.build_manifest.read_text())['inputs']['rust_sources_sha256']
    # The wait loop (#391) is where the kernel arrays are decoded now.
    report['selected_batch_decoder_compiled']='crates/reactor/src/wait_loop.rs' in selected_inputs
    if a.require_batch and not report['selected_batch_decoder_compiled']:raise ValueError('Selected source predates the Rust batch decoder; no current caller qualification')
    report['selected_callback_storage_compiled']='crates/reactor/src/callbacks.rs' in selected_inputs
    if a.callbacks and not report['selected_callback_storage_compiled']:raise ValueError('Selected source predates Rust callback storage; no current caller qualification')
    report['selected_builtins_compiled']='crates/reactor/src/builtins.rs' in selected_inputs
    if a.builtins and not report['selected_builtins_compiled']:raise ValueError('Selected source predates Rust builtin orchestration; no current caller qualification')
    report['selected_socket_output_compiled']='crates/reactor/src/output.rs' in selected_inputs
    if a.socket_output and not report['selected_socket_output_compiled']:raise ValueError('Selected source predates Rust Socket output; no current caller qualification')
    source=Path(__file__).parent;fixture=source/('production_blocking.php' if a.blocking else 'production_output.php' if a.socket_output else 'production_builtins.php' if a.builtins else 'production_callbacks.php' if a.callbacks else 'production_supported.php');addon_source=source/'production_supported.cc'
    for engine,row in zip(engines,identities):
        selected=bool(row['rust_feature_bits']&16384);export=a.selected_export if selected else a.native_export
        manifest=a.build_manifest if selected else export.parent/'build-manifest.json'
        if json.loads(manifest.read_text())['extension_sha256']!=row['sha256']:raise ValueError('Actual extension/manifest mismatch')
        if row['php']!=subprocess.check_output([a.php_config,'--version'],text=True).strip():raise ValueError('PHP compile SDK mismatch')
        mirror=work/(engine.name+'-headers');row['headers']=shared.headers(a.source,export,manifest,mirror)
        if a.require_typed_buffer:
            config=mirror/'include/config.h'
            row['typed_buffer_sdk']={'path':str(config),'enabled':typed_buffer.configured(config.read_text(),selected)}
        addon=work/(engine.name+'-addon.so')
        command=['c++','-std=c++17','-O2','-fPIC','-shared','-DHAVE_CONFIG_H',*shlex.split(subprocess.check_output([a.php_config,'--includes'],text=True)),*shlex.split(subprocess.check_output(['pkg-config','--cflags','openssl'],text=True)),'-I'+str(mirror/'include'),'-MD','-MF',str(addon)+'.d',addon_source,'-o',addon]
        command+=['-undefined','dynamic_lookup'] if sys.platform=='darwin' else ['-ldl']
        run(command)
        deps=shlex.split(Path(str(addon)+'.d').read_text().replace('\\\n',' ').split(':',1)[1])
        if any(str(a.source.resolve()) in str(Path(name).resolve()) for name in deps):raise ValueError('Compiler escaped attested header mirror')
        undefined=subprocess.check_output(['nm','-u',str(addon)],text=True)
        if '_ZN6swoole7ReactorC1EiNS0_4TypeE' not in undefined:raise ValueError('Addon does not call genuine production constructor')
        result=json.loads(run(engine.command('-d','extension='+str(addon),fixture)))
        if result['checks']!=(54 if a.blocking else 78 if a.socket_output else 31 if a.builtins else 22 if a.callbacks else 210 if sys.platform=='darwin' else 240) or result['method_image']!=str(engine.extension):raise ValueError('Actual production consumer/origin differs')
        if a.blocking:
            symbols=['_ZN6swoole7network6Socket9send_syncEPKvmi','_ZN6swoole7network6Socket11writev_syncEPK5iovecm','_ZN6swoole7network6Socket13sendfile_syncEPKc'+('x' if sys.platform=='darwin' else 'l')+'m']
            if any(symbol not in undefined for symbol in symbols):raise ValueError('Addon must invoke all real blocking Socket methods')
            if result['cases']!=7 or any(result[key]!=str(engine.extension) for key in ['vector_image','file_image','event_image']):raise ValueError('Actual blocking method origins/cases differ')
        if a.socket_output:
            symbols=['_ZN6swoole7network6Socket11handle_sendEv','_ZN6swoole7network6Socket15handle_sendfileEv','_ZN6swoole7network6Socket14sendfile_asyncEPKc'+('x' if sys.platform=='darwin' else 'l')+'m']
            if any(symbol not in undefined for symbol in symbols):raise ValueError('Addon must call all genuine production Socket output methods')
            if result['cases']!=9 or any(result[key]!=str(engine.extension) for key in ['sendfile_image','request_image']):raise ValueError('Actual output method origins/cases differ')
            if selected and any(result[key]!=str(engine.extension) for key in ['send_policy_image','sendfile_cork_image','sendfile_sent_image','request_policy_image','request_owner_image','request_range_image','request_sent_image','request_free_image']):raise ValueError('Actual compiled Rust output/parameters origins differ')
        if a.require_typed_buffer and a.socket_output:typed_buffer.consumer(result,undefined,str(engine.extension),selected)
        if a.require_owned_defer:
            defer_symbol='_ZN6swoole7Reactor5deferERKNSt3__18functionIFvPvEEES3_' if sys.platform=='darwin' else '_ZN6swoole7Reactor5deferERKSt8functionIFvPvEES2_'
            imports={line.split()[-1].lstrip('_') for line in undefined.splitlines() if line.split()}
            if defer_symbol.lstrip('_') not in imports or result.get('defer_method_image')!=str(engine.extension):raise ValueError('Actual production defer method import/origin differs')
            if result.get('defer_owned_compiled')!=int(selected):raise ValueError('Compiled deferred queue differs from the engine')
            if selected and any(result.get(key)!=str(engine.extension) for key in ['defer_push_image','defer_step_image','defer_clear_image']):raise ValueError('Actual selected Rust deferred queue origins differ')
        if a.builtins and (result['begin_image']!=str(engine.extension) or result['end_image']!=str(engine.extension)):raise ValueError('Actual production builtin method origins differ')
        if selected and a.builtins and result['builtins_image']!=str(engine.extension):raise ValueError('Actual Rust builtin origin differs')
        if selected and a.callbacks and result['callbacks_image']!=str(engine.extension):raise ValueError('Rust callback storage origin differs')
        if selected and not (a.callbacks or a.builtins or a.socket_output or a.blocking) and result['storage_image']!=str(engine.extension):raise ValueError('Rust storage origin differs')
        if selected and not (a.callbacks or a.builtins or a.socket_output or a.blocking) and any(result[key]!=str(engine.extension) for key in ['state_image','backend_owner_image','kernel_wait_image','registry_owner_image']):raise ValueError('Rust reactor state/backend origins differ')
        if selected and not (a.callbacks or a.builtins or a.socket_output or a.blocking) and report['selected_batch_decoder_compiled'] and result['batch_image']!=str(engine.extension):raise ValueError('Compiled Rust decoder origin differs')
        if any(sha(mirror/name)!=digest for name,digest in row['headers']['files_sha256'].items()):raise ValueError('Attested headers changed')
        report['results'].append(dict(engine=engine.name,**result))
    report['typed_buffer_execution_qualified']=bool(a.require_typed_buffer and a.socket_output)
    report['typed_buffer_scope']='existing9/78output consumer plus exact constructor/front/source/origins' if a.require_typed_buffer and a.socket_output else 'capability/source/SDK preconditions only; no typed Buffer execution claim' if a.require_typed_buffer else 'not requested'
    report['passed']=True
except Exception as error:
    report['error']=f'{type(error).__name__}: {error}'
finally:
    for binding in report['bindings']:
        try:shared.verify_borrowed_input(binding)
        except Exception as error:report.update(passed=False,error=str(error))
    write_report(output,report)
print(json.dumps({key:report.get(key) for key in ['passed','error','work','selected_batch_decoder_compiled']}))
raise SystemExit(not report['passed'])
