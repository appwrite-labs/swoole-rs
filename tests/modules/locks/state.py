#!/usr/bin/env python3
"""Compare PHP integer/wrap/lock and actual ZTS concurrent-atomic observations."""
import argparse
import json
from pathlib import Path
import sys

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--engine',action='append',required=True);p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[]);p.add_argument('--require-zts',action='store_true');p.add_argument('--output',type=Path,default=ROOT/'target/reports/locks-php-state.json');a=p.parse_args()
    witness=Path(__file__).with_suffix('.php')
    report={'scope':'New PHP boundary witness, paired exact observations; ZTS shared-resource atomic increments run in real PHP threads when available. Not an original test or whole-family parity/performance proof.','results':[],'passed':False}
    try:
        for engine in parse_engines(a.engine,a.php,a.php_extension):
            metadata=engine.metadata();result=run_isolated(engine.command(witness),timeout=30)
            row={'engine':engine.name,'metadata':metadata,'exit':result.returncode,'stdout':result.stdout,'stderr':result.stderr,'status':'failed'}
            report['results'].append(row)
            if result.returncode or result.stderr:continue
            row['values']=json.loads(result.stdout)
            row['zts_exercised']='thread_counts' in row['values']
            if a.require_zts and not row['zts_exercised']:row['detail']='Required actual ZTS workers unavailable';continue
            row['status']='passed'
        rows=report['results'];values=[r.get('values') for r in rows]
        report['observations_match']=len(values)>=2 and all(v==values[0] for v in values)
        report['passed']=report['observations_match'] and all(r['status']=='passed' for r in rows)
    except Exception as e:report['error']=str(e)
    write_report(a.output,report);print(json.dumps(report));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
