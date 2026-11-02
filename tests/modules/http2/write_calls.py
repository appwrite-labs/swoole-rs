#!/usr/bin/env python3
"""Actual client request and pipeline write bodies with native conversion, IO and stream-lifetime effects."""
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
        raise ValueError('Completed passing core receipt required')
    artifacts = args.core_report.parent / (args.core_report.stem + '-artifacts')
    archive = Path(core.get('archive', artifacts / 'libhttp2.a'))
    if gate.digest(archive) != core['archive_sha256']:
        raise ValueError('Archive changed')
    name = 'ext-src/swoole_http2_client_coro.cc'
    selected = artifacts / 'overlay' / name
    if gate.digest(selected) != core['patched_sources'][name]:
        raise ValueError('Overlay changed')
    if gate.digest(gate.ROOT / 'native/http2/rust_http2.h') != core['module_header_sha256']:
        raise ValueError('Module header changed')
    original = subprocess.check_output(['git', 'show', f'{gate.PIN}:{name}'], cwd=args.source, text=True)
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'commands': [], 'gaps': [
                  'Real PHP property/reference-count/GC semantics, sockets, HPACK and coroutine scheduling',
                  'Native Zend longjmp bailout; C++ exception prefixes are separate fixture outcomes',
                  'Callbacks freeing a still-used native raw stream are outside defined fixture lifetimes',
                  'Tracing is disabled; transport/conversion/allocation behavior is scripted by native doubles',
                  'Rust sanitizer instrumentation and performance']}
    pieces = []
    for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
        fragment = body(text, 'bool Client::write_data(')
        marker = '} else if (' + ('type == SW_HTTP2_TYPE_DATA' if variant == 'native' else 'frame_action == SW_RS_HTTP2_DATA') + ')'
        received = body(body(text, 'ReturnCode Client::parse_frame('), marker)
        received = received[:received.index('        if (length > 0)')]
        request = body(text, 'uint32_t Client::send_request(')
        pieces.append((variant, 'namespace ' + variant + ' {struct Client: FixtureClient {bool write_data(uint32_t _stream_id,zval *zdata,bool end){' + fragment + '}void receive_flags(uint32_t flags){auto stream=&value;' + received + '}uint32_t send_request(zval *zrequest){' + request + '}};}'))
    prelude = r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <iostream>
#include <stdexcept>
#include <exception>
#include <cstdio>
using namespace swoole;
namespace Http2 = swoole::http2;
inline std::vector<std::string> effects;
inline unsigned throw_at, mutation, query_failure, allocated;
struct zval {bool array=false;std::string bytes;bool scalar=false,truth=false;};
struct Stream {unsigned flags=0,stream_id=0;};
inline Stream *current;
inline void effect(std::string text,bool may_throw=true){effects.push_back(text);if(mutation&1)current->flags^=15;if(mutation&2)current->flags^=128;if(may_throw&&throw_at==effects.size())throw std::runtime_error("native effect");}
struct smart_str {char *s=nullptr;};
inline char *php_swoole_http_build_query(zval*z,size_t*n,smart_str*s){effect("query");if(query_failure)return nullptr;*n=z->bytes.size();s->s=z->bytes.data();++allocated;return s->s;}
inline void smart_str_free(smart_str*s){effect("free",false);if(s->s){s->s=nullptr;--allocated;}}
#define ZVAL_IS_ARRAY(z) ((z)->array)
#define E_WARNING 2
inline void php_swoole_error(int,const char*what){effect(what);}
namespace zend {class String {zval *value=nullptr;public:String(){effect("string:default");}String &operator=(zval*z){effect("string:assign");value=z;return *this;}explicit String(zval*z):value(z){effect("string:new");}~String(){effect("string:drop",false);}char *val(){effect("string:val");return value->bytes.data();}size_t len(){effect("string:len");return value->bytes.size();}};}
namespace std_string {inline std::string format(const char*pattern,unsigned id){char buf[128];std::snprintf(buf,sizeof(buf),pattern,id);return buf;}}
#undef swoole_trace_log
#define swoole_trace_log(...) ((void)0)
#undef swoole_http2_send_trace_log
#define swoole_http2_send_trace_log(...) ((void)0)
#define ZEND_STRL(x) x,sizeof(x)-1
#define SW_ZSTR_KNOWN(x) (x)
#define SW_ZEND_STR_DATA 1
#define SW_ZEND_STR_PIPELINE 2
#define SW_ZEND_STR_USE_PIPELINE_READ 3
#define Z_OBJCE_P(z) 0
#define SW_Z8_OBJ_P(z) (z)
#define IS_STRING 1
#define Z_TYPE_P(z) ((z)->array?2:((z)->scalar?3:IS_STRING))
#define Z_STRLEN_P(z) ((z)->bytes.size())
inline zval request_headers,request_data,request_pipeline,request_read;
inline int swoole_http2_request_ce;
inline bool zval_is_true(zval*z){effect("truth");return z->array?!z->bytes.empty():(z->scalar?z->truth:(!z->bytes.empty()&&z->bytes!="0"));}
inline zval *sw_zend_read_and_convert_property_array(int,zval*,const char*,size_t,int){effect("property:headers");return &request_headers;}
inline zval *sw_zend_read_property_ex(int,zval*,int property,int){effect("property:"+std::to_string(property));return property==1?&request_data:&request_pipeline;}
inline zval *zend_read_property_ex(int,zval*,int,bool,zval*){effect("property:read");return &request_read;}
inline void add_assoc_stringl_ex(zval*,const char*,size_t,char*,size_t){effect("add:content-type");}
struct ScratchBuffer {char str[1024];};
inline ScratchBuffer scratch;
#define sw_tg_buffer fixture_tg_buffer
inline ScratchBuffer *sw_tg_buffer(){effect("scratch");return &scratch;}
struct FixtureClient {Stream value;bool exists=true;unsigned stream_id=55,calls=0,fail_call=0;ssize_t header_result=5;bool created=false;zval object;zval*zobject=&object;
 ssize_t build_header(zval*,zval*,char*buffer){effect("build-header");if(header_result>0)std::memset(buffer,'H',header_result);return header_result;}
 Stream *create_stream(uint32_t id,uint8_t flags){effect("create:"+std::to_string(id)+":"+std::to_string(flags));value.flags=flags;value.stream_id=id;created=true;return &value;}
 bool send_data(uint32_t id,const char*data,size_t length,int flags){effect("data:"+std::to_string(id)+":"+std::to_string(flags));return send(data,length);}
 Stream *get_stream(unsigned id){effect("get:"+std::to_string(id));return exists?&value:nullptr;}
 void update_error_properties(int code,const char*text){effect("error:"+std::to_string(code)+":"+text);}
 bool send(const char*bytes,size_t length){effect("send:"+std::string(bytes,length));return ++calls!=fail_call;}
};
'''
    driver = r'''
struct Result {std::vector<std::string> events;bool returned=false;unsigned flags=0,calls=0,remaining=0;std::string exception;};
template<class C>Result execute(bool exists,unsigned flags,bool end,bool array,unsigned length,unsigned fail,unsigned query_fail,unsigned throw_index,unsigned change){
 effects.clear();throw_at=throw_index;mutation=change;query_failure=query_fail;allocated=0;C client;client.value.flags=flags;client.exists=exists;client.fail_call=fail;current=&client.value;zval data{array,std::string("a\0bcd",length)};Result result;
 try{result.returned=client.write_data(123,&data,end);}catch(const std::runtime_error&e){result.exception=e.what();}
 result.events=effects;result.flags=client.value.flags;result.calls=client.calls;result.remaining=allocated;return result;
}
struct RequestResult {std::vector<std::string> events;unsigned returned=0,flags=0,next=0,calls=0,remaining=0;bool created=false;std::string exception;};
template<class C>RequestResult request_execute(unsigned kind,unsigned length,bool pipeline,bool read,ssize_t header_result,unsigned id,unsigned fail,unsigned query_fail,unsigned throw_index,unsigned change){
 effects.clear();throw_at=throw_index;mutation=change;query_failure=query_fail;allocated=0;C client;current=&client.value;client.stream_id=id;client.fail_call=fail;client.header_result=header_result;
 request_data=zval{kind==1,std::string("a\0bcd",length),kind==2,length!=0};request_pipeline=zval{false,"",true,pipeline};request_read=zval{false,"",true,read};zval request;RequestResult result;
 try{result.returned=client.send_request(&request);}catch(const std::runtime_error&e){result.exception=e.what();}
 result.events=effects;result.flags=client.value.flags;result.next=client.stream_id;result.calls=client.calls;result.remaining=allocated;result.created=client.created;return result;
}
int main(){size_t count=0,requests=0;
for(unsigned kind:{0u,1u,2u})for(unsigned length:{0u,1u,5u})for(bool pipeline:{false,true})for(bool read:{false,true})for(ssize_t header:{ssize_t(-1),ssize_t(0),ssize_t(5)})for(unsigned id:{1u,0x7fffffffu,UINT32_MAX})for(unsigned fail:{0u,1u,2u})for(unsigned query_fail:{0u,1u})for(unsigned throw_index=0;throw_index<18;++throw_index)for(unsigned change=0;change<4;++change){
 auto a=request_execute<native::Client>(kind,length,pipeline,read,header,id,fail,query_fail,throw_index,change);auto b=request_execute<rust::Client>(kind,length,pipeline,read,header,id,fail,query_fail,throw_index,change);
 if(a.events!=b.events||a.returned!=b.returned||a.flags!=b.flags||a.next!=b.next||a.calls!=b.calls||a.remaining!=b.remaining||a.created!=b.created||a.exception!=b.exception){std::cerr<<"request mismatch kind="<<kind<<" length="<<length<<" pipeline="<<pipeline<<" read="<<read<<" header="<<header<<" id="<<id<<" fail="<<fail<<" query_fail="<<query_fail<<" throw="<<throw_index<<" mutation="<<change<<"\n";return 4;}++requests;
}
for(bool exists:{false,true})for(unsigned flags=0;flags<16;++flags)for(bool end:{false,true})for(bool array:{false,true})for(unsigned length:{0u,1u,5u})for(unsigned fail:{0u,1u,2u})for(unsigned query_fail:{0u,1u})for(unsigned throw_index=0;throw_index<13;++throw_index)for(unsigned change=0;change<4;++change){
 auto a=execute<native::Client>(exists,flags,end,array,length,fail,query_fail,throw_index,change);auto b=execute<rust::Client>(exists,flags,end,array,length,fail,query_fail,throw_index,change);
 if(a.events!=b.events||a.returned!=b.returned||a.flags!=b.flags||a.calls!=b.calls||a.remaining!=b.remaining||a.exception!=b.exception){std::cerr<<"write mismatch exists="<<exists<<" flags="<<flags<<" end="<<end<<" array="<<array<<" length="<<length<<" fail="<<fail<<" query_fail="<<query_fail<<" throw="<<throw_index<<" mutation="<<change<<"\n";return 1;}++count;
}
// Receiving and sending update the same sole native flags only at their original publication points.
for(unsigned flags:{0u,1u,2u,4u,8u,15u,255u,UINT32_MAX})for(unsigned end:{0u,1u,255u})for(unsigned receive:{0u,1u,255u}){
 unsigned expected=flags;if(receive){if(!end)expected|=SW_HTTP2_STREAM_PIPELINE_RESPONSE;}else if(end)expected|=SW_HTTP2_STREAM_REQUEST_END;
 if(sw_rs_http2_stream_update(flags,end,receive)!=expected)return 2;
 if(receive){native::Client a;rust::Client b;a.value.flags=b.value.flags=flags;a.receive_flags(end);b.receive_flags(end);if(a.value.flags!=b.value.flags||a.value.flags!=expected)return 3;}
 ++count;
}
std::cout<<"{\"passed\":true,\"write_cases\":"<<count<<",\"request_cases\":"<<requests<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'write', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
