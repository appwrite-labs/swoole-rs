#!/usr/bin/env python3
"""Real ZTS normal thread/atomic/barrier/owned mutex consumer; no cross-owner unlock or fault injection."""
import argparse
import json
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--engine',action='append',required=True);p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[]);p.add_argument('--output',type=Path,required=True);a=p.parse_args();fixture=Path(__file__).with_suffix('.php')
    report={'scope':__doc__,'engines':[],'results':[],'passed':False,'full_original_gate_passed':False,'sanitizer_qualified':False,'performance_qualified':False}
    for engine in parse_engines(a.engine,a.php,a.php_extension):
        metadata=engine.metadata();report['engines'].append(metadata)
        row={'engine':engine.name,'command':engine.command(fixture),'passed':False};report['results'].append(row)
        if metadata.get('zts') != 1:row['error']='Real ZTS metadata required';continue
        r=run_isolated(row['command'],timeout=45);row.update(exit=r.returncode,stdout=r.stdout,stderr=r.stderr)
        if r.returncode==0:
            try:
                observation=json.loads(r.stdout.strip());row['observations']=observation
                row['passed']=observation=={'workers':2,'atomic32':20000,'atomic64':20000,'mutex_protected':2000,'barrier_participants':3,'same_thread_unlock':True}
            except ValueError:row['error']='Expected exact JSON observations'
        write_report(a.output,report)
    report['backend_comparison_present']={bool(e['rust_feature_bits']&1024) for e in report['engines']}=={False,True}
    report['passed']=bool(report['results']) and report['backend_comparison_present'] and all(r['passed']for r in report['results']);write_report(a.output,report);print(json.dumps({'passed':report['passed'],'results':report['results']}));return int(not report['passed'])

if __name__=='__main__':sys.exit(main())
