#!/usr/bin/env python3
"""Actual imported socket/stream ownership, binary IO, factory fallback and hooked pair release."""
import argparse
from datetime import datetime,timezone
import json
from pathlib import Path
import sys
import subprocess
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report
from php_attestation import sha,attest


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append',required=True);p.add_argument('--build-manifest',type=Path,required=True)
    p.add_argument('--originals',action='store_true',help='Run the unchanged pinned PHPT selection with the official runner; preserve its failure status')
    p.add_argument('--source',type=Path,default=ROOT/'target/upstream/src')
    p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    if a.originals:
        command=[sys.executable,str(ROOT/'corpus/upstream/run.py'),'--source',str(a.source),
                 '--manifest',str(Path(__file__).with_name('socket-custody-originals.json')),
                 '--php',a.php,'--require-all','--isolate-cases','--output',str(a.output)]
        command += [part for engine in a.engine for part in ('--engine',engine)]
        command += [part for extension in a.php_extension for part in ('--php-extension',extension)]
        return subprocess.run(command,check=False).returncode
    report=dict(passed=False,execution_status='running',scope=__doc__,created_utc=datetime.now(timezone.utc).isoformat(),results=[])
    try:
        engines=parse_engines(a.engine,a.php,a.php_extension);identities=[engine.metadata() for engine in engines];report['engines']=identities
        report['attestation']=attest(a.build_manifest,identities)
        outcomes=[]
        for engine,identity in zip(engines,identities):
            result=run_isolated(engine.command(Path(__file__).with_suffix('.php')),timeout=30)
            row=dict(engine=engine.name,exit=result.returncode,stdout=result.stdout,stderr=result.stderr,extension_sha256_after=sha(identity['extension']))
            report['results'].append(row);write_report(a.output,report)
            if result.returncode or result.stderr:raise ValueError('Actual ownership fixture failed; raw outputs preserved')
            if row['extension_sha256_after']!=identity['sha256']:raise ValueError('Extension identity changed')
            row['summary']=json.loads(result.stdout);outcomes.append(row['summary'])
        if len({json.dumps(value,sort_keys=True) for value in outcomes})!=1:raise ValueError('Native/selected socket custody differs')
        report.update(passed=True,execution_status='complete')
    except Exception as error:report.update(execution_status='failed',error=f'{type(error).__name__}: {error}')
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','execution_status','error']}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
