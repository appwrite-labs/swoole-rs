#!/usr/bin/env python3
"""Prepare or qualify actual direct common-Zend facade consumers, without timing."""
import argparse
from datetime import datetime,timezone
import json
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
        if not sdk.get('passed'):raise ValueError('Require a completed native/hybrid SDK qualification')
        files={name:Path(__file__).with_name(name) for name in ['facade_entrypoint.cc','facade_entrypoint.php']}
        fixture=work/'facade_entrypoint.php';shutil.copy2(files['facade_entrypoint.php'],fixture)
        engines=[]
        if not a.prepare_only:
            engines=parse_engines(a.engine,a.php,a.php_extension);report['engines']=[engine.metadata() for engine in engines]
            if {bool(row['rust_feature_bits'] &16777216) for row in report['engines']}!={False,True}:raise ValueError('Require native and selected engines')
            selected=next(row for row in report['engines'] if row['rust_feature_bits'] &16777216)
            report['source_attestation']=attest(a.build_manifest,a.archive,selected,a.source_ref,16777216)
        for profile in ['native','hybrid']:
            directory=work/profile;directory.mkdir();shutil.copy2(files['facade_entrypoint.cc'],directory/'entrypoint.cc')
            row={};report['helper_profiles'][profile]=row
            try:
                helper=entrypoint.build_helper(a.sdk_report,directory,row,profile=profile)
                row['path']=str(helper)
                symbols=subprocess.check_output(['nm','-u',str(helper)],text=True,stderr=subprocess.PIPE)
                row['undefined_symbols']=symbols
                required=['swr_v1_phphelpers_'+name for name in ['string_plan','value_plan','duplicate_plan','factory_plan','call_success','empty']]
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
