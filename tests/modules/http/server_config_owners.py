#!/usr/bin/env python3
"""Exercise the production Rust Server collection views and shared context lifetime."""
import argparse
from pathlib import Path
import sys
import tempfile

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT))
from tools.harness import run_isolated,write_report
from tools.build_ext import native_link


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    prefix=args.archive.resolve().parents[1]/'include'
    report={'scope':__doc__,'passed':False}
    with tempfile.TemporaryDirectory(prefix='swr-server-config-') as directory:
        executable=Path(directory)/'owners'
        command=['c++','-std=c++14','-O1','-UNDEBUG','-I'+str(prefix),str(Path(__file__).with_suffix('.cc')),*native_link(args.archive),'-o',str(executable)]
        result=run_isolated(command,timeout=60)
        report['compile']={'command':command,'exit_code':result.returncode,'stderr':result.stderr}
        if result.returncode==0:
            result=run_isolated([str(executable)],timeout=15)
            report['execution']={'exit_code':result.returncode,'stdout':result.stdout,'stderr':result.stderr}
            report['passed']=result.returncode==0
    write_report(args.output,report)
    return int(not report['passed'])


if __name__=='__main__':sys.exit(main())
