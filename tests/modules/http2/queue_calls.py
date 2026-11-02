#!/usr/bin/env python3
"""Actual send/clean bodies: native std::queue versus sole Rust container with native effects."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
import gate
import sides
from settings_calls import body,select_overlay


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--core-report',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--include',action='append',default=[]);a=p.parse_args()
    core=json.loads(a.core_report.read_text())
    if not core['passed']:raise ValueError('Completed core receipt required')
    work_core=a.core_report.parent/(a.core_report.stem+'-artifacts');archive=Path(core.get('archive',work_core/'libhttp2.a'))
    if gate.digest(archive)!=core['archive_sha256']:raise ValueError('Archive changed')
    header=gate.ROOT/'native/http2/rust_http2.h'
    if gate.digest(header)!=core.get('module_header_sha256'):raise ValueError('Module header changed')
    name='ext-src/swoole_http2_client_coro.cc';selected=work_core/'overlay'/name
    if gate.digest(selected)!=core['patched_sources'][name]:raise ValueError('Overlay changed')
    original=subprocess.check_output(['git','show',f'{gate.PIN}:{name}'],cwd=a.source,text=True);pieces=[]
    report={'passed':False,'scope':__doc__,'upstream_commit':gate.PIN,'commands':[],
            'gaps':['actual PHP/coroutine/socket runtime and native Zend longjmp bailout','Physical allocation counts/backing/teardown timing differ; actual system OOM not forced','Rust sanitizer instrumentation','performance']}
    for variant,text in [('native',original),('rust',select_overlay(selected.read_text()))]:
        methods=[]
        for signature in ['bool send(const char *buf, size_t len)','void clean_send_queue()']:
            fragment=body(text,signature);methods.append(signature+'{'+fragment+'}')
        pieces.append((variant, 'namespace '+variant+' { struct Client: '+('RustFixtureClient' if variant=='rust' else 'NativeFixtureClient')+' {'+''.join(methods)+'};}\n'))
    prelude=r'''
#include "swoole_http2.h"
#include "swoole_error.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <queue>
#include <set>
#include <functional>
#include <iostream>
#include <stdexcept>
#include <type_traits>
using namespace swoole;
inline std::vector<std::string> effects;
inline unsigned serial,allocation_failure;
struct zend_string {std::string bytes;char *val;size_t len;unsigned id;};
inline std::set<zend_string*> live;
inline zend_string *zend_string_init(const char *buf,size_t n,bool persistent){
 effects.push_back("allocate:"+std::string(buf,n));if(allocation_failure)throw std::runtime_error("native allocation");
 auto s=new zend_string{std::string(buf,n),nullptr,n,serial++};s->val=s->bytes.data();live.insert(s);return s;}
inline void zend_string_release(zend_string*s){effects.push_back("release:"+std::to_string(s->id));if(!live.erase(s))std::abort();delete s;}
inline int swoole_http2_client_coro_exception_ce;
inline bool throw_exception;
inline void zend_throw_exception(int,const char*what,int code){effects.push_back("exception:"+std::to_string(code)+":"+what);if(throw_exception)throw std::runtime_error("native exception");}
struct FixtureSocket {bool bound=false;unsigned calls=0,fail_call=0,result_kind=0;int errCode=0;std::string errMsg;std::function<void()> reenter;
 bool has_bound(int event){effects.push_back("bound:"+std::to_string(event)+":"+std::to_string(bound));return bound;}
 ssize_t send_all(const char *buf,size_t n){++calls;effects.push_back("send:"+std::string(buf,n));if(reenter)reenter();if(calls==fail_call){if(result_kind==0)return -1;if(result_kind==1)return 0;if(result_kind==2)return ssize_t(n)-1;return ssize_t(n)+1;}return n;}
};
template<class Queue>struct FixtureClient {FixtureSocket *socket_;Queue send_queue;swoole::http2::Settings remote_settings{};std::function<void()> on_error;
 void io_error(){effects.push_back("io:"+std::to_string(socket_->errCode)+":"+socket_->errMsg);if(on_error)on_error();}
};
using NativeFixtureClient=FixtureClient<std::queue<zend_string*>>;
using RustFixtureClient=FixtureClient<swoole::http2::RustSendQueue<zend_string>>;
'''
    driver=r'''
struct Result {std::vector<std::string> events;bool returned=false;std::string exception;unsigned calls=0;int error=0;std::string message;size_t remaining=0;};
template<class C>Result execute(bool bound,unsigned queued,unsigned limit,unsigned fail_call,unsigned result_kind,unsigned scenario){
 if(!live.empty())std::abort();serial=0;allocation_failure=0;effects.clear();FixtureSocket socket;socket.bound=bound;socket.fail_call=fail_call;socket.result_kind=result_kind;C client;client.socket_=&socket;client.remote_settings.max_concurrent_streams=limit;
 for(unsigned i=0;i<queued;++i){std::string v="queued"+std::to_string(i);client.send_queue.push(zend_string_init(v.data(),v.size(),false));}effects.clear();
 bool reentered=false;socket.reenter=[&](){if(!reentered&&(scenario&1)){reentered=true;socket.bound=true;if(scenario&2)client.remote_settings.max_concurrent_streams=0;auto result=client.send("nested",6);effects.push_back("nested:"+std::to_string(result));socket.bound=false;}};
 client.on_error=[&](){if(scenario&4)client.clean_send_queue();if(scenario&8)client.remote_settings.max_concurrent_streams=0;};
 throw_exception=scenario&16;allocation_failure=scenario&32;Result result;
 try{result.returned=client.send("first",5);}catch(const std::runtime_error&e){result.exception=e.what();}
 result.remaining=client.send_queue.size();allocation_failure=0;client.clean_send_queue();
 result.events=effects;result.calls=socket.calls;result.error=socket.errCode;result.message=socket.errMsg;if(!live.empty())std::abort();return result;
}
static zend_string*peek(std::queue<zend_string*>&queue){return queue.empty()?nullptr:queue.front();}
static zend_string*peek(swoole::http2::RustSendQueue<zend_string>&queue){return queue.front_or_null();}
struct OwnershipResult {std::vector<std::string>events;size_t stored=0,remaining=0;};
template<class Queue>OwnershipResult residual(unsigned depth){
 if(!live.empty())std::abort();serial=0;allocation_failure=0;effects.clear();OwnershipResult result;
 {Queue queue;for(unsigned i=0;i<depth;++i)queue.push(zend_string_init("residual",8,false));result.stored=queue.size();}
 result.remaining=live.size();result.events=effects;
 auto leftovers=live;for(auto payload:leftovers)zend_string_release(payload);return result;
}
template<class Queue>std::vector<std::string>growth(unsigned steps){
 if(!live.empty())std::abort();serial=0;allocation_failure=0;effects.clear();
 {Queue queue;for(unsigned i=0;i<steps;++i){auto copied=peek(queue);queue.push(zend_string_init("growth",6,false));if(copied&&peek(queue)!=copied)std::abort();if(i%3!=0){auto payload=peek(queue);effects.push_back("front:"+std::to_string(payload->id));queue.pop();zend_string_release(payload);}}
 while(auto payload=peek(queue)){effects.push_back("front:"+std::to_string(payload->id));queue.pop();zend_string_release(payload);}}
 if(!live.empty())std::abort();return effects;
}
int main(){size_t count=0;
static_assert(!std::is_copy_constructible<swoole::http2::RustSendQueue<zend_string>>::value,"one owner");
static_assert(!std::is_copy_assignable<swoole::http2::RustSendQueue<zend_string>>::value,"one owner");
for(unsigned depth:{0u,1u,3u,4u,17u,128u,1025u}){auto a=residual<std::queue<zend_string*>>(depth);auto b=residual<swoole::http2::RustSendQueue<zend_string>>(depth);if(a.events!=b.events||a.stored!=b.stored||a.remaining!=b.remaining||a.remaining!=depth)return 3;++count;}
for(unsigned steps:{0u,1u,31u,257u,4096u}){if(growth<std::queue<zend_string*>>(steps)!=growth<swoole::http2::RustSendQueue<zend_string>>(steps))return 4;++count;}
auto previous_handler=std::set_new_handler(nullptr);
bool translated=false;try{swoole::http2::rust_send_queue_allocated(false);}catch(const std::bad_alloc&e){translated=std::string(e.what())==std::bad_alloc().what();}if(!translated)return 5;
if(!swoole::http2::rust_send_queue_allocated(true))return 6;++count;
static unsigned retries=0;std::set_new_handler([](){++retries;});if(swoole::http2::rust_send_queue_allocated(false)||retries!=1)return 7;++count;
std::set_new_handler([](){throw std::runtime_error("native new handler");});bool propagated=false;try{swoole::http2::rust_send_queue_allocated(false);}catch(const std::runtime_error&e){propagated=std::string(e.what())=="native new handler";}if(!propagated)return 8;++count;
std::set_new_handler(previous_handler);
for(bool bound:{false,true})for(unsigned queued:{0,1,2,4})for(unsigned limit:{0u,1u,2u,3u,4u,UINT32_MAX})for(unsigned fail:{0,1,2,3})for(unsigned kind:{0,1,2,3})for(unsigned scenario=0;scenario<64;++scenario){
 auto a=execute<native::Client>(bound,queued,limit,fail,kind,scenario);auto b=execute<rust::Client>(bound,queued,limit,fail,kind,scenario);
 if(a.events!=b.events||a.returned!=b.returned||a.exception!=b.exception||a.calls!=b.calls||a.error!=b.error||a.message!=b.message||a.remaining!=b.remaining){std::cerr<<"queue mismatch bound="<<bound<<" queued="<<queued<<" limit="<<limit<<" fail="<<fail<<" kind="<<kind<<" scenario="<<scenario<<"\n";return 1;}++count;
}
// Preserve the native signed-size comparison at all representable boundary facts.
for(size_t wanted:{size_t(0),size_t(1),size_t(INTPTR_MAX),size_t(INTPTR_MAX)+1,SIZE_MAX})for(ssize_t sent:{ssize_t(-1),ssize_t(0),ssize_t(1),INTPTR_MIN,INTPTR_MAX}){if(bool(sw_rs_http2_send_complete(sent,wanted))!=(sent==(ssize_t)wanted))return 2;++count;}
std::cout<<"{\"passed\":true,\"queue_cases\":"<<count<<",\"native_queue_object_bytes\":"<<sizeof(std::queue<zend_string*>)<<",\"candidate_member_bytes\":"<<sizeof(swoole::http2::RustSendQueue<zend_string>)<<"}\n";
}
'''
    work = a.output.parent / (a.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    report['commands'], result = sides.run(work, 'queue', prelude, pieces, driver, source=a.source, archive=archive, includes=a.include)
    report.update(result or {})
    report['harness_sha256'] = {cpp.name: gate.digest(cpp) for cpp in sorted(work.glob('*.cpp'))}
    if gate.digest(header)!=core['module_header_sha256']:
        report['passed']=False;report['error']='Module header changed during queue checks'
    report['native_sanitizers']=['address','undefined except alignment'];report['rust_sanitized']=False
    report['physical_scope']='candidate_member_bytes excludes the opaque Rust allocation and VecDeque backing; total footprint and system-OOM schedules remain unmeasured'
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'passed':report['passed'],'report':str(a.output)}));return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
