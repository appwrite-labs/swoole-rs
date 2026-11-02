#!/usr/bin/env python3
"""Whole pinned/patched HTTP2 file/range functions with native File/async/allocation traces."""
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
                  'Real PHP/reference/GC, File/syscalls, coroutine/async, MIME/StaticHandler and Zend bailout runtimes',
                  'All native effects and allocation buffers are deterministic doubles; no files/ports/shared fixtures',
                  'Allocated read buffers are initialized before short reads; native unspecified uninitialized suffix bytes are excluded',
                  'Callbacks freeing still-used raw pointers, Rust sanitizer instrumentation and optimized performance']}
    pieces = []
    for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
        methods = []
        signatures = ['static bool http2_server_send_range_file(HttpContext *ctx, StaticHandler *handler) {',
                      'bool swoole_http2_server_send_file(HttpContext *ctx, zend_string *file, off_t offset, size_t length) {']
        if variant == 'rust':
            signatures.insert(0, 'static bool http2_server_complete_file(')
        for signature in signatures:
            fragment = body(text, signature)
            if 'complete_file' in signature:
                signature = 'static bool http2_server_complete_file(HttpContext*ctx,const std::shared_ptr<Http2Stream>&stream,const std::shared_ptr<Http2Session>&client,bool trailer,bool error,bool range){'
            methods.append(signature + fragment + '}')
        pieces.append((variant, 'namespace ' + variant + ' {' + ''.join(methods) + '}'))
    prelude = r'''
#include "swoole_http2.h"
#define SW_USE_RUST_HTTP2
#include "rust_http2.h"
#include <string>
#include <vector>
#include <map>
#include <memory>
#include <functional>
#include <iostream>
#include <stdexcept>
#include <cstring>
#include <fcntl.h>
#undef SW_HAVE_COMPRESSION
#if TEST_COMPRESSION
#define SW_HAVE_COMPRESSION
#endif
#define ZEND_STRL(x) x,sizeof(x)-1
#define SW_ZSTR_KNOWN(x) (x)
#define SW_ZEND_STR_TRAILER 1
#define Z_ARRVAL_P(z) (z)
#define ZSTR_VAL(z) ((z)->bytes.data())
#define ZSTR_LEN(z) ((z)->bytes.size())
struct zval{unsigned length=0;bool contains=false;};
struct zend_string{std::string bytes="fixture.data";};
inline std::vector<std::string>effects;
inline unsigned fail_at,throw_at,read_mode,serial;
inline std::function<void(const std::string&)>mutate;
inline bool effect(std::string what,bool throwing=true){effects.push_back(what);if(mutate)mutate(what);if(throwing&&throw_at==effects.size())throw std::runtime_error("native effect");return fail_at!=effects.size();}
struct String {size_t length=0;char*str=nullptr;std::string bytes;unsigned id;
 String():id(++serial){effect("string:new:"+std::to_string(id));}
 String(const char*data,size_t n):length(n),bytes(data,n),id(++serial){str=bytes.data();effect("string:copy:"+std::to_string(id)+":"+bytes);}
 explicit String(const std::string&data):String(data.data(),data.size()){}
 ~String(){effect("string:drop:"+std::to_string(id),false);}
};
inline zval trailer_property,header_property;
inline int swoole_http_response_ce;
inline zval*sw_zend_read_property_ex(int,zval*,int,int){effect("trailer");return &trailer_property;}
inline size_t php_swoole_array_length_safe(zval*z){effect("trailer_length");return z->length;}
inline zval*sw_zend_read_and_convert_property_array(int,zval*,const char*,size_t,int){effect("header");return &header_property;}
inline bool zend_hash_str_exists(zval*z,const char*,size_t){effect("contains");return z->contains;}
namespace swoole {namespace mime_type {inline const std::string&get(const std::string&name){static const std::string value="mime";effect("mime:"+name);return value;}}}
inline bool coroutine;
inline bool swoole_coroutine_is_in(){effect("coroutine");return coroutine;}
inline std::map<void*,size_t>allocations;
inline void*emalloc(size_t n){effect("allocate:"+std::to_string(n));auto p=std::malloc(n?n:1);if(!p)throw std::bad_alloc();std::memset(p,'?',n);allocations[p]=n;return p;}
inline void efree(void*p){effect("free:"+std::to_string(allocations.at(p)),false);allocations.erase(p);std::free(p);}
struct File {off_t offset=0;
 File(const char*name,int){effect("file:new:"+std::string(name));}
 ~File(){effect("file:drop",false);}
 bool ready(){return effect("file:ready");}
 void set_offset(off_t n){effect("offset:"+std::to_string(n));offset=n;}
 ssize_t read(char*data,size_t n){if(!effect("read:"+std::to_string(n))||read_mode==1)return -1;size_t count=read_mode==2?n/2:n;for(size_t i=0;i<count;++i)data[i]='a'+unsigned(offset+i)%26;return count;}
 std::shared_ptr<String>read_content(){effect("read_content");return std::make_shared<String>("abcdefgh",8);}
};
struct System {static std::shared_ptr<String>read_file(const char*name,bool){if(!effect("read_file:"+std::string(name)))return {};return std::make_shared<String>("abcdefgh",8);}};
namespace swoole {namespace coroutine {template<class F>bool async(F&&callback){if(!effect("async"))return false;return callback();}}}
struct HttpContext {int fd=1;unsigned stream_id=3,end_=0,accept_compression=1;struct Response{zval*zobject;}response;
 void set_header(const char*,size_t,const std::string&value,bool){effect("set_header:"+value);}
 void close(HttpContext*){effect("close:"+std::to_string(end_));}
};
struct Http2Session;
struct Http2Stream {unsigned id=3;
 bool send_header(const String*body,bool end){return effect("send_header:"+std::to_string(body->length)+":"+std::to_string(end));}
 bool send_body(const String*body,bool end,const std::shared_ptr<Http2Session>&,off_t offset=0,size_t length=0){return effect("send_body:"+std::to_string(body->length)+":"+(body->str?std::string(body->str,body->length):std::string())+":"+std::to_string(end)+":"+std::to_string(offset)+":"+std::to_string(length));}
 bool send_trailer(){return effect("send_trailer");}
};
struct Http2Session {std::shared_ptr<Http2Stream>stream=std::make_shared<Http2Stream>();
 std::shared_ptr<Http2Stream>get_stream(unsigned id){effect("get:"+std::to_string(id));return stream;}
 bool remove_stream(unsigned id){return effect("remove:"+std::to_string(id));}
};
inline std::map<int,std::shared_ptr<Http2Session>>http2_sessions;
struct StaticHandler {struct Task{off_t offset;size_t length;const char*part_header;};std::vector<Task>tasks;size_t content=11;
 const std::string&get_content_type(){static const std::string value="range-mime";effect("content_type");return value;}
 size_t get_content_length(){effect("content_length");return content;}
 const std::vector<Task>&get_tasks(){effect("tasks");return tasks;}
 const char*get_filename(){effect("filename");return "range.file";}
 const std::string&get_end_part(){static const std::string value="end-part";effect("end_part");return value;}
};
'''
    driver = r'''
struct Result{std::vector<std::string>events;std::string exception;bool returned=false;unsigned ended=0,stream=0,compression=0,trailer=0;size_t remnants=0;};
template<bool rust>Result execute(bool range,unsigned tasks,bool trailer,bool header,bool coro,unsigned mode,size_t requested,unsigned failed,unsigned thrown,unsigned mutation){
 effects.clear();serial=0;fail_at=failed;throw_at=thrown;read_mode=mode;coroutine=coro;HttpContext ctx;zval object;ctx.response.zobject=&object;trailer_property.length=trailer?1:0;header_property.contains=header;StaticHandler handler;for(unsigned i=0;i<tasks;++i)handler.tasks.push_back({off_t(i*3),mode==3?0:size_t(i+3),"part"});auto client=std::make_shared<Http2Session>();http2_sessions[1]=client;
 mutate=[&](const std::string&event){if((mutation&1)&&event=="header")trailer_property.length=0;if((mutation&2)&&event.rfind("send_body:",0)==0){ctx.stream_id=7;client->stream->id=9;}if((mutation&4)&&event=="send_trailer"){ctx.stream_id=11;client->stream->id=13;}if((mutation&8)&&event=="contains"){header_property.contains=!header_property.contains;coroutine=!coroutine;}};
 zend_string file;Result r;try{r.returned=range?(rust?::rust::http2_server_send_range_file(&ctx,&handler) : ::native::http2_server_send_range_file(&ctx,&handler)):(rust?::rust::swoole_http2_server_send_file(&ctx,&file,2,requested) : ::native::swoole_http2_server_send_file(&ctx,&file,2,requested));}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.ended=ctx.end_;r.stream=ctx.stream_id;r.compression=ctx.accept_compression;r.trailer=trailer_property.length;r.remnants=allocations.size();mutate={};for(auto&p:allocations)std::free(p.first);allocations.clear();http2_sessions.clear();return r;
}
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.exception==b.exception&&a.returned==b.returned&&a.ended==b.ended&&a.stream==b.stream&&a.compression==b.compression&&a.trailer==b.trailer&&a.remnants==b.remnants;}
int main(){size_t count=0;
for(bool range:{false,true})for(unsigned tasks:range?std::vector<unsigned>{0,1,2,3}:std::vector<unsigned>{0})for(bool trailer:{false,true})for(bool header:{false,true})for(bool coro:{false,true})for(unsigned mode:range&&tasks?std::vector<unsigned>{0,1,2,3}:std::vector<unsigned>{0})for(size_t requested:range?std::vector<size_t>{8}:std::vector<size_t>{0,1,8,99,SIZE_MAX})for(unsigned mutation=0;mutation<16;++mutation){
 auto baseline=execute<false>(range,tasks,trailer,header,coro,mode,requested,0,0,mutation);auto candidate=execute<true>(range,tasks,trailer,header,coro,mode,requested,0,0,mutation);if(!same(baseline,candidate)){std::cerr<<"file baseline mismatch\n";return 1;}++count;
 for(unsigned index=1;index<=baseline.events.size();++index)for(bool throwing:{false,true}){
 auto a=execute<false>(range,tasks,trailer,header,coro,mode,requested,throwing?0:index,throwing?index:0,mutation);auto b=execute<true>(range,tasks,trailer,header,coro,mode,requested,throwing?0:index,throwing?index:0,mutation);if(!same(a,b)){std::cerr<<"file prefix mismatch range="<<range<<" tasks="<<tasks<<" trailer="<<trailer<<" header="<<header<<" coroutine="<<coro<<" mode="<<mode<<" requested="<<requested<<" failure/throw="<<index<<" throwing="<<throwing<<" mutation="<<mutation<<"\n";return 2;}++count;
 }
}
for(size_t wanted:{size_t(0),size_t(1),size_t(8),SIZE_MAX})for(size_t actual:{size_t(0),size_t(1),size_t(8),SIZE_MAX}){if(sw_rs_http2_file_length(wanted,actual)!=(wanted<actual?wanted:actual))return 3;++count;}
std::cout<<"{\"passed\":true,\"file_cases\":"<<count<<",\"compression_enabled\":"<<TEST_COMPRESSION<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    commands, report['variants'] = sides.variants(work, 'files', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include, define='TEST_COMPRESSION')
    report['commands'] += commands
    report['passed'] = len(report['variants']) == 2 and all(v['passed'] for v in report['variants'])
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
