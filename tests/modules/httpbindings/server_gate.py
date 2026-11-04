#!/usr/bin/env python3
"""Actual coroutine-server handler routing and accept loop.

The hybrid's accept loop is Rust (swr_v1_httpbindings_accept_loop) over the hybrid
tree's own C++ effects (swr_hsc_*). The Zend-side effects both sides share stand-ins
for: errCode/errMsg (set-error) and the E_WARNING (a warning, then the server stops)."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from source import definition, rust_declarations
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'http'))
from gate import PIN
sys.path.insert(0,str(Path(__file__).resolve().parents[3]))
from tools.build_hybrid import hybrid_tree
from tools.harness import ROOT,run_isolated,write_report

# The hybrid tree's C++ the Rust loop calls back into.
EFFECTS=['static SocketImpl *sock_of(void *sock)','int swr_hsc_error(void *sock, const char **message)',
         'void swr_hsc_accept_errors(swr_v1_httpbindings_accept_errors *errors)',
         'int64_t swr_hsc_accept(void *sock, void *accept, bool *connected, bool *bailed)','void swr_hsc_sleep(void)']
# An effect that exits (Exit) ends the loop there: later callbacks do nothing and the
# exit resumes in call_accept, as upstream's frame would never return.
CANDIDATE_HOST=r"""using SocketImpl=AcceptSocket;
struct HttpServerAccept{int cache=0;Callback callback;};
EFFECTS
struct Host{AcceptSocket*sock;AcceptServer*hs;HttpServerAccept accept;bool exited=false;};
template<class R,class F>R effect(void*data,R otherwise,F f){auto*h=static_cast<Host*>(data);if(h->exited)return otherwise;try{return f(*h);}catch(const Exit&){h->exited=true;return otherwise;}}
void call_accept(AcceptSocket*sock,AcceptServer*hs){
 Host h{sock,hs};swr_v1_httpbindings_accept_errors errors{};candidate_server::swr_hsc_accept_errors(&errors);
 swr_v1_httpbindings_accept_host host{&h,
  [](void*d){return effect(d,false,[](Host&h){return h.hs->running;});},
  [](void*d,bool*connected){return effect(d,int64_t(0),[&](Host&h){bool bailed=false;return candidate_server::swr_hsc_accept(h.sock,&h.accept,connected,&bailed);});},
  [](void*d,const char**message){*message="";return effect(d,0,[&](Host&h){return int32_t(candidate_server::swr_hsc_error(h.sock,message));});},
  [](void*d){effect(d,0,[](Host&){candidate_server::swr_hsc_sleep();return 0;});},
  [](void*d){effect(d,0,[](Host&h){http_server_coro_set_error(nullptr,h.sock);return 0;});},
  [](void*d,swr_v1_httpbindings_text text){effect(d,0,[&](Host&h){oracle->event("warning:"+std::string((const char*)text.data,text.length));h.hs->running=false;return 0;});}};
 int32_t result=swr_v1_httpbindings_accept_loop(&host,&errors);
 if(h.exited)throw Exit{};
 if(!result)RETURN_FALSE;
}"""


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',type=Path,required=True)
    parser.add_argument('--archive',type=Path,required=True,help='Rust archive of the hybrid build under test')
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args();local=Path(__file__).resolve().parent
    report={'pin':PIN,'scope':__doc__,'limitations':'Native Zend/socket/callback facades and bounded single-iteration accept loop; not actual Zend GC/bailout/coroutine IO/PHP or complete binding parity. Real native container order and callback identity retained.', 'native_sanitizers':True,'rust_instrumented':False,'passed':False}
    with tempfile.TemporaryDirectory(prefix='swr-httpbindings-server-') as temp:
        tree=Path(temp)
        # Upstream side from the pin, candidate side from the hybrid build's fully patched tree.
        hybrid_source,hybrid_flags=hybrid_tree(args.archive)
        names=['ext-src/swoole_http_server_coro.cc','include/swoole.h']
        original={n:subprocess.check_output(['git','-C',str(args.source),'show',PIN+':'+n]).decode() for n in names}
        selected={n:(hybrid_source/n).read_text() for n in names}
        for namespace,texts in [('upstream_server',original),('candidate_server',selected)]:
            helper=definition(texts[names[1]],'static inline unsigned int swoole_str_istarts_with(')
            helper=rust_declarations(helper,hybrid_flags)+helper
            handler=definition(texts[names[0]],'zend::Callable *get_handler(').replace('zend::Callable *get_handler(','zend::Callable *HandlerServer::get_handler(',1).replace('const HttpContext *ctx','const RouteContext *ctx')
            if namespace=='upstream_server':
                start=definition(texts[names[0]],'static PHP_METHOD(swoole_http_server_coro, start)')
                loop=definition(start,'    while (hs->running)')
                accept='void call_accept(AcceptSocket*sock,AcceptServer*hs){int fci_cache=0;Callback zcallback;\n'+loop+'\n}'
            else:
                accept=CANDIDATE_HOST.replace('EFFECTS','\n'.join(definition(texts[names[0]],signature) for signature in EFFECTS))
            p=tree/(namespace+'.cc');p.write_text('#include "server_oracle.h"\n'+helper+'\nnamespace '+namespace+'{\n'+handler+'\n'+accept+'\n}\n')
        binary=tree/'gate';cmd=['c++','-std=c++14','-O1','-g','-fsanitize=address,undefined','-fno-sanitize-recover=all','-I'+str(local),'-I'+str(ROOT/'native/httpbindings'),*hybrid_flags,str(tree/'upstream_server.cc'),str(tree/'candidate_server.cc'),str(local/'server_gate.cc'),str(args.archive.resolve()),'-lm','-o',str(binary)]
        if sys.platform.startswith('linux'):cmd+=['-ldl','-lpthread','-lrt','-lutil']
        report['compile_command']=cmd;r=run_isolated(cmd,timeout=60);report['compile']={'returncode':r.returncode,'stdout':r.stdout,'stderr':r.stderr}
        if not r.returncode:
            r=run_isolated([str(binary)],timeout=90);report['run']={'returncode':r.returncode,'stdout':r.stdout,'stderr':r.stderr}
            if not r.returncode:report['outcome']=json.loads(r.stdout.splitlines()[-1]);report['passed']=True
        write_report(args.output,report)
        print(json.dumps(report.get('outcome',report.get('run',report['compile']))))
        return 0 if report['passed'] else 1


if __name__=='__main__':sys.exit(main())
