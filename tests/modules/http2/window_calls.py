#!/usr/bin/env python3
"""Actual server/client flow and WINDOW_UPDATE fragments with native reentry effects.

Routing oracle uses pinned switch labels and following stream-kind predicates;
it does not execute the full PHP frame parsers.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
import gate
import sides
from settings_calls import body,select_overlay


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--core-report',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--include',action='append',default=[]);a=p.parse_args();core=json.loads(a.core_report.read_text())
    if not core['passed']:raise ValueError('Completed core receipt required')
    artifacts=a.core_report.parent/(a.core_report.stem+'-artifacts');archive=Path(core.get('archive',artifacts/'libhttp2.a'))
    if gate.digest(archive)!=core['archive_sha256']:raise ValueError('Archive changed')
    report={'passed':False,'scope':__doc__,'upstream_commit':gate.PIN,'commands':[],
            'gaps':['full PHP frame parser execution','actual IO/Coroutine/Zend runtime and bailout','all possible destruction schedules','Rust sanitizer instrumentation','performance']}
    pieces=[];oracles=[]
    mapping={'SETTINGS':'SETTINGS','HEADERS':'HEADERS','DATA':'DATA','PING':'PING_REPLY','WINDOW_UPDATE':'WINDOW_UPDATE','RST_STREAM':'RESET','GOAWAY':'GOAWAY','PUSH_PROMISE':'IGNORE_PUSH'}
    for role,filename,marker in [('server','ext-src/swoole_http2_server.cc','int swoole_http2_server_parse('),('client','ext-src/swoole_http2_client_coro.cc','ReturnCode Client::parse_frame(')]:
        original=subprocess.check_output(['git','show',f'{gate.PIN}:{filename}'],cwd=a.source,text=True);selected=artifacts/'overlay'/filename
        if gate.digest(selected)!=core['patched_sources'][filename]:raise ValueError('Overlay changed')
        native_parser=body(original,marker);labels=re.findall(r'case SW_HTTP2_TYPE_(\w+):',native_parser)
        if role=='client':
            for label in ['HEADERS','DATA']:
                if f'type == SW_HTTP2_TYPE_{label}' not in native_parser:raise ValueError('Pinned stream predicate changed')
                labels.append(label)
        if len(set(labels))!=len(labels):raise ValueError('Duplicate native type routing labels')
        cases=''.join('case SW_HTTP2_TYPE_'+label+': return SW_RS_HTTP2_'+mapping[label]+';' for label in labels)
        oracles.append('inline unsigned '+role+'_route(unsigned type,unsigned flags){auto action=[&](){switch(type){'+cases+'default:return SW_RS_HTTP2_OTHER;}}();if(action==SW_RS_HTTP2_SETTINGS&&(flags&SW_HTTP2_FLAG_ACK))return SW_RS_HTTP2_SETTINGS_ACK;if(action==SW_RS_HTTP2_PING_REPLY&&(flags&SW_HTTP2_FLAG_ACK))return SW_RS_HTTP2_PING_ACK;return action;}\n')
        for variant,text in [('native',original),('rust',select_overlay(selected.read_text()))]:
            parser=body(text,marker);prefix=parser[:parser.index('    switch (type)' if variant=='native' else '    switch (frame_action)')];remote=body(parser,'case '+('SW_HTTP2_TYPE_WINDOW_UPDATE' if variant=='native' else 'SW_RS_HTTP2_WINDOW_UPDATE'))
            start=parser.index('// flow control' if role=='server' else '// now we control the connection flow')
            ending='\n        }\n\n        if (flags & SW_HTTP2_FLAG_END_STREAM)' if role=='server' else '\n        }\n    }\n\n'
            if role == 'server' and variant == 'rust' and 'case SW_RS_HTTP2_RECV_FLOW:' in parser:
                flow = body(parser, 'case SW_RS_HTTP2_RECV_FLOW: {').rsplit('break;', 1)[0]
            else:
                flow=parser[start:parser.index(ending,start)]
            remote_signature='int remote(const char *buf,uint32_t '+('stream_id' if role=='server' else '_stream_id')+')'
            remote_prefix='auto client=this;uint32_t value=0;' if role=='server' else ''
            flow_prefix='auto client=this;HttpContext storage{this};auto ctx=&storage;' if role=='server' else ''
            flow_signature='int flow(Stream *stream,ssize_t length,uint32_t '+('stream_id' if role=='server' else '_stream_id')+')'
            id_variable='stream_id' if role=='server' else '_stream_id'
            prefix_setup='auto client=this;' if role=='server' else 'FixtureSocket socket;socket.buffer.str=(char*)input;socket_=&socket;'
            prefix_signature='int prefix_frame(const char *'+('buf' if role=='server' else 'input')+',uint32_t *out)'
            route='frame_action' if variant=='rust' else role+'_route(type, flags)'
            prefix_method=prefix_signature+'{'+prefix_setup+prefix+'out[0]=type;out[1]=flags;out[2]='+id_variable+';out[3]=length;return '+route+';}'
            lookup='Stream *get_stream(uint32_t id){return Base::get_stream(id).get();}' if role=='client' else ''
            pieces.append((variant, 'namespace '+role+'_'+variant+' {struct State: Base {'+lookup+prefix_method+remote_signature+'{'+remote_prefix+'do{'+remote+'}while(false);return SW_SUCCESS;}'+flow_signature+'{'+flow_prefix+flow+'return SW_SUCCESS;}};}\n'))
    prelude=r'''
#include "swoole_http2.h"
#include "swoole_error.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <array>
#include <vector>
#include <string>
#include <memory>
#include <functional>
#include <iostream>
#include <cstdarg>
using namespace swoole;
namespace Http2=swoole::http2;
inline std::vector<std::string> effects;
inline void emit(const char*fmt,...){char b[256];va_list v;va_start(v,fmt);vsnprintf(b,sizeof(b),fmt,v);va_end(v);effects.push_back(b);}
#undef swoole_http2_frame_trace_log
#define swoole_http2_frame_trace_log(...) emit(__VA_ARGS__)
#undef swoole_error_log
#define swoole_error_log(level,error,...) emit(__VA_ARGS__)
#undef swoole_trace_log
#define swoole_trace_log(flags,...) emit(__VA_ARGS__)
struct Coroutine {std::function<void()> callback;void resume(){effects.push_back("resume");if(callback)callback();}};
struct Stream {uint32_t local_window_size=0,remote_window_size=0;Coroutine *waiting_coroutine=nullptr;};
struct FixtureSocket {struct Buffer {char *str;} buffer;Buffer *get_read_buffer(){effects.push_back("read-buffer");return &buffer;}};
struct Base {FixtureSocket *socket_=nullptr;uint32_t last_stream_id=0;bool shutting_down=false;uint32_t local_window_size=0,remote_window_size=0,stream_id=17;swoole::http2::Settings local_settings{};bool is_coro=false;std::shared_ptr<Stream> live;unsigned scenario=0,calls=0;
 std::shared_ptr<Stream> get_stream(uint32_t id){effects.push_back("get:"+std::to_string(id));return live;}
 bool send_window_update(uint32_t id,uint32_t amount){++calls;effects.push_back("send:"+std::to_string(id)+":"+std::to_string(amount));if(id==0&&(scenario&1)){local_settings.init_window_size=19;if(live)live->local_window_size=0;local_window_size=71;}return !((scenario&2)&&calls==1)&&!((scenario&4)&&calls==2);}
};
struct HttpContext {Base *state;};
inline void http2_server_send_window_update(HttpContext *ctx,uint32_t id,uint32_t amount){ctx->state->send_window_update(id,amount);}
'''
    driver=r'''
struct Result {std::vector<std::string> events;uint32_t local,remote,stream_local,stream_remote,initial;bool live;int code;unsigned calls;};
template<class T>Result execute(bool remote,uint32_t current,uint32_t stream_window,uint32_t initial,size_t amount,unsigned scenario,bool exists,bool coro,bool waiting,uint32_t id){
 effects.clear();T state;state.local_window_size=state.remote_window_size=current;state.local_settings.init_window_size=initial;state.scenario=scenario;state.is_coro=coro;auto stream=std::make_shared<Stream>();stream->local_window_size=stream->remote_window_size=stream_window;if(exists)state.live=stream;
 Coroutine coroutine;coroutine.callback=[&](){if(scenario&8){state.live.reset();stream->remote_window_size=91;}};stream->waiting_coroutine=waiting?&coroutine:nullptr;
 alignas(4) std::array<char,4> bytes;uint32_t encoded=htonl(amount);memcpy(bytes.data(),&encoded,4);
 int code=remote?state.remote(bytes.data(),id):state.flow(stream.get(),amount,id);
 return {effects,state.local_window_size,state.remote_window_size,stream->local_window_size,stream->remote_window_size,state.local_settings.init_window_size,bool(state.live),code,state.calls};
}
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.local==b.local&&a.remote==b.remote&&a.stream_local==b.stream_local&&a.stream_remote==b.stream_remote&&a.initial==b.initial&&a.live==b.live&&a.code==b.code&&a.calls==b.calls;}
int main(){size_t count=0;std::array<uint32_t,10> windows={0,1,2,3,4,15,16,65535,0x80000000,UINT32_MAX};
for(auto current:windows)for(auto stream:windows)for(auto initial:windows)for(size_t amount:{size_t(1),size_t(2),size_t(16),size_t(65535),size_t(0xffffff)})for(unsigned scenario=0;scenario<8;++scenario){
 auto a=execute<server_native::State>(false,current,stream,initial,amount,scenario,true,false,false,1);auto b=execute<server_rust::State>(false,current,stream,initial,amount,scenario,true,false,false,1);if(!same(a,b)){std::cerr<<"server flow mismatch\n";return 1;}
 auto c=execute<client_native::State>(false,current,stream,initial,amount,scenario,true,false,false,1);auto d=execute<client_rust::State>(false,current,stream,initial,amount,scenario,true,false,false,1);if(!same(c,d)){std::cerr<<"client flow mismatch\n";return 2;}count+=2;
}
for(auto current:windows)for(auto stream:windows)for(auto amount:windows)for(unsigned scenario:{0u,8u})for(bool exists:{false,true})for(bool coro:{false,true})for(bool waiting:{false,true})for(uint32_t id:{0u,1u}){
 auto a=execute<server_native::State>(true,current,stream,0,amount,scenario,exists,coro,waiting,id);auto b=execute<server_rust::State>(true,current,stream,0,amount,scenario,exists,coro,waiting,id);if(!same(a,b))return 3;
 auto c=execute<client_native::State>(true,current,stream,0,amount,scenario,exists,coro,waiting,id);auto d=execute<client_rust::State>(true,current,stream,0,amount,scenario,exists,coro,waiting,id);if(!same(c,d))return 4;count+=2;
}
for(unsigned client=0;client<2;++client)for(unsigned type=0;type<256;++type)for(unsigned flags=0;flags<256;++flags){unsigned native=client?client_route(type,flags):server_route(type,flags);alignas(8) std::array<char,16> backing{};auto bytes=backing.data()+3;Http2::set_frame_header(bytes,type,0,flags,0);SwRsHttp2Frame plan;
 if(!sw_rs_http2_decode_frame((uint8_t*)bytes,9,0,client,&plan)||plan.action!=native)return 5;++count;}
for(size_t current:{size_t(0),size_t(1),size_t(UINT32_MAX),SIZE_MAX})for(size_t amount:{size_t(0),size_t(1),size_t(UINT32_MAX),SIZE_MAX})for(uint32_t maximum:{0u,1u,UINT32_MAX})for(bool end:{false,true}){if(bool(sw_rs_http2_data_rejected(current,amount,maximum,end))!=(current+amount>maximum||end))return 6;++count;}
// Execute the actual pinned/patched parser prefix, including last-id publication and shutdown.
for(unsigned type=0;type<256;++type)for(unsigned flags=0;flags<256;++flags)for(uint32_t previous:{0u,0x7fffffffu,UINT32_MAX})for(bool shutdown:{false,true}){
 alignas(8) std::array<char,16> backing{};auto bytes=backing.data()+3;uint32_t id=(type*0x01010101u)^flags;
 Http2::set_frame_header(bytes,type,(type<<16)|(flags<<8)|type,flags,id);
 server_native::State sn;server_rust::State sr;sn.last_stream_id=sr.last_stream_id=previous;sn.shutting_down=sr.shutting_down=shutdown;
 std::array<uint32_t,4> an{},bn{};effects.clear();auto ac=sn.prefix_frame(bytes,an.data());auto ae=effects;effects.clear();auto bc=sr.prefix_frame(bytes,bn.data());
 if(ac!=bc||an!=bn||ae!=effects||sn.last_stream_id!=sr.last_stream_id)return 7;
 client_native::State cn;client_rust::State cr;cn.last_stream_id=cr.last_stream_id=previous;effects.clear();ac=cn.prefix_frame(bytes,an.data());ae=effects;effects.clear();bc=cr.prefix_frame(bytes,bn.data());
 if(ac!=bc||an!=bn||ae!=effects||cn.last_stream_id!=cr.last_stream_id)return 8;count+=2;
}
std::cout<<"{\"passed\":true,\"window_routing_cases\":"<<count<<"}\n";
}
'''
    work = a.output.parent / (a.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'windows', prelude + ''.join(oracles), pieces, driver, source=a.source, archive=archive, includes=a.include)
    report.update(result or {})
    report['native_sanitizers']=['address','undefined except alignment'];report['rust_sanitized']=False;a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'passed':report['passed'],'report':str(a.output)}));return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
