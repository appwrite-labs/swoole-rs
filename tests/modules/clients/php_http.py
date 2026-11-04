#!/usr/bin/env python3
"""Actual coroutine HTTP retry, settings mutation from callback and cookie bytes."""
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
    p.add_argument('--engine',action='append',required=True);p.add_argument('--build-manifest',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    fixture=Path(__file__).with_suffix('.php');peer=fixture.with_name('php_http_peer.php')
    report=dict(passed=False,execution_status='running',scope=__doc__,created_utc=datetime.now(timezone.utc).isoformat(),results=[])
    try:
        engines=parse_engines(a.engine,a.php,a.php_extension);identities=[engine.metadata() for engine in engines];report['engines']=identities
        report['attestation']=attest(a.build_manifest,identities)
        outcomes={mode:[] for mode in ['retry','callback']}
        for engine,identity in zip(engines,identities):
            for mode in outcomes:
                result=run_isolated(engine.command(fixture,mode),timeout=35)
                row=dict(engine=engine.name,mode=mode,exit=result.returncode,stdout=result.stdout,stderr=result.stderr,extension_sha256_after=sha(identity['extension']))
                report['results'].append(row);write_report(a.output,report)
                if result.returncode or result.stderr:raise ValueError('Actual HTTP fixture failed; raw outputs preserved')
                if row['extension_sha256_after']!=identity['sha256']:raise ValueError('Extension identity changed')
                row['summary']=json.loads(result.stdout);outcomes[mode].append(row['summary'])
        if any(len({json.dumps(value,sort_keys=True) for value in rows})!=1 for rows in outcomes.values()):raise ValueError('Native/selected HTTP outcomes differ')
        report.update(passed=True,execution_status='complete')
    except Exception as error:report.update(execution_status='failed',error=f'{type(error).__name__}: {error}')
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','execution_status','error']}));return int(not report['passed'])
if __name__=='__main__':sys.exit(main())
