#!/usr/bin/env python3
"""Complete pinned/patched server DATA case with native shared ownership and callback traces."""
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
    header = gate.ROOT / 'native/http2/rust_http2.h'
    name = 'ext-src/swoole_http2_server.cc'
    selected = artifacts / 'overlay' / name
    if gate.digest(archive) != core['archive_sha256'] or gate.digest(selected) != core['patched_sources'][name]:
        raise ValueError('Frozen archive/overlay identity changed')
    if gate.digest(header) != core['module_header_sha256']:
        raise ValueError('Module header changed')
    original = subprocess.check_output(['git', 'show', f'{gate.PIN}:{name}'], cwd=args.source, text=True)
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'commands': [], 'gaps': [
                  'Whole frame parser/header routing has separate gates; this executes the complete DATA case',
                  'Real PHP/form/multipart/String/Socket/coroutine/GC and Zend bailout runtime',
                  'Effects use native doubles; actual shared_ptr lifetime/last-owner destruction is exercised',
                  'Callbacks freeing a still-used raw Context/body, Rust sanitizer instrumentation and performance']}
    pieces = []
    for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
        marker = 'case ' + ('SW_HTTP2_TYPE_DATA' if variant == 'native' else 'SW_RS_HTTP2_DATA') + ': {'
        fragment = body(text, marker)
        pieces.append((variant, 'namespace ' + variant + ' {int receive(const std::shared_ptr<Session>&client,int flags,uint32_t stream_id,ssize_t length,const char*buf){switch(0){case 0:{' + fragment + '}}return SW_OK;}}'))
    prelude = r'''
#include "swoole_http2.h"
#include "swoole_error.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <array>
#include <map>
#include <memory>
#include <functional>
#include <iostream>
#include <stdexcept>
#include <cstring>
namespace Http2=swoole::http2;
#define ZEND_STRL(x) x,sizeof(x)-1
#define PARSE_STRING 1
#undef swoole_http2_frame_trace_log
#define swoole_http2_frame_trace_log(...) ((void)0)
#undef swoole_error_log
#define swoole_error_log(level,code,format,id) event("warning:"+std::to_string(code)+":"+std::to_string(id))
inline std::vector<std::string>effects;
inline unsigned throw_at;
inline bool multipart_failure;
inline std::function<void(const std::string&)>mutate;
inline void event(const std::string&what,bool throwing=true){effects.push_back(what);if(mutate)mutate(what);if(throwing&&throw_at==effects.size())throw std::runtime_error("native effect");}
struct String{std::string bytes;size_t length=0;char*str=nullptr;void set(const std::string&s){bytes=s;length=s.size();str=bytes.data();}void append(const char*data,size_t n){event("append:"+std::to_string(n));bytes.append(data,n);length=bytes.size();str=bytes.data();}};
struct zval{};
struct Session;
inline int parser_marker;
struct HttpContext{unsigned end_=0,parse_body=0;void*mt_parser=nullptr;Session*client;String first,second;String*active=nullptr;unsigned gets=0;struct Request{unsigned post_form_urlencoded=0;zval object;zval*zobject=&object,*zpost=nullptr;}request;
 String*get_http2_data_buffer(){event("buffer:"+std::to_string(++gets));if(!active){event("buffer:new");active=&first;}return active;}
 size_t get_http2_data_length(){event("length");return active?active->length:0;}
 bool parse_multipart_data(const char*data,size_t n){event("multipart:"+std::string(data,n));return !multipart_failure;}
};
inline unsigned final_stream_window;
struct Stream{HttpContext*ctx;uint32_t local_window_size=0;~Stream(){final_stream_window=local_window_size;event("stream:drop",false);}};
struct Session{std::shared_ptr<Stream>slot;uint32_t max_body_size=0,local_window_size=0;Http2::Settings local_settings{};
 std::shared_ptr<Stream>get_stream(unsigned id){event("get:"+std::to_string(id));return slot;}
 void remove_stream(unsigned id){event("remove:"+std::to_string(id));slot.reset();}
 void handle(const std::shared_ptr<Session>&,const std::shared_ptr<Stream>&){event("handle");}
};
inline void http2_server_send_rst_stream(HttpContext*,unsigned code){event("reset:"+std::to_string(code));}
inline void http2_server_send_window_update(HttpContext*,unsigned id,unsigned increment){event("window:"+std::to_string(id)+":"+std::to_string(increment));}
inline int swoole_http_request_ce;
inline zval post;
inline zval*swoole_http_init_and_read_property(int,zval*,zval**slot,const char*,size_t){event("post");*slot=&post;return *slot;}
inline std::map<char*,size_t>allocations;
inline char*estrndup(const char*bytes,size_t n){event("duplicate:"+std::to_string(n));char*p=(char*)std::malloc(n+1);if(!p)throw std::bad_alloc();std::memcpy(p,bytes,n);p[n]=0;allocations[p]=n;return p;}
inline void treat_data(int,char*bytes,zval*){event("treat:"+std::string(bytes));allocations.erase(bytes);std::free(bytes);}
struct SapiModule{void(*treat_data)(int,char*,zval*);};
inline SapiModule sapi_module{treat_data};
'''
    driver = r'''
struct Result{std::vector<std::string>events;std::string first,second,exception;int returned=999;unsigned ended=0,parse=0,form=0,window=0,initial=0,stream_window=0,gets=0,active=0;bool multipart=false,live=false;size_t remnants=0;};
template<bool rust>Result execute(bool exists,unsigned length,unsigned buffered,unsigned maximum,bool ended,bool end,unsigned form_mode,std::array<unsigned,3>windows,unsigned mutation,bool failure,unsigned thrown){
 effects.clear();throw_at=thrown;multipart_failure=failure;final_stream_window=777;auto client=std::make_shared<Session>();client->max_body_size=maximum;client->local_window_size=windows[0];client->local_settings.init_window_size=windows[2];HttpContext ctx;ctx.client=client.get();ctx.end_=ended;ctx.parse_body=form_mode&1;ctx.request.post_form_urlencoded=(form_mode>>1)&1;ctx.mt_parser=(form_mode&4)?&parser_marker:nullptr;ctx.first.set(std::string(buffered,'p'));ctx.second.set("other");ctx.active=buffered?&ctx.first:nullptr;
 std::weak_ptr<Stream>weak;if(exists){client->slot=std::make_shared<Stream>();client->slot->ctx=&ctx;client->slot->local_window_size=windows[1];weak=client->slot;}
 mutate=[&](const std::string&event){
 if((mutation&1)&&event=="buffer:1"){ctx.end_=!ctx.end_;client->max_body_size=8;}
 if((mutation&2)&&event.rfind("append:",0)==0){client->local_window_size=0;client->local_settings.init_window_size=19;if(auto stream=weak.lock())stream->local_window_size=0;}
 if((mutation&4)&&event.rfind("window:0:",0)==0){client->local_settings.init_window_size=13;if(auto stream=weak.lock())stream->local_window_size=0;ctx.parse_body=!ctx.parse_body;}
 if((mutation&8)&&event=="length"){ctx.parse_body=!ctx.parse_body;ctx.request.post_form_urlencoded=!ctx.request.post_form_urlencoded;ctx.mt_parser=ctx.mt_parser?nullptr:&parser_marker;ctx.active=&ctx.second;}
 if((mutation&16)&&event=="post"){ctx.parse_body=0;ctx.request.post_form_urlencoded=0;ctx.mt_parser=&parser_marker;}
 if((mutation&32)&&(event.rfind("reset:",0)==0||event.rfind("multipart:",0)==0))client->slot.reset();
 if((mutation&64)&&event=="handle")client->slot.reset();
 };Result r;static const char bytes[]="a\0bcdefghijklmnop";
 try{r.returned=rust?::rust::receive(client,end?SW_HTTP2_FLAG_END_STREAM:0,3,length,bytes) : ::native::receive(client,end?SW_HTTP2_FLAG_END_STREAM:0,3,length,bytes);}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.first=ctx.first.bytes;r.second=ctx.second.bytes;r.ended=ctx.end_;r.parse=ctx.parse_body;r.form=ctx.request.post_form_urlencoded;r.multipart=ctx.mt_parser!=nullptr;r.window=client->local_window_size;r.initial=client->local_settings.init_window_size;r.stream_window=weak.expired()?final_stream_window:weak.lock()->local_window_size;r.live=!weak.expired();r.gets=ctx.gets;r.active=ctx.active==&ctx.first?1:ctx.active==&ctx.second?2:0;r.remnants=allocations.size();mutate={};for(auto&p:allocations)std::free(p.first);allocations.clear();return r;
}
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.first==b.first&&a.second==b.second&&a.exception==b.exception&&a.returned==b.returned&&a.ended==b.ended&&a.parse==b.parse&&a.form==b.form&&a.multipart==b.multipart&&a.window==b.window&&a.initial==b.initial&&a.stream_window==b.stream_window&&a.live==b.live&&a.gets==b.gets&&a.active==b.active&&a.remnants==b.remnants;}
int main(){size_t count=0;std::array<std::array<unsigned,3>,4>windows={{{0,0,16},{15,15,16},{UINT32_MAX,UINT32_MAX,UINT32_MAX},{1,0,UINT32_MAX}}};
for(unsigned length:{0u,1u,7u})for(unsigned buffered:{0u,1u,8u})for(unsigned maximum:{0u,1u,8u,UINT32_MAX})for(bool ended:{false,true})for(bool end:{false,true})for(unsigned form:{0u,1u,3u,4u,5u,7u})for(auto window:windows)for(unsigned mutation:{0u,1u,2u,4u,8u,16u,32u,64u,127u})for(bool failure:{false,true}){
 auto a=execute<false>(true,length,buffered,maximum,ended,end,form,window,mutation,failure,0);auto b=execute<true>(true,length,buffered,maximum,ended,end,form,window,mutation,failure,0);if(!same(a,b)){std::cerr<<"receive mismatch length="<<length<<" buffered="<<buffered<<" maximum="<<maximum<<" ended="<<ended<<" end="<<end<<" form="<<form<<" mutation="<<mutation<<" failure="<<failure<<"\n";for(auto&e:a.events)std::cerr<<"native "<<e<<"\n";for(auto&e:b.events)std::cerr<<"rust "<<e<<"\n";return 1;}++count;
}
for(bool exists:{false,true})for(unsigned length:{0u,7u})for(unsigned form:{0u,1u,3u,4u,5u,7u})for(unsigned mutation:{0u,1u,2u,4u,8u,16u,32u,64u,127u})for(bool failure:{false,true}){auto base=execute<false>(exists,length,1,UINT32_MAX,false,true,form,windows[0],mutation,failure,0);for(unsigned thrown=1;thrown<=base.events.size();++thrown){auto a=execute<false>(exists,length,1,UINT32_MAX,false,true,form,windows[0],mutation,failure,thrown);auto b=execute<true>(exists,length,1,UINT32_MAX,false,true,form,windows[0],mutation,failure,thrown);if(!same(a,b)){std::cerr<<"receive exception-prefix mismatch\n";return 2;}++count;}}
std::cout<<"{\"passed\":true,\"receive_cases\":"<<count<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'receive', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    if gate.digest(header) != core['module_header_sha256']:
        report['passed'] = False
        report['error'] = 'Module header changed during checks'
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
