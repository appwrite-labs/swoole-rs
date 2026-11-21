#!/usr/bin/env python3
"""url_rewrite_rules: the pinned add_rewrite_rule/apply_rewrite_rules (std::regex) against the
server's Rust-owned rules rewriting the Rust static context."""
import argparse
from pathlib import Path
import sys
import tempfile
from gate import PIN, body, add_archive_arguments, prepare_archive, Sides, run
from tools.harness import write_report

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    add_archive_arguments(parser)
    parser.add_argument('--output', default='target/reports/http-rewrites.json')
    args = parser.parse_args(); local = Path(__file__).resolve().parent
    report = {'pin': PIN, 'scope': 'Pinned add_rewrite_rule/apply_rewrite_rules against swr_v1_server_static_rewrite; not socket/PHP parity', 'native_sanitizers': True, 'rust_instrumented': args.rust_sanitize, 'rust_std_instrumented': False, 'passed': False}
    with tempfile.TemporaryDirectory(prefix='swr-http-rewrite-') as temp:
        tree = Path(temp); path = 'src/server/static_handler.cc'
        archive = prepare_archive(args, tree, report)
        if not archive: write_report(args.output, report); return 1
        sides = Sides(args.source, args.archive, tree, path)
        text = sides.original(path); extracted = tree/'upstream_rewrite.cc'
        parts = [body(text, 'void Server::add_rewrite_rule('), body(text, 'bool Server::apply_rewrite_rules(')]
        extracted.write_text('#include "rewrite_oracle.h"\nnamespace upstream_rewrite {\nusing swoole::starts_with;\n'+'\n'.join(parts)+'\n}\n')
        binary = tree/'gate'
        if not sides.build(binary, [('upstream', extracted), ('candidate', local/'rewrite_gate.cc')], archive, report, gc_sections=True): write_report(args.output, report); return 1
        return run(binary, ['swr_v1_server_static_rewrite', 'swr_v1_server_static_rewrite_add'], args.output, report)
if __name__ == '__main__': sys.exit(main())
