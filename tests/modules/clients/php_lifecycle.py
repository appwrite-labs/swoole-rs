#!/usr/bin/env python3
"""Actual async Client close, callback reentry, owner release, GC and peer EOF.

Private builtin-stream child processes use ephemeral loopback ports; no fixed
fixtures/global logs. Does not prove TLS/proxy/all-client/native ASan parity.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import sys

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.ext_identity import attest
from tools.harness import parse_engines,run_isolated,write_report


def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php',default='php')
    p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append',required=True)
    p.add_argument('--build-manifest',type=Path,help='Required selected extension source/overlay attestation')
    p.add_argument('--native-only',action='store_true',help='Prepare native control evidence only; passed remains false')
    p.add_argument('--output',required=True,type=Path)
    a=p.parse_args();fixture=Path(__file__).with_suffix('.php');peer=fixture.with_name('php_peer.php')
    report=dict(created_utc=datetime.now(timezone.utc).isoformat(),passed=False,scope=__doc__,execution_status='running',engines=[],results=[])
    try:
        write_report(a.output,report)
        engines=parse_engines(a.engine,a.php,a.php_extension);report['engines']=[engine.metadata() for engine in engines]
        kinds={bool(identity['rust_feature_bits']&1048576) for identity in report['engines']}
        if kinds!={False,True} and not (a.native_only and kinds=={False}):raise ValueError('Require actual native and clients-bit1048576 engines')
        if a.native_only and kinds!={False}:raise ValueError('Native-only mode cannot credit selected code')
        if not a.native_only:
            if not a.build_manifest:raise ValueError('Selected lifecycle proof requires --build-manifest')
            selected=next(identity for identity in report['engines'] if identity['rust_feature_bits']&1048576)
            report['build_attestation']=attest(a.build_manifest,selected,1048576)
        summaries={mode:[] for mode in ['connect','receive']}
        for engine,identity in zip(engines,report['engines']):
            for mode in summaries:
                result=run_isolated(engine.command(fixture,mode),timeout=35)
                row=dict(engine=engine.name,mode=mode,exit=result.returncode,stdout=result.stdout,stderr=result.stderr,extension_sha256_after=sha(identity['extension']))
                report['results'].append(row);write_report(a.output,report)
                if row['extension_sha256_after']!=identity['sha256']:raise ValueError('Engine identity changed during fixture')
                if result.returncode or result.stderr:raise ValueError('Actual PHP lifecycle fixture failed; raw outputs retained')
                row['summary']=json.loads(result.stdout)
                # TCP receive chunk boundaries are not deterministic across two
                # independent sockets. Preserve every chunk in raw evidence and
                # compare exact bytes/state/order; fixture checks bounded total.
                normalized={key:value for key,value in row['summary'].items() if key!='receive_chunks'}
                if ''.join(row['summary']['receive_chunks'])!=row['summary']['response']:raise ValueError('Recorded chunks do not assemble exact response')
                summaries[mode].append(normalized)
        if any(len({json.dumps(value,sort_keys=True) for value in values})!=1 for values in summaries.values()):raise ValueError('Native/selected PHP lifecycle outcomes differ')
        if a.native_only:
            report.update(execution_status='native_control_only',control_passed=True,remaining='No selected Rust client engine executed; full gate remains unpassed')
        else:report.update(execution_status='complete',passed=True)
    except Exception as error:report.update(execution_status='failed',error=f'{type(error).__name__}: {error}')
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','execution_status','error']}));return int(not (report['passed'] or report.get('control_passed')))


if __name__=='__main__':sys.exit(main())
