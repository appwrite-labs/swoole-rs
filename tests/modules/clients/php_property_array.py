#!/usr/bin/env python3
"""An array property replaced while its old value's __destruct writes it again keeps that write."""
import argparse
import json
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report

EXPECTED=dict(array=[True,['kept','a']],scalar=[False,'text'],set=[True,dict(timeout=1,keep_alive=False)])

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append',required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    report=dict(passed=False,scope=__doc__,results=[])
    try:
        for engine in parse_engines(a.engine,a.php,a.php_extension):
            result=run_isolated(engine.command(Path(__file__).with_suffix('.php')),timeout=30)
            row=dict(engine=engine.name,exit=result.returncode,stdout=result.stdout,stderr=result.stderr);report['results'].append(row)
            if result.returncode or result.stderr:raise ValueError(f'{engine.name}: fixture failed; raw outputs preserved')
            if json.loads(result.stdout)!=EXPECTED:raise ValueError(f'{engine.name}: unexpected outcome')
        report['passed']=True
    except Exception as error:report['error']=f'{type(error).__name__}: {error}'
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','error']}));return int(not report['passed'])
if __name__=='__main__':sys.exit(main())
