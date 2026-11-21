#!/usr/bin/env python3
"""Pinned native HTTP locale comparisons against actual Rust, without global host changes."""
import argparse
import json
from pathlib import Path
import sys
import tempfile
from gate import PIN, body, Sides
from tools.harness import run_isolated, write_report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',type=Path,required=True)
    parser.add_argument('--archive',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args();local=Path(__file__).resolve().parent
    report={'pin':PIN,'scope':'Actual pinned get_method/boundary bodies and location/disposition libc helpers; unavailable locales unsupported, not passed. No PHP/full activation or performance claim.',
            'rust_instrumented':False,'native_sanitizers':False,'sanitizer_note':'Unsanitized native libc required: Mac ASan interceptor folds ASCII instead of selected locale','passed':False}
    with tempfile.TemporaryDirectory(prefix='swr-http-locale-') as temporary:
        tree=Path(temporary)
        sides=Sides(args.source,args.archive,tree,'src/protocol/http.cc',sanitize=False)
        text=sides.original('src/protocol/http.cc')
        start=text.index('static const char *method_strings[]');end=text.index('};',start)+2
        extracted=tree/'native.cc'
        extracted.write_text('#include "swoole_http.h"\nnamespace upstream_locale {\n'+text[start:end]+'\n'+body(text,'int get_method(')+'\n'+body(text,'bool parse_multipart_boundary(')+'\nunsigned strcaseeq(const char *a,size_t n,const char *b,size_t m){return swoole_strcaseeq(a,n,b,m);}\nunsigned istarts_with(const char *a,size_t n,const char *b,size_t m){return swoole_str_istarts_with(a,n,b,m);}\n}\n')
        binary=tree/'gate'
        if not sides.build(binary,[('upstream',extracted),('candidate',local/'locale_gate.cc')],args.archive.resolve(),report,gc_sections=True):write_report(args.output,report);return 1
        result=run_isolated([str(binary)],timeout=60);report['run']={'returncode':result.returncode,'stdout':result.stdout,'stderr':result.stderr}
        if result.stdout:report['outcome']=json.loads(result.stdout.splitlines()[-1])
        report['passed']=result.returncode==0;write_report(args.output,report);print(result.stdout,result.stderr)
        return 0 if report['passed'] else 1


if __name__=='__main__':sys.exit(main())
