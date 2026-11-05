#!/usr/bin/env python3
"""Prepare checked actual-PHP helper workloads; never start timed measurements."""
import argparse
from datetime import datetime,timezone
import json
from pathlib import Path
import shlex
import shutil
import sys
import uuid
import entrypoint
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report
from tools.hybrid_identity import attest
sha=entrypoint.sha


def optimization_observation(identity):
    directory=Path(identity['extension']).parent
    export_path=directory/'export.json'
    export=json.loads(export_path.read_text())
    for name,info in export['files'].items():
        if sha(directory/name)!=info['sha256']:
            raise ValueError('Immutable export changed: '+name)
    if export['files']['swoole.so']['sha256']!=identity['sha256']:
        raise ValueError('Export does not identify this engine')
    flags=export['native_compilation_observation']['make_variables']['CXXFLAGS']
    levels=[flag for flag in shlex.split(flags) if flag.startswith('-O')]
    if not levels or levels[-1] not in ['-O1','-O2','-O3','-Os','-Oz']:
        raise ValueError('Require observed optimized native CXXFLAGS')
    manifest=json.loads((directory/'build-manifest.json').read_text())
    profile=(manifest.get('rust_compilation') or {}).get('artifact',{}).get('profile')
    if identity['rust_feature_bits'] &16777216:
        if not profile or profile.get('opt_level') not in ['1','2','3','s','z'] or profile.get('debug_assertions'):
            raise ValueError('Require actual optimized selected Rust compilation profile')
    return dict(configured_cxxflags=flags,rust_profile=profile,fix_halfclose=manifest['inputs']['fix_halfclose'])


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--sdk-report',type=Path,required=True)
    p.add_argument('--php',default='php')
    p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append',required=True)
    p.add_argument('--build-manifest',type=Path,required=True)
    p.add_argument('--archive',type=Path,required=True)
    p.add_argument('--source-ref',required=True)
    p.add_argument('--output',type=Path,required=True)
    a=p.parse_args();output=a.output.resolve();work=output.parent/('prepared-php-'+uuid.uuid4().hex);work.mkdir(parents=True)
    report=dict(created_utc=datetime.now(timezone.utc).isoformat(),passed=False,scope=__doc__,work=str(work),
                measurement_performed=False,results=[])
    try:
        files={name:Path(__file__).with_name(name) for name in ['entrypoint.cc','php_benchmark.php']}
        for name,path in files.items():shutil.copy2(path,work/name)
        engines=parse_engines(a.engine,a.php,a.php_extension);report['engines']=[engine.metadata() for engine in engines]
        if {bool(row['rust_feature_bits'] &16777216) for row in report['engines']}!={False,True}:
            raise ValueError('Require native and selected phphelpers engines')
        selected=next(row for row in report['engines'] if row['rust_feature_bits'] &16777216)
        report['source_attestation']=attest(a.build_manifest,a.archive,selected,a.source_ref,16777216)
        report['optimization_observations']={row['name']:optimization_observation(row) for row in report['engines']}
        if len({row['fix_halfclose'] for row in report['optimization_observations'].values()})!=1:
            raise ValueError('Require matching explicit native correction identities')
        helpers={};report['helper_profiles']={}
        for profile in ['native','hybrid']:
            directory=work/profile;directory.mkdir()
            shutil.copy2(work/'entrypoint.cc',directory/'entrypoint.cc')
            row={};helpers[profile]=entrypoint.build_helper(a.sdk_report,directory,row,profile=profile)
            report['helper_profiles'][profile]=row['helper_compilation']
        report['helper_compilation']=report['helper_profiles']['native']
        if any(row['php']!=report['helper_profiles']['hybrid' if row['rust_feature_bits'] &16777216 else 'native']['sdk_php'] for row in report['engines']):
            raise ValueError('Helper SDK/PHP versions differ')
        report['future_commands']={}
        for engine,identity in zip(engines,report['engines']):
            helper=helpers['hybrid' if identity['rust_feature_bits'] &16777216 else 'native']
            command=engine.command('-d','extension='+str(helper),str(work/'php_benchmark.php'),'--check')
            result=run_isolated(command,timeout=60)
            row=dict(engine=engine.name,command=command,exit=result.returncode,stdout=result.stdout,stderr=result.stderr,
                     passed=False,status='failed',extension_sha256_after=sha(identity['extension']))
            report['results'].append(row)
            if result.returncode==2 and result.stderr.startswith('UNSUPPORTED:'):row['status']='unsupported'
            if result.returncode==0 and not result.stderr and row['extension_sha256_after']==identity['sha256']:
                try:
                    row['check']=json.loads(result.stdout)
                    row['passed']=row['check']['elapsed_ns']==0 and row['check']['checks']>0
                    row['status']='passed' if row['passed'] else 'failed'
                except (ValueError,KeyError):pass
            report['future_commands'][engine.name]=engine.command('-d','extension='+str(helper),str(work/'php_benchmark.php'),
                '--measure','WORKLOAD','ITERATIONS','ACKNOWLEDGED_HOST_LEASE')
        if not all(row['passed'] for row in report['results']):
            raise ValueError('Actual PHP preparation failed; every first engine result retained')
        if len({json.dumps(row['check'],sort_keys=True) for row in report['results']})!=1:
            raise ValueError('Native/selected workload outputs differ')
        receipt=report['source_attestation']
        if sha(a.build_manifest)!=receipt['manifest_sha256'] or sha(a.archive)!=receipt['archive_sha256']:
            raise ValueError('Selected build identity changed')
        for identity in report['engines']:optimization_observation(identity)
        report['matched_checks']=report['results'][0]['check']['checks'];report['workloads']=report['results'][0]['check']['workloads']
        report['artifacts']={path.name:str(path) for path in work.iterdir() if path.is_file()}
        report['passed']=True
    except Exception as error:report['error']=str(error)
    write_report(output,report);print(json.dumps({key:report.get(key) for key in ['passed','error','work']}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
