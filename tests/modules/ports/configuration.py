#!/usr/bin/env python3
"""Actual Port configuration conversion/reentry/rejection/lifetime; original suites remain separate."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report


def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def attest(manifest,archive,source_ref,metadata):
    data=json.loads(manifest.read_text());archive_hash=sha(archive)
    candidates=[m for m in metadata if m['rust_feature_bits']&8388608]
    if len(candidates)!=1 or candidates[0]['sha256']!=data['extension_sha256']:raise ValueError('Manifest does not identify exactly one actual M25 candidate')
    if not data['migrated_feature_bits']&8388608 or 'ports' not in data['inputs']['modules']:raise ValueError('Manifest lacks expected M25 feature8388608')
    if data['inputs']['rust_archive_sha256']!=archive_hash:raise ValueError('Selected archive does not match build manifest')
    module_source=data['inputs']['rust_sources_sha256']['crates/ports/src/lib.rs']
    if module_source!=sha(ROOT/'crates/ports/src/lib.rs'):raise ValueError('Manifest M25 source differs from this current socket-controller consumer')
    caller_digests={k:v for k,v in data['inputs']['headers_sha256'].items() if k.startswith('native/ports/')}
    caller_digests.update({item['path']:item['sha256'] for item in data['inputs']['patches'] if item['path'].startswith('migration/patches/ports/')})
    # Every ports header and patch the build applied, as this checkout has them.
    expected={str(path.relative_to(ROOT)) for pattern in ('native/ports/*.h','migration/patches/ports/*.patch') for path in ROOT.glob(pattern)}
    if set(caller_digests)!=expected or any(sha(ROOT/path)!=digest for path,digest in caller_digests.items()):raise ValueError('Manifest M25 header/callers differ from current socket-controller consumer')
    export=manifest.parent/'export.json'
    if export.exists():
        identity=json.loads(export.read_text())
        if identity['source_ref']!=source_ref:raise ValueError('Export source reference differs')
        expected=identity['files']
        if expected['build-manifest.json']['sha256']!=sha(manifest) or expected['libswoole_rs_ffi.a']['sha256']!=archive_hash or expected['swoole.so']['sha256']!=candidates[0]['sha256']:raise ValueError('Immutable export identities differ')
        kind='immutable producer export'
    else:
        blob=subprocess.check_output(['git','-C',str(ROOT),'show',source_ref+':crates/ports/src/lib.rs'])
        if hashlib.sha256(blob).hexdigest()!=module_source:raise ValueError('Frozen source reference differs from manifest M25 source')
        kind='frozen Git M25 source blob'
    symbols=subprocess.check_output(['nm','-g',candidates[0]['extension']],text=True,stderr=subprocess.DEVNULL)
    defined={parts[-1].lstrip('_') for line in symbols.splitlines() if len(parts:=line.split())>=2 and parts[-2].upper() in {'T','W'}}
    required=['number','protocol','protocol_name','event','socket_step','protocol_plan','framing_settings','stream_settings','defaults','callback_property','flag_setting','buffer_setting','packet_eof','packet_width','packet_offset','option','tls_option','tls_value','sni_plan','callback_hook']
    if not all('swr_v1_ports_'+name in defined for name in required):raise ValueError('Actual candidate DSO lacks required M25 controller exports')
    return {'passed':True,'source_ref':source_ref,'source_reference':kind,'required_candidate_dso_exports':required}

CASES={
    'convert_eof':None, 'convert_length_type':None, 'settings_release_reentry':None,
    'callback_release_reentry':None, 'ignored_ssl':None,
    'fatal_empty_eof':'package_eof cannot be an empty string',
    'fatal_long_eof':'package_eof max length is 8',
    'fatal_length_type':'unknown package_length_type',
    'fatal_length_offset':"'package_length_offset' value is too large",
    'fatal_body_offset':"'package_body_offset' value is too large",
    'fatal_body_alias':"'package_body_offset' value is too large",
    'conversion_throw':'unknown package_length_type',
}


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--engine',action='append',required=True);p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[]);p.add_argument('--build-manifest',type=Path,required=True);p.add_argument('--archive',type=Path,required=True);p.add_argument('--source-ref',required=True);p.add_argument('--case',action='append',choices=list(CASES));p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    script=Path(__file__).with_suffix('.php');engines=parse_engines(a.engine,a.php,a.php_extension)
    report={'scope':__doc__,'engines':[e.metadata() for e in engines],'results':[],'passed':False,'unmeasured_cases':[c for c in CASES if a.case and c not in a.case]}
    try:report['candidate_attestation']=attest(a.build_manifest,a.archive,a.source_ref,report['engines'])
    except Exception as error:report['candidate_attestation']={'passed':False,'error':str(error)};write_report(a.output,report);return 1
    for engine in engines:
        for case,message in CASES.items():
            if a.case and case not in a.case:continue
            command=engine.command('-d','display_errors=stderr',script,case);r=run_isolated(command,timeout=20)
            observations=[]
            for line in r.stdout.splitlines():
                if line.startswith('OBS '):observations.append(json.loads(line[4:]))
            output=r.stdout+r.stderr
            if case=='conversion_throw':
                # Upstream reports the __toString exception as uncaught, then the failed conversion as fatal (#317).
                expected=(r.returncode==255 and 'Uncaught RuntimeException: port conversion sentinel' in output
                          and 'unknown package_length_type' in output
                          and observations==[{'case':case,'shutdown':True,'error_type':1,'conversions':1,'reentry':True,'setting_merged':False}])
            elif message is None:
                expected=r.returncode==0 and len(observations)==1
            else:
                expected=(r.returncode==255 and message in output and observations==[{'case':case,'shutdown':True,'error_type':1,'raw_backlog':17,'callback_retained':True}])
            report['results'].append({'engine':engine.name,'case':case,'command':command,'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr,'observations':observations,'expected_rejection':message,'passed':expected});write_report(a.output,report)
    pairs={case:[(row['exit'],row['observations']) for row in report['results'] if row['case']==case] for case in CASES if not a.case or case in a.case}
    report['observations_match']=all(all(value==values[0] for value in values) for values in pairs.values())
    report['backend_comparison_present']={bool(e['rust_feature_bits']&8388608) for e in report['engines']}=={False,True}
    report['passed']=bool(report['results']) and not report['unmeasured_cases'] and report['backend_comparison_present'] and report['observations_match'] and all(r['passed'] for r in report['results'])
    write_report(a.output,report);print(json.dumps({'passed':report['passed'],'cases':len(pairs),'failed':[(r['engine'],r['case'],r['exit']) for r in report['results'] if not r['passed']]}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
