#!/usr/bin/env python3
"""Actual original/selected cookie bodies: ordinary callbacks, no fault/reentry/alias profiles."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from gate import PIN, body, add_archive_arguments, prepare_archive, Sides
from tools.harness import run_isolated, write_report

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    add_archive_arguments(parser)
    args=parser.parse_args()
    if args.output.exists():parser.error('Refusing an existing receipt')
    local=Path(__file__).resolve().parent
    control=local/'cookie_flow_gate.cc'
    report={'pin':PIN,'scope':__doc__,'passed':False,'full_gate_passed':False,'native_sanitizers':True,
            'rust_instrumented':args.rust_sanitize,'precompiled_std_instrumented':False,'minimum_free_bytes':4*1024**3,
            'unrun_profiles':['Original complete headers corpus','Fault/exception/reentry/mutation/alias/extreme inputs','PHP/server/platform/allocation/lifetime/performance/default activation']}
    with tempfile.TemporaryDirectory(prefix='swr-cookie-flow-') as temp:
        tree=Path(temp);path='src/protocol/http.cc'
        def guard():
            free=shutil.disk_usage(tree).free
            if free<report['minimum_free_bytes']:
                report['blocked']={'free_bytes':free};write_report(args.output,report);return False
            return True
        if not guard():return 1
        archive=prepare_archive(args,tree,report)
        if not archive:write_report(args.output,report);return 1
        sides=Sides(args.source,args.archive,tree,'thirdparty/multipart_parser.h',path)
        if not guard():return 1
        compiled=sides.native(report,path)
        write_report(args.output,report)
        if not compiled:return 1
        original=sides.original(path);selected=sides.selected(path)
        units=[]
        for side,namespace,text in [('upstream','upstream_headers',original),('candidate','candidate_headers',selected)]:
            parts=body(text,'void parse_cookie(')
            unit=tree/(namespace+'.cc');unit.write_text('#include "headers_oracle.h"\nnamespace '+namespace+' {\n'+parts+'\n}\n');units.append((side,unit))
        if not guard():return 1
        binary=tree/'gate'
        built=sides.build(binary,[*units,('candidate',control)],archive,report)
        write_report(args.output,report)
        if not built:return 1
        report['actual_rust_symbol_present']='swr_v2_http_cookie_flow' in subprocess.check_output(['nm','-g',str(binary)],text=True)
        if not report['actual_rust_symbol_present']:write_report(args.output,report);return 1
        result=run_isolated([str(binary)],timeout=30)
        report['run']={'returncode':result.returncode,'stdout':result.stdout,'stderr':result.stderr}
        report['passed']=result.returncode==0
        if report['passed']:report['outcome']=json.loads(result.stdout)
        write_report(args.output,report);print(json.dumps({'passed':report['passed'],'outcome':report.get('outcome')}))
        return 0 if report['passed'] else 1
if __name__=='__main__':sys.exit(main())
