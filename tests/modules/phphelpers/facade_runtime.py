#!/usr/bin/env python3
"""Prepare or qualify actual direct common-Zend facade consumers, without timing."""
import argparse
from datetime import datetime,timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import uuid
import entrypoint
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report
from tools.hybrid_identity import attest
sha=entrypoint.sha


def selected_header_identity(sdk,manifest,source_ref,work):
    """Match the SDK's actual selected header/ABI to this production input set."""
    inputs=json.loads(Path(manifest).read_text())['inputs']
    destination='ext-src/php_swoole_cxx.h'
    patches=[row for row in inputs['patches'] if destination in row['upstream_paths']]
    if len(patches)!=1 or patches[0]['upstream_paths']!=[destination]:
        raise ValueError('Require one isolated attested common facade header overlay')
    directory=work/'production-header';(directory/'ext-src').mkdir(parents=True)
    native_header=Path(sdk['work'])/'headers-native'/destination
    if sdk['pin']!=entrypoint.PIN or sha(native_header)!=entrypoint.PUBLIC_HEADER_SHA256:
        raise ValueError('SDK pinned native header identity differs')
    original=native_header.read_bytes()
    (directory/destination).write_bytes(original)
    patch=patches[0];content=subprocess.check_output(['git','show',source_ref+':'+patch['path']],cwd=ROOT)
    if hashlib.sha256(content).hexdigest()!=patch['sha256']:raise ValueError('Production header overlay changed')
    path=directory/'overlay.patch';path.write_bytes(content)
    env={**os.environ,'GIT_CEILING_DIRECTORIES':str(directory.parent)}
    for options in [['--check'],[]]:
        subprocess.run(['git','apply',*options,str(path)],cwd=directory,env=env,check=True,capture_output=True)
    projected=sha(directory/destination)
    if projected!=sdk['selected_header_sha256']:raise ValueError('SDK selected header differs from production source')
    expected=inputs['headers_sha256']['native/phphelpers/swoole_rs_phphelpers.h']
    rows=[row for row in sdk['commands'] if row['engine']=='hybrid' and not row['stdext_enabled']
          and row['cpp_standard']=='c++14' and row['unit']=='ext-src/php_swoole_cxx.cc']
    dependencies=[digest for path,digest in rows[0]['dependencies_sha256'].items() if Path(path).name=='swoole_rs_phphelpers.h']
    if dependencies!=[expected]:raise ValueError('SDK local ABI differs from selected production hybrid')
    return dict(selected_header_sha256=projected,local_abi_sha256=expected,patch=patch)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--sdk-report',type=Path,required=True)
    p.add_argument('--prepare-only',action='store_true',help='Compile tiny helpers only; do not load or claim runtime')
    p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append');p.add_argument('--build-manifest',type=Path)
    p.add_argument('--archive',type=Path);p.add_argument('--source-ref');p.add_argument('--output',type=Path,required=True)
    a=p.parse_args()
    if not a.prepare_only and not all([a.engine,a.build_manifest,a.archive,a.source_ref]):p.error('Runtime requires engine,manifest,archive and source-ref')
    output=a.output.resolve();work=output.parent/('facade-entry-'+uuid.uuid4().hex);work.mkdir(parents=True)
    report=dict(created_utc=datetime.now(timezone.utc).isoformat(),passed=False,scope=__doc__,work=str(work),
        preparation_only=a.prepare_only,runtime_executed=False,helper_profiles={},results=[],measurement_performed=False)
    try:
        sdk=json.loads(a.sdk_report.read_text())
        if not sdk.get('passed') or not sdk.get('layout_matched'):raise ValueError('Require completed native/hybrid SDK/layout qualification')
        files={name:Path(__file__).with_name(name) for name in ['facade_entrypoint.cc','facade_entrypoint.php']}
        fixture=work/'facade_entrypoint.php';shutil.copy2(files['facade_entrypoint.php'],fixture)
        engines=[]
        if not a.prepare_only:
            engines=parse_engines(a.engine,a.php,a.php_extension);report['engines']=[engine.metadata() for engine in engines]
            if {bool(row['rust_feature_bits'] &16777216) for row in report['engines']}!={False,True}:raise ValueError('Require native and selected engines')
            selected=next(row for row in report['engines'] if row['rust_feature_bits'] &16777216)
            report['source_attestation']=attest(a.build_manifest,a.archive,selected,a.source_ref,16777216)
            report['header_source_identity']=selected_header_identity(sdk,a.build_manifest,report['source_attestation']['source_ref'],work)
        for profile in ['native','hybrid']:
            directory=work/profile;directory.mkdir();shutil.copy2(files['facade_entrypoint.cc'],directory/'entrypoint.cc')
            row={};report['helper_profiles'][profile]=row
            try:
                helper=entrypoint.build_helper(a.sdk_report,directory,row,profile=profile)
                row['path']=str(helper)
                symbols=subprocess.check_output(['nm','-u',str(helper)],text=True,stderr=subprocess.PIPE)
                row['undefined_symbols']=symbols
                required=['swr_v1_phphelpers_'+name for name in ['string_plan','value_plan','duplicate_plan','factory_plan','iterator_plan','call_success','empty']]
                row['required_rust_calls']={name:name in symbols for name in required}
                if profile=='native':
                    if 'swr_v1_phphelpers_' in symbols:raise ValueError('Native helper unexpectedly consumes Rust policy')
                elif not all(row['required_rust_calls'].values()):raise ValueError('Selected helper omits a required Rust caller')
                row['passed']=True
            except Exception as error:row.update(passed=False,error=str(error))
        if not all(row['passed'] for row in report['helper_profiles'].values()):raise ValueError('Helper compile failed; all profile outputs retained')
        plans=[(engine,identity,profile) for engine,identity in zip(engines,report.get('engines',[]))
               for profile in (['hybrid'] if identity['rust_feature_bits'] &16777216 else ['native'])]
        for engine,identity,profile in plans:
            helper=report['helper_profiles'][profile]
            if identity['php']!=helper['helper_compilation']['sdk_php']:raise ValueError('Helper SDK/PHP versions differ')
            command=engine.command('-d','extension='+helper['path'],str(fixture))
            result=run_isolated(command,timeout=30);report['runtime_executed']=True
            row=dict(engine=engine.name,profile=profile,command=command,exit=result.returncode,stdout=result.stdout,stderr=result.stderr,
                extension_sha256_after=sha(identity['extension']),passed=False,status='failed')
            report['results'].append(row)
            if result.returncode==2 and result.stderr.startswith('UNSUPPORTED:'):row['status']='unsupported'
            if result.returncode==0 and not result.stderr and row['extension_sha256_after']==identity['sha256']:
                try:row['summary']=json.loads(result.stdout);row['passed']=row['summary']['matches']>0;row['status']='passed' if row['passed'] else 'failed'
                except (ValueError,KeyError):pass
            if result.returncode<0:
                report['stopped_on_signal']=True;report['not_run_profiles']=[dict(engine=e.name,profile=p,status='NOT_RUN') for e,_,p in plans[len(report['results']):]]
                break
        if not a.prepare_only:
            if len(report['results'])!=len(plans) or not all(row['passed'] for row in report['results']):raise ValueError('Facade runtime failed; original statuses retained')
            if len({json.dumps(row['summary'],sort_keys=True) for row in report['results']})!=1:raise ValueError('Facade outputs differ across profiles/engines')
        if not a.prepare_only:
            receipt=report['source_attestation']
            if sha(a.build_manifest)!=receipt['manifest_sha256'] or sha(a.archive)!=receipt['archive_sha256']:raise ValueError('Production input changed')
        report['passed']=True
    except Exception as error:report['error']=str(error)
    write_report(output,report);print(json.dumps({key:report.get(key) for key in ['passed','error','preparation_only','runtime_executed','work']}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
