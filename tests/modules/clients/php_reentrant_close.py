#!/usr/bin/env python3
"""A persistent Client closed from its package_length_func during recv(): not pooled, key reconnects fresh.
A Coroutine\\Http\\Client reentered from write_func (a request or recv there fails) and from another coroutine while a request yields.
An upgraded client's recv() from a second coroutine while the first waits on a frame."""
import argparse
import json
import re
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import parse_engines,run_isolated,write_report

EXPECTED=dict(outer=dict(received=False,errCode=5001,connected=False),fresh=dict(reuse=False,reuse_count=0),
              peer=dict(accepts=2,eof=True),
              http=dict(get=dict(ok=True,status=200,first=['a'],second=[['b',0,False],['c',0,False]],
                                 nested=dict(get=[False,True],post=[False,True],recv=[False,True])),
                        download=dict(ok=True,status=200,body='abc'),used_while_yielded=True),warnings=[])
BOUND=re.compile(r'second recv\n\nFatal error: Uncaught Swoole\\Error: Socket#\d+ has already been bound to another coroutine#\d+, '
                 r'reading of the same socket in coroutine#\d+ at the same time is not allowed')

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php',default='php');p.add_argument('--php-extension',action='append',default=[])
    p.add_argument('--engine',action='append',required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    report=dict(passed=False,scope=__doc__,results=[])
    try:
        for engine in parse_engines(a.engine,a.php,a.php_extension):
            result=run_isolated(engine.command(Path(__file__).with_suffix('.php')),timeout=35)
            row=dict(engine=engine.name,exit=result.returncode,stdout=result.stdout,stderr=result.stderr);report['results'].append(row)
            if result.returncode or result.stderr:raise ValueError(f'{engine.name}: fixture failed; raw outputs preserved')
            if json.loads(result.stdout)!=EXPECTED:raise ValueError(f'{engine.name}: unexpected outcome')
            result=run_isolated(engine.command(Path(__file__).with_name('php_reentrant_close_websocket.php')),timeout=35)
            report['results'].append(dict(engine=engine.name,exit=result.returncode,stdout=result.stdout,stderr=result.stderr))
            if result.returncode!=255 or not BOUND.match(result.stdout) or 'unreachable' in result.stdout:
                raise ValueError(f'{engine.name}: websocket recv from a second coroutine did not stop with the bound-socket error')
        report['passed']=True
    except Exception as error:report['error']=f'{type(error).__name__}: {error}'
    write_report(a.output,report);print(json.dumps({key:report.get(key) for key in ['passed','error']}));return int(not report['passed'])
if __name__=='__main__':sys.exit(main())
