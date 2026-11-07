#!/usr/bin/env python3
"""Actual HTTP proxy creation boundary checks. The transport routes are Rust's
(crates/ports/src/http_routes.rs), tested there."""
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
    parser.add_argument('--output', default='target/reports/http-dispatch.json')
    args = parser.parse_args()
    local = Path(__file__).resolve().parent
    report = {'pin': PIN, 'scope': 'Pinned HTTP factory/auth controls and actual Rust-owned candidate factory/auth; native allocation fault traces and supported candidate C++ faults. No physical allocation-layout or complete Rust OOM parity claim', 'native_sanitizers': True, 'rust_instrumented': args.rust_sanitize, 'rust_std_instrumented': False, 'passed': False,
              'allocation_contract': {
                  'physical_old_new_cpp_allocation_equality': 'not_applicable: ownership and native class layout changed',
                  'native_control_faults': 'Every observed C++ new position; partial fields, event-size prefix and explicit fixture cleanup',
                  'candidate_cpp_faults': 'Every observed factory, authorization return-string and cached-field update allocation; actual production class, independent custody and rollback',
                  'candidate_rust_oom': 'not_run: Rust System/Box/Vec failure injection is not supplied by the C++ new oracle',
                  'full_allocation_parity_passed': False,
              }}
    with tempfile.TemporaryDirectory(prefix='swr-http-dispatch-') as temp:
        tree = Path(temp)
        archive = prepare_archive(args, tree, report)
        if not archive: write_report(args.output, report); return 1
        paths = ['src/protocol/http.cc', 'src/server/static_handler.cc']
        dependencies_paths = ['src/core/base.cc', 'src/core/base64.cc']
        sides = Sides(args.source, args.archive, tree, 'thirdparty/multipart_parser.h', *paths, *dependencies_paths)
        if not sides.native(report, paths[0]): write_report(args.output, report); return 1
        proxy_signatures = ['HttpProxy *HttpProxy::create(', 'std::string HttpProxy::get_auth_str(']
        for namespace, text in [('upstream_dispatch', sides.original(paths[0])), ('candidate_dispatch', sides.selected(paths[0]))]:
            proxy = '\n'.join(body(text, signature) for signature in proxy_signatures)
            # Candidate definitions bind to the real production class and its Rust
            # record, never another three-std::string fixture layout.
            proxy_namespace = 'swoole' if namespace == 'candidate_dispatch' else namespace
            extracted = tree / (namespace + '.cc')
            extracted.write_text('#include "dispatch_oracle.h"\nnamespace ' + proxy_namespace + ' {\n' + proxy + '\n}\n')
        base = sides.original(dependencies_paths[0])
        codec = sides.original(dependencies_paths[1])
        table = codec[codec.index('static char base64en[]'):codec.index('#define BASE64DE_FIRST')]
        dependencies = tree / 'dependencies.cc'
        dependencies.write_text('#include "dispatch_oracle.h"\n' + body(base, 'size_t sw_snprintf(') +
                                '\n' + table + '\n' + body(codec, 'size_t base64_encode('))
        binary = tree/'gate'
        units = [('upstream', tree/'upstream_dispatch.cc'), ('candidate', tree/'candidate_dispatch.cc'), ('candidate', local/'dispatch_gate.cc'), ('upstream', dependencies)]
        if not sides.build(binary, units, archive, report): write_report(args.output, report); return 1
        return run(binary, ['swr_v1_http_proxy_auth_enabled', 'swr_v1_http_proxy_owned_authorization'], args.output, report)
if __name__ == '__main__': sys.exit(main())
