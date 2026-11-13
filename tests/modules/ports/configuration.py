#!/usr/bin/env python3
"""Actual Port configuration conversion/reentry/rejection/lifetime; original suites remain separate."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report
from tools.ext_identity import attest as ext_attest


def attest(manifest,metadata):
    candidates=[m for m in metadata if m['rust_feature_bits']&8388608]
    if len(candidates)!=1:raise ValueError('Require exactly one M25 candidate engine')
    receipt=ext_attest(manifest,candidates[0],8388608)
    symbols=subprocess.check_output(['nm','-g',candidates[0]['extension']],text=True,stderr=subprocess.DEVNULL)
    defined={parts[-1].lstrip('_') for line in symbols.splitlines() if len(parts:=line.split())>=2 and parts[-2].upper() in {'T','W'}}
    required=['event','callback_property','callback_hook']
    if not all('swr_v1_ports_'+name in defined for name in required):raise ValueError('Actual candidate DSO lacks required M25 controller exports')
    return {'passed':True,**receipt,'required_candidate_dso_exports':required}

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
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--engine',action='append',required=True);p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[]);p.add_argument('--build-manifest',type=Path,required=True);p.add_argument('--case',action='append',choices=list(CASES));p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    script=Path(__file__).with_suffix('.php');engines=parse_engines(a.engine,a.php,a.php_extension)
    report={'scope':__doc__,'engines':[e.metadata() for e in engines],'results':[],'passed':False,'unmeasured_cases':[c for c in CASES if a.case and c not in a.case]}
    try:report['candidate_attestation']=attest(a.build_manifest,report['engines'])
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
