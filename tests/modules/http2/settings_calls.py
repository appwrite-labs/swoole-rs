#!/usr/bin/env python3
"""Actual per-item server loop and client SETTINGS lambda with HPACK/trace doubles."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gate
import sides


def select_overlay(text):
    # Resolve only our feature guards; preserve unrelated original directives.
    output, stack, active = [], [], True
    for line in text.splitlines(True):
        stripped = line.strip()
        if stripped.startswith(('#if ', '#ifdef ', '#ifndef ')):
            owned = stripped == '#ifdef SW_USE_RUST_HTTP2'
            stack.append((owned, active))
            if not owned and active:
                output.append(line)
        elif stripped == '#else' and stack:
            owned, parent = stack[-1]
            if owned:
                active = parent and not active
            elif active:
                output.append(line)
        elif stripped == '#endif' and stack:
            owned, parent = stack.pop()
            if not owned and active:
                output.append(line)
            active = parent
        elif active:
            output.append(line)
    if stack:
        raise ValueError('Unclosed preprocessor guard')
    return ''.join(output)


def body(text, marker):
    start = text.index(marker)
    opening = text.index('{', start)
    depth, pos = 1, opening + 1
    while depth:
        depth += (text[pos] == '{') - (text[pos] == '}')
        pos += 1
    return text[opening + 1:pos - 1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--core-report', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--include', action='append', default=[])
    args = parser.parse_args()
    core = json.loads(args.core_report.read_text())
    if not core['passed']:
        raise ValueError('A completed passing core receipt is required')
    artifacts = args.core_report.parent / (args.core_report.stem + '-artifacts')
    overlay = artifacts / 'overlay'
    archive = Path(core.get('archive', artifacts / 'libhttp2.a'))
    if gate.digest(archive) != core['archive_sha256']:
        raise ValueError('Archive receipt mismatch')
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'not_covered': ['actual PHP runtime', 'HPACK implementation', 'full server/client production TUs', 'performance']}
    pieces = []
    for role, filename, marker in [
        ('server', 'ext-src/swoole_http2_server.cc', 'while (length > 0)'),
        ('client', 'ext-src/swoole_http2_client_coro.cc', '[&](uint16_t id, uint32_t value) -> ReturnCode')]:
        original = subprocess.check_output(['git', 'show', f'{gate.PIN}:{filename}'], cwd=args.source, text=True)
        selected = select_overlay((overlay / filename).read_text())
        if gate.digest(overlay / filename) != core['patched_sources'][filename]:
            raise ValueError('Overlay receipt mismatch')
        for variant, text in [('native', original), ('rust', selected)]:
            fragment = body(text, marker)
            # Isolate the exact source block, then let the compiler select the overlay guard.
            if role == 'server':
                method = 'int apply(const char *buf, ssize_t length) { uint16_t id=0;uint32_t value=0;auto client=this;while(length>0){' + fragment + '}return SW_SUCCESS;}'
            else:
                method = 'ReturnCode apply(uint16_t id,uint32_t value){' + fragment + '}'
            pieces.append((variant, ('\n#define SW_USE_RUST_HTTP2\n' if variant == 'rust' else '\n#undef SW_USE_RUST_HTTP2\n') +
                          'namespace ' + role + '_' + variant + ' { struct State: Shared {' + method + '};}\n'))
    prelude = r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <vector>
#include <string>
#include <iostream>
#include <cstdarg>
#include <stdexcept>
using namespace swoole;
namespace Http2=swoole::http2;
inline std::vector<std::string> effects;
inline int hp_result;
inline bool fail_throw;
inline void emit(const char *fmt,...) {char b[512];va_list args;va_start(args,fmt);vsnprintf(b,sizeof(b),fmt,args);va_end(args);effects.push_back(b);}
#undef swoole_http2_frame_trace_log
#define swoole_http2_frame_trace_log(...) emit(__VA_ARGS__)
#undef swoole_trace_log
#define swoole_trace_log(flag,...) emit(__VA_ARGS__)
#undef swoole_warning
#define swoole_warning(...) emit(__VA_ARGS__)
inline const char* nghttp2_strerror(int) {return "controlled-error";}
inline int nghttp2_hd_deflate_change_table_size(void *p,uint32_t v) {emit("hpack:%d:%u",p!=nullptr,v);if(fail_throw)throw std::runtime_error("native effect");return hp_result;}
inline void nghttp2_error(int rc,const char *message){emit("client-error:%d:%s",rc,message);}
struct Shared { Http2::Settings remote_settings{};uint32_t remote_window_size=91;void *deflater=nullptr;};
'''
    driver = r'''
int main(){size_t cases=0;uint32_t seed=0x8e8c4991;auto random=[&](){seed^=seed<<13;seed^=seed>>17;seed^=seed<<5;return seed;};
for(unsigned iteration=0;iteration<1000;++iteration)for(unsigned id:{0,1,2,3,4,5,6,7,65535})for(bool same:{false,true})for(bool table:{false,true})for(int error:{0,-1})for(bool throws:{false,true}){
 uint32_t value=random();Shared initial;initial.remote_settings={value,value,value,value,value,value};initial.remote_window_size=random();initial.deflater=table?(void*)1:nullptr;
 if(!same)value^=0x80000000;hp_result=error;fail_throw=throws;
 for(bool server:{false,true}){
  std::vector<std::string> expected;Shared expected_state;int expected_rc=0;std::string expected_exception;
  for(bool rust:{false,true}){
   effects.clear();Shared state;int rc=0;std::string exception;
   unsigned char bytes[6]={uint8_t(id>>8),uint8_t(id),uint8_t(value>>24),uint8_t(value>>16),uint8_t(value>>8),uint8_t(value)};
   if(server){
    if(rust){server_rust::State s;static_cast<Shared&>(s)=initial;try{rc=s.apply((char*)bytes,6);}catch(const std::exception&e){exception=e.what();}state=s;}
    else{server_native::State s;static_cast<Shared&>(s)=initial;try{rc=s.apply((char*)bytes,6);}catch(const std::exception&e){exception=e.what();}state=s;}
   }else{
    if(rust){client_rust::State s;static_cast<Shared&>(s)=initial;try{rc=s.apply(id,value);}catch(const std::exception&e){exception=e.what();}state=s;}
    else{client_native::State s;static_cast<Shared&>(s)=initial;try{rc=s.apply(id,value);}catch(const std::exception&e){exception=e.what();}state=s;}
   }
   if(!rust){expected=effects;expected_state=state;expected_rc=rc;expected_exception=exception;}
   else if(expected!=effects||memcmp(&expected_state.remote_settings,&state.remote_settings,sizeof(state.remote_settings))||expected_state.remote_window_size!=state.remote_window_size||expected_rc!=rc||expected_exception!=exception){std::cerr<<"mismatch role="<<server<<" id="<<id<<" same="<<same<<" table="<<table<<" error="<<error<<" throws="<<throws<<"\n";return 1;}
  }++cases;
 }
}
std::cout<<"{\"passed\":true,\"settings_cases\":"<<cases<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'settings', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
