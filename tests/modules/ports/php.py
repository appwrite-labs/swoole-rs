#!/usr/bin/env python3
"""Independent actual PHP Port callback/settings/reentry/lifetime consumer; originals separate."""
import argparse
import json
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--engine',action='append',required=True);p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[]);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    script=Path(__file__).with_suffix('.php');report={'scope':__doc__,'engines':[],'results':[],'passed':False,'backend_comparison_present':False}
    for engine in parse_engines(a.engine,a.php,a.php_extension):
        report['engines'].append(engine.metadata());r=run_isolated(engine.command(script),timeout=30);row={'engine':engine.name,'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr};report['results'].append(row)
        if not r.returncode:
            try:row['observations']=json.loads(r.stdout.strip().splitlines()[-1])
            except (ValueError,IndexError):row['parse_failed']=True
    report['backend_comparison_present']={bool(e['rust_feature_bits']&8388608) for e in report['engines']}=={False,True}
    report['passed']=bool(report['results']) and all(r['exit']==0 and 'observations' in r for r in report['results']) and all(r.get('observations')==report['results'][0].get('observations') for r in report['results'])
    write_report(a.output,report);print(json.dumps({'passed':report['passed'],'backend_comparison_present':report['backend_comparison_present'],'results':report['results']}));return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
