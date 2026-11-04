#!/usr/bin/env python3
"""Actual private UDP DNS query/response, A/AAAA and request-ID failure witnesses."""
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
    fixture=Path(__file__).with_suffix('.php');peer=fixture.with_name('php_dns_peer.php')
    report=dict(passed=False,execution_status='running',scope=__doc__,created_utc=datetime.now(timezone.utc).isoformat(),results=[])
    try:
        engines=parse_engines(a.engine,a.php,a.php_extension);identities=[engine.metadata() for engine in engines];report['engines']=identities
        report['attestation']=attest(a.build_manifest,identities)
        outcomes={mode:[] for mode in ['a','aaaa','bad-id']}
        for engine,identity in zip(engines,identities):
            info=run_isolated(engine.command('--ri','swoole'),timeout=10)
            report.setdefault('backend_info',[]).append(dict(engine=engine.name,stdout=info.stdout,stderr=info.stderr,exit=info.returncode))
            if info.returncode or info.stderr:raise ValueError('Cannot attest native DNS backend')
            if 'c-ares =>' in info.stdout:raise ValueError('Unsupported c-ares backend bypasses the converted socket DNS codec')
            for mode in outcomes:
                result=run_isolated(engine.command(fixture,mode),timeout=35)
                row=dict(engine=engine.name,mode=mode,exit=result.returncode,stdout=result.stdout,stderr=result.stderr,extension_sha256_after=sha(identity['extension']))
                report['results'].append(row);write_report(a.output,report)
                if result.returncode or result.stderr:raise ValueError('Actual DNS fixture failed; raw outputs preserved')
                if row['extension_sha256_after']!=identity['sha256']:raise ValueError('Extension identity changed')
                row['summary']=json.loads(result.stdout);outcomes[mode].append(row['summary'])
        if any(len({json.dumps(value,sort_keys=True) for value in rows})!=1 for rows in outcomes.values()):raise ValueError('Native/selected DNS outcomes differ')
        report.update(passed=True,execution_status='complete')
    except Exception as error:report.update(execution_status='failed',error=f'{type(error).__name__}: {error}')
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','execution_status','error']}));return int(not report['passed'])
if __name__=='__main__':sys.exit(main())
