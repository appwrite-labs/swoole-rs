#!/usr/bin/env python3
"""Actual server response lifecycle with native header/body/trailer/close/remove effects."""
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
    name = 'ext-src/swoole_http2_server.cc'
    selected = artifacts / 'overlay' / name
    if gate.digest(selected) != core['patched_sources'][name]:
        raise ValueError('Overlay changed')
    if gate.digest(gate.ROOT / 'native/http2/rust_http2.h') != core['module_header_sha256']:
        raise ValueError('Module header changed')
    original = subprocess.check_output(['git', 'show', f'{gate.PIN}:{name}'], cwd=args.source, text=True)
    report = {'passed': False, 'scope': __doc__, 'upstream_commit': gate.PIN,
              'commands': [], 'gaps': [
                  'Actual PHP/Socket/HPACK/compression/coroutine/GC and Zend longjmp runtimes',
                  'Native stream methods are scripted effects here; complete bodies have separate module gates',
                  'Callbacks freeing a still-used native raw Context/body are outside defined fixture lifetimes',
                  'Rust sanitizer instrumentation and performance']}
    pieces = []
    for variant, text in [('native', original), ('rust', select_overlay(selected.read_text()))]:
        fragment = body(text, 'static bool http2_server_respond(HttpContext *ctx, const String *body) {')
        pieces.append((variant, 'namespace ' + variant + ' {using String=BufferValue;bool respond(HttpContext*ctx,const BufferValue*body){' + fragment + '}}'))
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
using namespace swoole;
#undef SW_HAVE_COMPRESSION
#if TEST_COMPRESSION
#define SW_HAVE_COMPRESSION
#endif
#define SW_ZSTR_KNOWN(x) (x)
#define SW_ZEND_STR_TRAILER 1
struct zval {unsigned length;};
struct BufferValue {size_t length=0;};
struct HttpContext {int fd=1;unsigned stream_id=3,end_=0,send_header_=0,send_chunked=0,content_compressed=0;struct Response {zval*zobject;}response;std::unique_ptr<BufferValue>zlib_buffer;unsigned closes=0;void close(HttpContext*);};
inline std::vector<std::string>effects;
inline std::function<void(const std::string&)> mutate;
inline unsigned throw_at,failure;
inline void event(std::string text){effects.push_back(text);if(mutate)mutate(text);if(throw_at==effects.size())throw std::runtime_error("native effect");}
inline zval trailer_property;
inline int swoole_http_response_ce;
inline zval*sw_zend_read_property_ex(int,zval*,int,int){event("property");return &trailer_property;}
inline size_t php_swoole_array_length_safe(zval*z){event("length");return z->length;}
struct Http2Session;
struct FixtureStream {unsigned id=3;HttpContext*ctx;
 bool send_header(const BufferValue*body,bool end){event("header:"+std::to_string(end)+":"+std::to_string(body->length)+":"+std::to_string(ctx->end_));return !(failure&1);}
 bool send_body(const BufferValue*body,bool end,const std::shared_ptr<Http2Session>&){event("body:"+std::to_string(end)+":"+std::to_string(body->length)+":"+std::to_string(ctx->end_));return !(failure&2);}
 bool send_end_stream_data_frame(){event("empty:"+std::to_string(ctx->end_));return !(failure&4);}
 bool send_trailer(){event("trailer:"+std::to_string(trailer_property.length)+":"+std::to_string(ctx->end_));return !(failure&8);}
};
struct Http2Session {bool shutting_down=false;std::map<unsigned,std::shared_ptr<FixtureStream>>streams;
 std::shared_ptr<FixtureStream>get_stream(unsigned id){event("get:"+std::to_string(id));return streams.at(id);}
 bool remove_stream(unsigned id){event("remove:"+std::to_string(id));return streams.erase(id)>0;}
};
inline std::map<int,std::shared_ptr<Http2Session>>http2_sessions;
inline void HttpContext::close(HttpContext*){event("close:"+std::to_string(end_));++closes;}
'''
    driver = r'''
struct Result {std::vector<std::string>events;bool returned=false,shutdown=false;unsigned end=0,header=0,chunked=0,compressed=0,closes=0,trailer=0;size_t streams=0,body=0,zlib=0;std::string exception;};
template<bool rust>Result execute(bool sent,bool chunked,unsigned body_len,bool compressed,unsigned trailer,bool shutdown,unsigned failure_bits,unsigned change,unsigned throw_index){
 effects.clear();throw_at=throw_index;failure=failure_bits;HttpContext ctx;zval object;ctx.response.zobject=&object;ctx.send_header_=sent;ctx.send_chunked=chunked;ctx.content_compressed=compressed;ctx.zlib_buffer=std::make_unique<BufferValue>();ctx.zlib_buffer->length=2;trailer_property.length=trailer;BufferValue data{body_len};auto session=std::make_shared<Http2Session>();session->shutting_down=shutdown;auto stream=std::make_shared<FixtureStream>();stream->ctx=&ctx;session->streams[3]=stream;http2_sessions[1]=session;
 bool changed_header=false;mutate=[&](const std::string&what){
 if((change&1)&&what=="length"){ctx.send_header_=!ctx.send_header_;ctx.send_chunked=!ctx.send_chunked;}
 if((change&2)&&what.rfind("header:",0)==0&&!changed_header){changed_header=true;ctx.content_compressed=!ctx.content_compressed;data.length=0;trailer_property.length=trailer_property.length?0:2;}
 if((change&4)&&(what.rfind("body:",0)==0||what.rfind("empty:",0)==0)){session->shutting_down=!session->shutting_down;trailer_property.length=0;ctx.end_=7;}
 if((change&8)&&(what.rfind("close:",0)==0||what.rfind("remove:",0)==0)){session->streams.clear();session->shutting_down=!session->shutting_down;ctx.end_=9;}
 };Result r;
 try{r.returned=rust?::rust::respond(&ctx,&data) : ::native::respond(&ctx,&data);}catch(const std::runtime_error&e){r.exception=e.what();}
 r.events=effects;r.end=ctx.end_;r.header=ctx.send_header_;r.chunked=ctx.send_chunked;r.compressed=ctx.content_compressed;r.closes=ctx.closes;r.streams=session->streams.size();r.shutdown=session->shutting_down;r.trailer=trailer_property.length;r.body=data.length;r.zlib=ctx.zlib_buffer->length;mutate={};http2_sessions.clear();return r;
}
static bool same(const Result&a,const Result&b){return a.events==b.events&&a.returned==b.returned&&a.end==b.end&&a.header==b.header&&a.chunked==b.chunked&&a.compressed==b.compressed&&a.closes==b.closes&&a.streams==b.streams&&a.shutdown==b.shutdown&&a.trailer==b.trailer&&a.body==b.body&&a.zlib==b.zlib&&a.exception==b.exception;}
int main(){size_t count=0;
for(bool sent:{false,true})for(bool chunked:{false,true})for(unsigned length:{0u,1u,7u})for(bool compressed:{false,true})for(unsigned trailer:{0u,1u,3u})for(bool shutdown:{false,true})for(unsigned failure=0;failure<16;++failure)for(unsigned change=0;change<16;++change){
 auto a=execute<false>(sent,chunked,length,compressed,trailer,shutdown,failure,change,0);auto b=execute<true>(sent,chunked,length,compressed,trailer,shutdown,failure,change,0);if(!same(a,b)){std::cerr<<"response lifecycle mismatch sent="<<sent<<" chunked="<<chunked<<" length="<<length<<" compressed="<<compressed<<" trailer="<<trailer<<" shutdown="<<shutdown<<" failure="<<failure<<" mutation="<<change<<"\n";return 1;}++count;
}
for(unsigned failure=0;failure<16;++failure)for(unsigned change=0;change<16;++change)for(unsigned throw_index=1;throw_index<15;++throw_index){auto a=execute<false>(false,true,0,false,1,true,failure,change,throw_index);auto b=execute<true>(false,true,0,false,1,true,failure,change,throw_index);if(!same(a,b)){std::cerr<<"response lifecycle exception prefix mismatch\n";return 2;}++count;}
std::cout<<"{\"passed\":true,\"response_lifecycle_cases\":"<<count<<",\"compression_enabled\":"<<TEST_COMPRESSION<<"}\n";
}
'''
    work = args.output.parent / (args.output.stem + '-artifacts')
    work.mkdir(parents=True, exist_ok=False)
    commands, report['variants'] = sides.variants(work, 'lifecycle', prelude, pieces, driver, source=args.source, archive=archive, includes=args.include, define='TEST_COMPRESSION')
    report['commands'] += commands
    report['passed'] = len(report['variants']) == 2 and all(r['passed'] for r in report['variants'])
    report['native_sanitizers'] = ['address', 'undefined except alignment']
    report['rust_sanitized'] = False
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'report': str(args.output)}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
