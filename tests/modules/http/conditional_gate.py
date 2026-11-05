#!/usr/bin/env python3
"""Conditional-date comparison using actual libc calendar calls and extracted native callers."""
import argparse
from pathlib import Path
import sys
import tempfile
from gate import PIN, body, Sides, run
from tools.harness import write_report

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', default='target/reports/http-conditionals.json')
    args = parser.parse_args(); local = Path(__file__).resolve().parent
    report = {'pin': PIN, 'scope': 'Actual pinned/patched conditional callers with libc calendar calls; defined inputs/errno/call order, not PHP/server parity', 'native_sanitizers': True, 'rust_instrumented': False}
    with tempfile.TemporaryDirectory(prefix='swr-http-conditional-') as temp:
        tree = Path(temp); path = 'src/server/static_handler.cc'
        sides = Sides(args.source, args.archive, tree, path)
        if not sides.native(report, path): write_report(args.output, report); return 1
        for namespace, text in [('upstream_conditional', sides.original(path)), ('candidate_conditional', sides.selected(path))]:
            extracted = tree/(namespace+'.cc')
            # Keep the actual generic declaration used by legacy and V2 callers.
            prefix = 'template<typename Handler>\n' if 'template<typename Handler>\nstatic bool conditional_file_date(' in text else ''
            parts = ([prefix + body(text, 'static bool conditional_file_date(')] if namespace == 'candidate_conditional' and 'static bool conditional_file_date(' in text else []) + [body(text, 'bool StaticHandler::is_modified('), body(text, 'bool StaticHandler::is_modified_range(')]
            # The selected handler names its server serv_; the facade's is serv.
            extracted.write_text('#include "conditional_oracle.h"\nnamespace '+namespace+' {\n'+'\n'.join(parts).replace('serv_,','serv,').replace('serv_ ?', 'serv ?').replace('&serv_->', '&serv->').replace('mtime_slot_', 'reinterpret_cast<const int64_t *>(&file_mtime)')+'\n}\n')
        binary = tree/'gate'
        units = [('upstream', tree/'upstream_conditional.cc'), ('candidate', tree/'candidate_conditional.cc'), ('candidate', local/'conditional_gate.cc')]
        if not sides.build(binary, units, args.archive.resolve(), report): write_report(args.output, report); return 1
        return run(binary, ['swr_v1_http_conditional'], args.output, report)
if __name__ == '__main__': sys.exit(main())
