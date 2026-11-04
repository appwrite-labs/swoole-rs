#!/usr/bin/env python3
"""Actual PHP resolver cluster retry, binary results, callback changes, exceptions and bailout."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report

from php_attestation import sha,attest

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append',required=True);p.add_argument('--build-manifest',type=Path)
    p.add_argument('--native-only',action='store_true',help='Prepare native controls only, passed remains false')
    p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    fixture=Path(__file__).with_suffix('.php')
    report=dict(passed=False,execution_status='running',scope=__doc__,created_utc=datetime.now(timezone.utc).isoformat(),results=[])
    try:
        engines=parse_engines(a.engine,a.php,a.php_extension);identities=[engine.metadata() for engine in engines];report['engines']=identities
        if a.native_only:
            if len(identities)!=1 or identities[0]['rust_feature_bits']&1048576:raise ValueError('Native-only requires one unselected engine')
        else:
            if not a.build_manifest:raise ValueError('Selected resolver proof requires --build-manifest')
            report['attestation']=attest(a.build_manifest,identities)
        outcomes={mode:[] for mode in ['string','cycle','mutate-port','missing-port','lookup-throw','count-throw','pop-throw','bailout']}
        for engine,identity in zip(engines,identities):
            for mode in outcomes:
                result=run_isolated(engine.command(fixture,mode),timeout=35)
                row=dict(engine=engine.name,mode=mode,exit=result.returncode,stdout=result.stdout,stderr=result.stderr,extension_sha256_after=sha(identity['extension']))
                report['results'].append(row);write_report(a.output,report)
                if result.returncode!=(43 if mode=='bailout' else 0) or result.stderr:raise ValueError('Actual resolver fixture failed; raw outputs preserved')
                if row['extension_sha256_after']!=identity['sha256']:raise ValueError('Extension identity changed')
                row['summary']=dict(exit=result.returncode,stdout=result.stdout) if mode=='bailout' else json.loads(result.stdout)
                if mode=='bailout' and result.stdout!='native-effect-bailout\n':raise ValueError('Unexpected bailout prefix')
                outcomes[mode].append(row['summary'])
        if any(len({json.dumps(value,sort_keys=True) for value in rows})!=1 for rows in outcomes.values()):raise ValueError('Native/selected resolver outcomes differ')
        if a.native_only:report.update(execution_status='native_control_only',control_passed=True,remaining='No converted resolver-binding engine executed; Rust runtime gate remains unpassed')
        else:report.update(passed=True,execution_status='complete')
    except Exception as error:report.update(execution_status='failed',error=f'{type(error).__name__}: {error}')
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','execution_status','error']}));return int(not (report['passed'] or report.get('control_passed')))
if __name__=='__main__':sys.exit(main())
