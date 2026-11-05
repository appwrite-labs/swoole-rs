#!/usr/bin/env python3
"""Generate the native HTTP wire views from their canonical Rust records."""
import argparse
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'crates/phphttp/src/state.rs').read_text()
primitive = {'c_int': 'int', 'u32': 'uint32_t', 'u8': 'uint8_t', 'i8': 'int8_t',
             'i64': 'SessionId', 'usize': 'size_t', 'zval': 'zval',
             '*mut c_char': 'char *', '*const c_char': 'const char *',
             '*mut c_void': 'void *', 'Request': 'Request', 'Response': 'Response',
             'Llhttp': 'llhttp_t', '*mut crate::parse::Multipart': 'multipart_parser *'}
renames = {'h2_data': 'h2_data_', 'send_header': 'send_header_', 'end': 'end_',
           'send_trailer': 'send_trailer_', 'write_buffer': 'write_buffer_',
           'compressed': 'compressed_', 'upload_tmp_dir': 'upload_tmp_dir_',
           'compression_types': 'compression_types_'}
text = '''// Generated from crates/phphttp/src/state.rs; native borrowed views only.
#ifndef SWR_PHPHTTP_NATIVE_RECORD_CONTEXT
#include "php_swoole_cxx.h"
#include "php_swoole_http.h"
#else
#ifndef SWOOLE_RS_HTTP_RECORDS_H
#define SWOOLE_RS_HTTP_RECORDS_H
'''
for rust, native in [('Request', 'Request'), ('Response', 'Response'), ('State', 'ContextState')]:
    body = source.split('struct ' + rust + ' {', 1)[1].split('\n}', 1)[0]
    fields = re.findall(r'^    (?:pub\(crate\) )?(\w+): ([^,]+),', body, re.M)
    text += 'struct ' + native + ' {\n'
    for name, kind in fields:
        if kind == 'Slot':
            text += f'    zval *{name};\n    zval _{name};\n'
        elif kind == 'Parse':
            text += '    alignas(8) unsigned char parse_[SWR_PHPHTTP_PARSE_SIZE];\n'
        else:
            wire = 'void *' if kind.startswith('Option<') else primitive[kind]
            if rust == 'Response' and name == 'method':
                wire = 'enum llhttp_method'
            text += '    ' + wire + ' ' + renames.get(name, name) + ';\n'
    text += '};\n'
text += '#endif\n#endif\n'
output = ROOT / 'native/httpbindings/swoole_rs_http_records.h'
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--check', action='store_true')
a = p.parse_args()
if a.check:
    if not output.is_file() or output.read_text() != text:
        raise SystemExit(f'{output} is stale; run python3 tools/http_records.py')
else:
    output.write_text(text)
