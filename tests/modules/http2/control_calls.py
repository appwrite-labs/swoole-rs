#!/usr/bin/env python3
"""Actual WINDOW_UPDATE/RST/GOAWAY bodies with allocation/send/reentry doubles."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
import gate
import sides
from settings_calls import body, select_overlay


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--core-report', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--include', action='append', default=[])
    args = parser.parse_args()
    core = json.loads(args.core_report.read_text())
    if not core['passed']:
        raise ValueError('Completed core receipt required')
    artifacts = args.core_report.parent / (args.core_report.stem + '-artifacts')
    archive = Path(core.get('archive', artifacts / 'libhttp2.a'))
    if gate.digest(archive) != core['archive_sha256']:
        raise ValueError('Archive receipt mismatch')
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'commands': [],
              'not_covered': ['PHP runtime and actual IO', 'uninitialized native PING payload bytes', 'performance']}
    methods = {
        'server': [
            ('static void http2_server_send_window_update(', 'void window(HttpContext *ctx, uint32_t stream_id, uint32_t size)'),
            ('static bool http2_server_send_rst_stream(', 'bool reset(HttpContext *ctx, int error_code)'),
            ('bool swoole_http2_server_goaway(', 'bool goaway(HttpContext *ctx, zend_long error_code, zend_string *sdata)')],
        'client': [
            ('bool Client::send_window_update(', 'bool window(int _stream_id, uint32_t size)'),
            ('bool Client::send_goaway_frame(', 'bool goaway(zend_long error_code, const char *debug_data, size_t debug_data_len)')]}
    pieces=[]
    for role, filename in [('server','ext-src/swoole_http2_server.cc'),('client','ext-src/swoole_http2_client_coro.cc')]:
        original=subprocess.check_output(['git','show',f'{gate.PIN}:{filename}'],cwd=args.source,text=True)
        selected=artifacts/'overlay'/filename
        if gate.digest(selected)!=core['patched_sources'][filename]:
            raise ValueError('Overlay identity mismatch')
        for variant,text in [('native',original),('rust',select_overlay(selected.read_text()))]:
            code=[]
            for marker, signature in methods[role]:
                # Definitions may be preceded by declarations: match the exact definition signature.
                start=text.index(marker)
                if text.find(';',start)<text.find('{',start):
                    start=text.index(marker,start+len(marker))
                fragment=body(text[start:],marker)
                code.append(signature+'{'+fragment+'}')
            pieces.append((variant, 'namespace '+role+'_'+variant+' { struct State: '+('::Server' if role=='server' else '::Client')+' {'+''.join(code)+'};}\n'))
    prelude=r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <vector>
#include <array>
#include <memory>
#include <map>
#include <string>
#include <iostream>
#include <stdexcept>
#include <cstdarg>
using namespace swoole;
namespace Http2=swoole::http2;
namespace swoole {namespace http2 {inline const char *get_type(int type){if(type!=7)std::abort();return "GOAWAY";}}}
using zend_long=int64_t;
#define ZEND_LONG_FMT "%lld"
struct zend_string {std::string bytes;};
#define ZSTR_VAL(s) ((s)->bytes.data())
#define ZSTR_LEN(s) ((s)->bytes.size())
inline std::vector<std::string> effects;
inline std::vector<uint8_t> emitted;
inline bool send_result,send_throw,mutate;
inline void emit(const char *fmt,...){char b[512];va_list v;va_start(v,fmt);vsnprintf(b,sizeof(b),fmt,v);va_end(v);effects.push_back(b);}
#undef swoole_trace_log
#define swoole_trace_log(flag,...) emit(__VA_ARGS__)
#undef swoole_http2_send_trace_log
#define swoole_http2_send_trace_log(...) emit(__VA_ARGS__)
inline void *ecalloc(size_t n,size_t size){effects.push_back("allocate");return calloc(n,size);}
inline void efree(void *p){effects.push_back("free");free(p);}
struct Session {uint32_t last_stream_id=0;bool shutting_down=false;};
struct HttpContext {int fd=1;bool (*send)(HttpContext*,const char*,size_t);};
struct Server {std::map<int,std::shared_ptr<Session>> http2_sessions;};
inline Session *active_session;
inline bool send_record(const char*p,size_t n){effects.push_back("send");emitted.assign(p,p+n);if(mutate&&active_session){active_session->last_stream_id=91;active_session->shutting_down=false;}if(send_throw)throw std::runtime_error("native send");return send_result;}
inline bool context_send(HttpContext*,const char*p,size_t n){return send_record(p,n);}
struct Client {uint32_t stream_id=0,last_stream_id=0;bool send(const char*p,size_t n){return send_record(p,n);}};
'''
    driver=r'''
int main(){size_t count=0;uint32_t seed=0x8e8c4991;auto random=[&](){seed^=seed<<13;seed^=seed>>17;seed^=seed<<5;return seed;};
for(unsigned iteration=0;iteration<5000;++iteration){uint32_t id=random(),value=random();
 // Native aligned store is compared against Rust's arbitrary-byte output.
 alignas(8) std::array<uint8_t,32> a{},b{};uint8_t kind=random(),flags=random();uint32_t length=random();
 Http2::set_frame_header((char*)a.data()+3,kind,length,flags,id);
 sw_rs_http2_pack_frame_header(b.data()+3,kind,length,flags,id);if(a!=b)return 1;++count;
 uint32_t network=htonl(value);std::array<uint8_t,8> unaligned{};memcpy(unaligned.data()+1,&network,4);
 if(sw_rs_http2_read_u32(unaligned.data()+1)!=value)return 2;
 sw_rs_http2_write_u32(unaligned.data()+1,id);uint32_t got;memcpy(&got,unaligned.data()+1,4);if(ntohl(got)!=id)return 3;count+=2;
 for(bool server:{false,true})for(unsigned operation:{0,1,2})for(bool succeeds:{false,true})for(bool reenter:{false,true}){
  if(!server&&operation==1)continue;send_result=succeeds;send_throw=false;mutate=reenter;
  std::vector<std::string> expected;std::vector<uint8_t> expected_bytes;bool expected_result=false,expected_closed=false;uint32_t expected_last=0;
  for(bool rust:{false,true}){effects.clear();emitted.clear();bool result=true;auto session=std::make_shared<Session>();session->last_stream_id=id;active_session=session.get();
   std::string debug(iteration%17,char(value));if(debug.size()>3)debug[2]=0;zend_string zd{debug};HttpContext ctx{1,context_send};
   if(server){if(rust){server_rust::State s;s.http2_sessions[1]=session;if(operation==0)s.window(&ctx,id,value);else if(operation==1)result=s.reset(&ctx,value);else result=s.goaway(&ctx,value,iteration%2?&zd:nullptr);}
    else{server_native::State s;s.http2_sessions[1]=session;if(operation==0)s.window(&ctx,id,value);else if(operation==1)result=s.reset(&ctx,value);else result=s.goaway(&ctx,value,iteration%2?&zd:nullptr);}}
   else{if(rust){client_rust::State s;s.last_stream_id=id;s.stream_id=value;result=operation==0?s.window(id,value):s.goaway(value,debug.data(),debug.size());}
    else{client_native::State s;s.last_stream_id=id;s.stream_id=value;result=operation==0?s.window(id,value):s.goaway(value,debug.data(),debug.size());}}
   if(!rust){expected=effects;expected_bytes=emitted;expected_result=result;expected_closed=session->shutting_down;expected_last=session->last_stream_id;}
   else if(expected!=effects||expected_bytes!=emitted||expected_result!=result||expected_closed!=session->shutting_down||expected_last!=session->last_stream_id){std::cerr<<"mismatch server="<<server<<" operation="<<operation<<"\n";return 4;}
  }++count;
 }
}
std::cout<<"{\"passed\":true,\"control_cases\":"<<count<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'control', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    report['native_sanitizers']=['address','undefined except alignment'];report['rust_sanitized']=False
    args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'passed':report['passed'],'report':str(args.output)}))
    return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
