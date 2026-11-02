#!/usr/bin/env python3
"""Actual server body/header and client DATA send loops with native IO/resume effects."""
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
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'commands': [], 'gaps': [
                  'Real PHP, Socket, compression, coroutine runtime and Zend longjmp',
                  'Callbacks freeing still-used raw streams/contexts/body buffers are outside defined fixture lifetimes',
                  'Zero max-frame non-progress schedules are not executed without a terminating IO failure',
                  'Native header builder is a scripted effect in send_header checks',
                  'Rust sanitizer instrumentation and performance']}
    pieces = []
    for role, name, markers in [
            ('server', 'ext-src/swoole_http2_server.cc', ['bool Http2Stream::send_header(', 'bool Http2Stream::send_body(']),
            ('client', 'ext-src/swoole_http2_client_coro.cc', ['bool Client::send_data('])]:
        original = subprocess.check_output(['git', 'show', f'{gate.PIN}:{name}'], cwd=args.source, text=True)
        selected = artifacts / 'overlay' / name
        if gate.digest(selected) != core['patched_sources'][name]:
            raise ValueError('Overlay changed')
        for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
            methods = []
            for marker in markers:
                fragment = body(text, marker)
                signature = ('bool send_header(const BufferValue *body,bool end_stream)' if 'send_header' in marker else
                             'bool send_body(const BufferValue *body,bool end_stream,const std::shared_ptr<FixtureSession>&session,off_t offset,size_t length)' if 'send_body' in marker else
                             'bool send_data(uint32_t _stream_id,const char*p,size_t len,int flag)')
                methods.append(signature + '{' + fragment + '}')
            pieces.append((variant, 'namespace ' + role + '_' + variant + ' {struct State: ' + ('ServerFixture' if role == 'server' else 'ClientFixture') + ' {using String=BufferValue;using Coroutine=FixtureCoroutine;' + ''.join(methods) + '};}'))
    prelude = r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <memory>
#include <functional>
#include <iostream>
#include <stdexcept>
using namespace swoole;
namespace Http2=swoole::http2;
inline std::vector<std::string> effects;
inline unsigned throw_at,change,send_calls,fail_call,page_size=8;
inline std::function<void()> mutate;
inline void effect(std::string text){effects.push_back(text);if(throw_at==effects.size())throw std::runtime_error("native effect");}
struct BufferValue {char storage[256]{};char*str=storage;size_t length=0;
 void clear(){effect("clear");length=0;}
 void append(const char*p,size_t n){effect("append:"+std::string(p,n));if(length+n>sizeof(storage))std::abort();std::memcpy(storage+length,p,n);length+=n;}
};
struct FixtureContext {BufferValue write;bool co_socket=false;unsigned end_=0,send_header_=1;
 BufferValue *get_write_buffer(){effect("buffer");return &write;}
 bool is_co_socket(){effect("co-socket");return co_socket;}
 bool send(FixtureContext*,const char*p,size_t n){effect("send:"+std::string(p,n));if(mutate)mutate();return ++send_calls!=fail_call;}
};
struct FixtureCoroutine {std::function<void()> resume;inline static FixtureCoroutine *active;
 static FixtureCoroutine *get_current(){effect("get-current");return active;}
 void yield(){effect("yield");if(resume)resume();}
};
struct FixtureSession {swoole::http2::Settings local_settings{};uint32_t remote_window_size=0;};
inline bool coroutine_in;
#define swoole_coroutine_is_in fixture_coroutine_is_in
inline bool fixture_coroutine_is_in(){effect("coroutine-in");return coroutine_in;}
#define swoole_pagesize fixture_pagesize
inline size_t fixture_pagesize(){effect("pagesize");return page_size;}
#undef swoole_warning
#define swoole_warning(...) effect("warning")
#undef swoole_trace_log
#define swoole_trace_log(...) ((void)0)
inline ssize_t header_result;
inline ssize_t http2_server_build_header(FixtureContext*,unsigned char*p,const BufferValue*){effect("header-build");if(header_result>0)std::memset(p,'H',header_result);return header_result;}
struct ServerFixture {FixtureContext *ctx;uint32_t id=3,remote_window_size=0;FixtureCoroutine *waiting_coroutine=nullptr;};
struct ClientFixture {swoole::http2::Settings local_settings{};
 bool send(const char*p,size_t n){effect("send:"+std::string(p,n));if(mutate)mutate();return ++send_calls!=fail_call;}
};
'''
    driver = r'''
struct Result {std::vector<std::string> events;bool returned=false,waiting=false;unsigned window=0,session_window=0,maximum=0,calls=0,end=0,header=0;std::string exception;};
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.returned==b.returned&&a.waiting==b.waiting&&a.window==b.window&&a.session_window==b.session_window&&a.maximum==b.maximum&&a.calls==b.calls&&a.end==b.end&&a.header==b.header&&a.exception==b.exception;}
template<class C>Result server(bool header,unsigned len,unsigned offset,unsigned length,unsigned maximum,unsigned window,bool end,bool co,bool in,unsigned fail,unsigned mutation,unsigned throw_index){
 effects.clear();throw_at=throw_index;change=mutation;send_calls=0;fail_call=fail;coroutine_in=in;
 C state;FixtureContext context;context.co_socket=co;state.ctx=&context;state.remote_window_size=window;
 auto session=std::make_shared<FixtureSession>();session->local_settings.max_frame_size=maximum;session->remote_window_size=window+7;BufferValue data;data.length=len;std::memset(data.str,'B',sizeof(data.storage));FixtureCoroutine coroutine;FixtureCoroutine::active=&coroutine;
 coroutine.resume=[&](){state.remote_window_size=64;session->local_settings.max_frame_size=2;};
 mutate=[&](){if(change&1){state.remote_window_size=64;session->remote_window_size=71;}if(change&2){session->local_settings.max_frame_size=2;context.end_^=1;context.send_header_=9;}};Result r;
 try{r.returned=header?state.send_header(length?&data:nullptr,end):state.send_body(&data,end,session,offset,length);}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.window=state.remote_window_size;r.session_window=session->remote_window_size;r.maximum=session->local_settings.max_frame_size;r.waiting=state.waiting_coroutine!=nullptr;r.calls=send_calls;r.end=context.end_;r.header=context.send_header_;mutate={};return r;
}
template<class C>Result client(unsigned len,unsigned maximum,int flags,unsigned fail,unsigned mutation,unsigned throw_index){
 effects.clear();throw_at=throw_index;send_calls=0;fail_call=fail;C state;state.local_settings.max_frame_size=maximum;char bytes[64];std::memset(bytes,'D',sizeof(bytes));mutate=[&](){if(mutation)state.local_settings.max_frame_size=2;};Result r;
 try{r.returned=state.send_data(3,bytes,len,flags);}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.calls=send_calls;r.maximum=state.local_settings.max_frame_size;mutate={};return r;
}
int main(){size_t count=0;
for(unsigned len:{0u,1u,7u,16u})for(unsigned offset:{0u,1u})for(unsigned length:{0u,1u,13u})for(unsigned maximum:{1u,7u,16u})for(unsigned window:{0u,1u,8u,16u})for(bool end:{false,true})for(bool co:{false,true})for(bool in:{false,true})for(unsigned fail:{0u,1u,2u,3u})for(unsigned mutation=0;mutation<4;++mutation){
 auto a=server<server_native::State>(false,len,offset,length,maximum,window,end,co,in,fail,mutation,0);auto b=server<server_rust::State>(false,len,offset,length,maximum,window,end,co,in,fail,mutation,0);if(!same(a,b)){std::cerr<<"server body mismatch len="<<len<<" offset="<<offset<<" length="<<length<<" maximum="<<maximum<<" window="<<window<<" end="<<end<<" co="<<co<<" in="<<in<<" fail="<<fail<<" mutation="<<mutation<<"\n";return 1;}++count;
}
for(unsigned len:{0u,1u,13u})for(unsigned maximum:{0u,1u,7u,16u})for(int flags:{0,1,255})for(unsigned fail:{0u,1u,2u,3u})for(unsigned mutation:{0u,1u})for(unsigned throw_index=0;throw_index<9;++throw_index){
 if(maximum==0&&len&&fail==0)continue;
 auto a=client<client_native::State>(len,maximum,flags,fail,mutation,throw_index);auto b=client<client_rust::State>(len,maximum,flags,fail,mutation,throw_index);if(!same(a,b)){std::cerr<<"client DATA mismatch\n";return 2;}++count;
}
for(unsigned len:{0u,1u,13u})for(unsigned length:{0u,1u})for(bool end:{false,true})for(ssize_t header:{ssize_t(-1),ssize_t(0),ssize_t(5)})for(unsigned fail:{0u,1u})for(unsigned mutation:{0u,1u})for(unsigned throw_index=0;throw_index<10;++throw_index){
 header_result=header;auto a=server<server_native::State>(true,len,0,length,16,16,end,false,false,fail,mutation,throw_index);auto b=server<server_rust::State>(true,len,0,length,16,16,end,false,false,fail,mutation,throw_index);if(!same(a,b)){std::cerr<<"server header mismatch\n";return 3;}++count;
}
// Partial native effect prefixes in representative bounded send/yield paths.
for(unsigned window:{0u,1u,16u})for(bool co:{false,true})for(bool in:{false,true})for(unsigned throw_index=1;throw_index<32;++throw_index){
 auto a=server<server_native::State>(false,13,0,0,7,window,true,co,in,0,3,throw_index);auto b=server<server_rust::State>(false,13,0,0,7,window,true,co,in,0,3,throw_index);if(!same(a,b)){std::cerr<<"server body exception mismatch\n";return 4;}++count;
}
std::cout<<"{\"passed\":true,\"send_cases\":"<<count<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'send', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include)
    report.update(result or {})
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
