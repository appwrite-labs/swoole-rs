#!/usr/bin/env python3
"""Differential actual CONNECT serialization and handshake bodies; native allocation remains outside Rust."""
import argparse
from pathlib import Path
import shutil
import sys
import tempfile
from gate import PIN, body, add_archive_arguments, prepare_archive, Sides, run
from tools.harness import write_report

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    add_archive_arguments(parser)
    parser.add_argument('--output', default='target/reports/http-proxy.json')
    args = parser.parse_args(); local = Path(__file__).resolve().parent
    if Path(args.output).exists(): parser.error('Refusing an existing receipt')
    report = {'pin': PIN, 'scope': 'Actual pinned/patched proxy methods with native String/error facades; not full proxy socket/remote-service parity', 'native_sanitizers': True, 'rust_instrumented': args.rust_sanitize, 'rust_std_instrumented': False, 'passed': False, 'minimum_free_bytes': 4*1024**3}
    with tempfile.TemporaryDirectory(prefix='swr-http-proxy-') as temp:
        tree = Path(temp); path = 'src/protocol/http.cc'
        def guard():
            free = shutil.disk_usage(tree).free
            if free < report['minimum_free_bytes']:
                report['blocked'] = {'free_bytes': free}; write_report(args.output, report); return False
            return True
        if not guard(): return 1
        archive = prepare_archive(args, tree, report)
        if not archive: write_report(args.output, report); return 1
        sides = Sides(args.source, args.archive, tree, 'thirdparty/multipart_parser.h', path, 'src/core/base.cc', 'src/core/base64.cc')
        base = sides.original('src/core/base.cc'); codec = sides.original('src/core/base64.cc')
        globals_source = '#include "proxy_oracle.h"\n' + body(base, 'size_t sw_snprintf(')
        table = codec[codec.index('static char base64en[]'):codec.index('#define BASE64DE_FIRST')]
        globals_source += '\n' + table + '\n' + body(codec, 'size_t base64_encode(')
        dependencies = tree/'dependencies.cc'; dependencies.write_text(globals_source)
        if not guard(): return 1
        if not sides.native(report, path): write_report(args.output, report); return 1
        signatures = ['std::string HttpProxy::get_auth_str(', 'size_t HttpProxy::pack(', 'bool HttpProxy::handshake(']
        for namespace, text in [('upstream_proxy', sides.original(path)), ('candidate_proxy', sides.selected(path))]:
            parts = [body(text, signature) for signature in signatures]
            extracted = tree/(namespace+'.cc'); extracted.write_text('#include "proxy_oracle.h"\nnamespace '+namespace+' {\n'+'\n'.join(parts)+'\n}\n')
        binary = tree/'gate'
        units = [('upstream', tree/'upstream_proxy.cc'), ('candidate', tree/'candidate_proxy.cc'), ('candidate', local/'proxy_gate.cc'), ('upstream', dependencies)]
        if not guard(): return 1
        if not sides.build(binary, units, archive, report): write_report(args.output, report); return 1
        return run(binary, ['swr_v1_http_proxy_owned_pack', 'swr_v1_http_proxy_owned_authorization', 'swr_v1_core_proxy_new'], args.output, report)
if __name__ == '__main__': sys.exit(main())
