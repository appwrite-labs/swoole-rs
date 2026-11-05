#!/usr/bin/env python3
"""Unchanged local TLS PHPTs against published PHP extensions; no builds."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from corpus.upstream.run import COMMIT, prepare, record_result, run_cases, save_report
from tools.harness import parse_engines


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def artifact_identity(export, build):
    """Strict identity view of either actual root export schema; preserve raw input."""
    bits=build['migrated_feature_bits']
    if 'files' in export:
        files=export['files']
        source=export['source_ref']
        if not re.fullmatch(r'[0-9a-f]{40}',source):raise ValueError('Invalid frozen source ref')
        rows=[e for e in export['engines']if e.get('name')=='hybrid']
        if len(rows)!=1 or rows[0]['rust_feature_bits']!=bits:
            raise ValueError('Export hybrid feature metadata differs from build')
        view=dict(schema='files-v1',selected_source_commit=source,
            manifest_sha256=files['build-manifest.json']['sha256'],
            extension_sha256=files['swoole.so']['sha256'],archive_sha256=files['libswoole_rs_ffi.a']['sha256'],
            reference_sha256=files['reference.so']['sha256'],
            reference_build_attestation=export.get('reference_build_attestation'))
    else:
        if export['feature_bits']!=bits:raise ValueError('Export feature bits differ from build')
        view=dict(schema='legacy',selected_source_commit=export['selected_source_commit'],
            manifest_sha256=export['manifest_sha256'],extension_sha256=export['extension']['sha256'],
            archive_sha256=export['archive']['sha256'],reference_sha256=export['reference']['sha256'],
            reference_build_attestation=export['reference'].get('build_attestation'))
    for key,value in view.items():
        if key.endswith('_sha256') and not re.fullmatch(r'[0-9a-f]{64}',value):
            raise ValueError('Malformed exported hash: '+key)
    return dict(view,feature_bits=bits)


@contextmanager
def matching_child_php(engine, directory):
    """Original shell_exec('php ...') gets the exact parent's engine/options."""
    directory.mkdir(parents=True)
    wrapper=directory/'php'
    wrapper.write_text('#!/bin/sh\nexec '+shlex.join(engine.command('-d','zend.assertions=1'))+' "$@"\n')
    wrapper.chmod(0o755)
    previous=os.environ.get('PATH')
    os.environ['PATH']=str(directory)+os.pathsep+(previous or '')
    try:yield {'path':str(wrapper),'exec_argv':engine.command('-d','zend.assertions=1')}
    finally:
        if previous is None:os.environ.pop('PATH',None)
        else:os.environ['PATH']=previous


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--runner', type=Path, required=True, help='Matching SDK official run-tests.php')
    parser.add_argument('--manifest', type=Path, default=ROOT/'tests/modules/tls/php-local-cases.json')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--container-inspect', type=Path, help='Host-captured filtered Docker inspect with immutable Image ID and resource custody')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--timeout', type=int, default=45)
    parser.add_argument('--dh-witness', action='store_true', help='Additionally run owned actual PHP DHE/DH-setting witness with pinned support, not an original case')
    parser.add_argument('--dh-witness-only', action='store_true', help='Run only that owned witness; original manifest remains unexecuted planning metadata')
    args = parser.parse_args()
    if args.dh_witness_only:args.dh_witness=True
    args.php=shutil.which(args.php) or args.php
    attempt=args.output.resolve().parent/(args.output.stem+'-'+uuid.uuid4().hex)
    attempt.mkdir(parents=True)
    report=dict(passed=False, execution_status='running', created_utc=datetime.now(timezone.utc).isoformat(),
        scope='Selected unchanged local original PHP TLS/DTLS cases; actual published selected/reference extensions. No builds, benchmarks, PHP/system OOM or whole TLS completion claim.',
        attempt=str(attempt), commands=[], engines=[], results=[], batches=[])
    if args.output.exists():shutil.copy2(args.output,attempt/'previous-summary.json')
    def command(argv):
        completed=subprocess.run(list(map(str,argv)),capture_output=True,text=True,timeout=60)
        output=completed.stdout+completed.stderr
        log=attempt/(str(len(report['commands'])).zfill(2)+'.log.gz')
        log.write_bytes(gzip.compress(output.encode(),mtime=0))
        report['commands'].append(dict(argv=list(map(str,argv)),returncode=completed.returncode,log=str(log)))
        if completed.returncode:raise ValueError('Preflight command failed: '+str(log))
        return completed.stdout
    try:
        if any(os.environ.get(k) for k in ['LD_PRELOAD','LD_AUDIT','DYLD_INSERT_LIBRARIES']):
            raise ValueError('Injected runtime requires separate matched validation')
        manifest_path=args.manifest.resolve()
        manifest=json.loads(manifest_path.read_text());original_cases=list(manifest['tests']);cases=[]if args.dh_witness_only else list(original_cases)
        assert original_cases and len({case['path']for case in original_cases})==len(original_cases)
        report['imported_helpers']={name:{'path':str(Path(sys.modules[name].__file__).resolve()),'sha256':sha(sys.modules[name].__file__)} for name in ['corpus.upstream.run','tools.harness','tools.build_upstream']}
        if args.container_inspect:
            inspected=json.loads(args.container_inspect.read_text())
            assert re.fullmatch(r'sha256:[0-9a-f]{64}',inspected['image_id'])
            report['container_inspect']={'path':str(args.container_inspect),'observed':inspected}
        support_files=list(manifest['support_files']);owned_case=None
        if args.dh_witness:
            owned_source=ROOT/'tests/modules/tls/php-dh.phpt'
            owned_case={'path':'tests/modules/tls/php-dh.phpt','sha256':sha(owned_source),'module':'tls-owned-dh',
                'origin':'New owned module witness; not a pinned original assertion',
                'requires':['upstream_test_bootstrap','processes','atomics','tcp']}
            cases.append(owned_case)
            support_files.append({'path':'tests/include/ssl_certs/dhparams.pem','sha256':'4d79a7f70746bafb6a9d13e6dbc35b80bf2ef0c4ca9356a41433a9147147e0c1'})
            report['additional_cases']=[owned_case]
        report['original_case_count']=0 if args.dh_witness_only else len(original_cases)
        if args.dh_witness_only:report['scope']+=' Owned DH witness only; original manifest planning cases were not executed.'
        report.update(driver_sha256=sha(__file__),manifest_sha256=sha(manifest_path),manifest=manifest,
                      manifest_role='Pinned selection planning; embedded NOT_RUN fields are not runtime results',
                      artifact_export=json.loads((args.artifacts/'export.json').read_text()))
        export=report['artifact_export'];build=json.loads((args.artifacts/'build-manifest.json').read_text())
        assert manifest['commit']==COMMIT==build['inputs']['upstream_commit']
        identity=artifact_identity(export,build);report['artifact_identity']=identity
        assert sha(args.artifacts/'build-manifest.json')==identity['manifest_sha256']
        expected={args.artifacts/'swoole.so':identity['extension_sha256'],args.artifacts/'reference.so':identity['reference_sha256'],args.artifacts/'libswoole_rs_ffi.a':identity['archive_sha256']}
        assert build['extension_sha256']==identity['extension_sha256']
        assert build['inputs']['rust_archive_sha256']==identity['archive_sha256']
        for path,digest in expected.items():assert sha(path)==digest,(str(path),'artifact identity differs')
        report['build_manifest']=build
        report['reference_build_attestation']=identity['reference_build_attestation']
        report['reference_identity_scope']='Attested build' if report['reference_build_attestation'] else 'Observed artifact identity only; no native reference build/source attestation supplied'
        report['php_runtime']={name:command(argv).strip() for name,argv in [('version',[args.php,'-n','-v']),('config',['php-config','--configure-options']),('phpize',['phpize','--version'])]}
        assert report['php_runtime']['config'].strip()==build['inputs']['php_config'].strip(),'PHP SDK configure options differ from selected build'
        assert Path(shutil.which('phpize')).resolve()==Path(build['inputs']['phpize']).resolve(),'PHP SDK phpize executable differs from selected build'
        report['sdk_executables']={name:{'path':shutil.which(name),'sha256':sha(shutil.which(name))}for name in [args.php,'php-config','phpize']}
        report['runner_sha256']=sha(args.runner)
        archive_symbols=command(['nm','-g',args.artifacts/'libswoole_rs_ffi.a'])
        names=sorted(set(re.findall(r'\b[Tt] _?swr_v1_tls_([A-Za-z0-9_]+)\s*$',archive_symbols,re.M)))
        assert set(['context_create','dtls_new','dtls_append','dtls_read','dtls_control','dtls_listen'])<=set(names),'Published archive lacks TLS exports'
        report['archive_tls_exports']=names
        report['caller_proof']={};crypto={}
        for name,filename,selected in [('hybrid','swoole.so',True),('upstream','reference.so',False)]:
            extension=args.artifacts/filename
            nm=command(['nm','-g',extension]);disassembly=command(['objdump','-d','-C',extension])
            calls={n:bool(re.search(r'\b(?:bl|callq?|b|jmpq?)\s+[^\n]*\bswr_v1_tls_'+n+r'\b',disassembly))for n in names}
            defs={n:bool(re.search(r'\b[Tt] swr_v1_tls_'+n+r'\s*$',nm,re.M))for n in names}
            assert all(calls.values()) and all(defs.values()) if selected else not any(calls.values()) and 'swr_v1_tls_' not in nm
            report['caller_proof'][name]=dict(call_instructions=calls,definitions=defs)
            libraries=command(['ldd',extension]);paths=re.findall(r'lib(?:ssl|crypto)[^ ]* => (/[^ ]+)',libraries)
            assert len(set(paths))>=2,'Actual libssl/libcrypto resolution missing'
            for path in paths:crypto[path]=sha(path)
        report['crypto_before']=crypto
        engines=parse_engines(['hybrid='+str(args.artifacts/'swoole.so'),'upstream='+str(args.artifacts/'reference.so')],args.php,args.php_extension)
        with tempfile.TemporaryDirectory(prefix='m21-php-') as temporary:
            sandbox=Path(temporary)
            prepare(args.source,sandbox,original_cases,support_files=support_files)
            if owned_case:
                target=sandbox/owned_case['path'];target.parent.mkdir(parents=True,exist_ok=True)
                shutil.copy2(owned_source,target);assert sha(target)==owned_case['sha256']
            shutil.copy2(args.runner,sandbox/'run-tests.php')
            assert sha(sandbox/'run-tests.php')==report['runner_sha256']
            (sandbox/'home').mkdir();(sandbox/'tmp').mkdir()
            for engine in engines:
                with matching_child_php(engine,attempt/engine.name/'bin') as child_php:
                    metadata=engine.metadata();metadata['child_php_selector']=child_php;report['engines'].append(metadata)
                    observed_child=json.loads(command(['php','-r','echo json_encode([PHP_VERSION,defined("SWOOLE_RS_FEATURE_BITS")?SWOOLE_RS_FEATURE_BITS:0]);']))
                    assert observed_child==[metadata['php'],metadata['rust_feature_bits']]
                    assert metadata['php']==build['inputs']['php']
                    assert metadata['rust_feature_bits']==(identity['feature_bits']if engine.name=='hybrid'else 0)
                    info=command(engine.command('-r','echo json_encode(["openssl"=>OPENSSL_VERSION_TEXT,"zts"=>PHP_ZTS,"ssl"=>defined("SWOOLE_SSL")]);'))
                    metadata['ssl_runtime']=json.loads(info)
                    assertions=command(engine.command('-d','zend.assertions=1','-r','echo json_encode(["zend.assertions"=>ini_get("zend.assertions"),"assert.active"=>ini_get("assert.active"),"assert.exception"=>ini_get("assert.exception")]);'))
                    metadata['official_runner_assertion_options']=json.loads(assertions)
                    assert metadata['official_runner_assertion_options']['zend.assertions']=='1'
                    for index,case in enumerate(cases):
                        missing=sorted(set(case['requires'])-set(metadata['capabilities']))
                        if missing:record_result(report,engine,case,dict(status='unsupported',detail='Missing capabilities: '+repr(missing)))
                        else:
                            results,batch=run_cases(engine,[case],sandbox,attempt/engine.name/str(index).zfill(2),args.timeout,1)
                            report['batches'].append(dict(engine=engine.name,**batch));record_result(report,engine,case,results[case['path']])
                            if case['path'].endswith('/client_ca.phpt') and Path('/tmp/client_ca.txt').is_file():
                                diagnostic=attempt/engine.name/'client-ca-curl.txt';shutil.copyfile('/tmp/client_ca.txt',diagnostic)
                                report.setdefault('client_ca_diagnostics',{})[engine.name]={'path':str(diagnostic),'scope':'Observed immediately after this engine original case; private container tmp required'}
                        save_report(args.output,report)
        for path,digest in expected.items():assert sha(path)==digest,'Published artifact changed during run'
        assert all(sha(path)==digest for path,digest in crypto.items()),'Crypto libraries changed'
        assert sha(__file__)==report['driver_sha256'] and sha(manifest_path)==report['manifest_sha256']
        assert sha(args.runner)==report['runner_sha256']
        if owned_case:assert sha(owned_source)==owned_case['sha256']
        assert all(sha(row['path'])==row['sha256']for row in report['imported_helpers'].values())
        assert all(sha(row['path'])==row['sha256']for row in report['sdk_executables'].values())
        report.update(execution_status='complete',passed=len(report['results'])==2*len(cases) and all(r['status']=='passed' for r in report['results']))
    except Exception as error:
        report.update(execution_status='failed',error=repr(error))
    save_report(args.output,report)
    print(json.dumps({k:report.get(k)for k in ['passed','execution_status','error','attempt']}))
    return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
