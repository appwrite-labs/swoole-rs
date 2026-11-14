#!/usr/bin/env python3
"""Actual module INI handler/access/restore and information-output bailout parity."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report
from tools.ext_identity import attest


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append',required=True);p.add_argument('--build-manifest',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    report=dict(created_utc=datetime.now(timezone.utc).isoformat(),passed=False,scope=__doc__,results=[])
    try:
        engines=parse_engines(a.engine,a.php,a.php_extension);identities=[engine.metadata() for engine in engines]
        report['engines']=identities
        if {bool(row['rust_feature_bits'] &16777216) for row in identities}!={False,True}:raise ValueError('Require native and PHPhelpers-selected engines')
        selected=next(row for row in identities if row['rust_feature_bits'] &16777216)
        report['source_attestation']=attest(a.build_manifest,selected,16777216)
        outputs=[]
        for engine,identity in zip(engines,identities):
            for mode in ['ordinary','output-exit']:
                result=run_isolated(engine.command(Path(__file__).with_suffix('.php'),mode),timeout=20)
                row=dict(engine=engine.name,mode=mode,exit=result.returncode,stdout=result.stdout,stderr=result.stderr)
                report['results'].append(row);write_report(a.output,report)
                if hashlib.sha256(engine.extension.read_bytes()).hexdigest()!=identity['sha256']:raise ValueError('Extension changed during module witness')
                if result.stderr or result.returncode!=(37 if mode=='output-exit' else 0):raise ValueError('Module PHP witness failed; original output retained')
                if mode=='ordinary':
                    observation=json.loads(result.stdout)
                    # Build timestamps and the explicitly added bridge identity
                    # describe different binaries; every upstream row stays exact.
                    observation['info']=re.sub(r'^(Built|Rust migration bridge) => .*\n','',observation['info'],flags=re.M)
                    outputs.append(observation)
        if len({json.dumps(row,sort_keys=True) for row in outputs})!=1:raise ValueError('Native/selected module behavior differs')
        report.update(passed=True,execution_status='complete')
    except Exception as error:report.update(execution_status='failed',error=f'{type(error).__name__}: {error}')
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','execution_status','error']}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
