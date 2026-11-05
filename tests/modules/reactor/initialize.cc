#include <cassert>
#include <new>
#include <cstdint>
#include <cstdlib>
#include <ctime>
#include <functional>
#include <iostream>
#include <map>
#include <string>
#include <vector>
#include "swoole_rs_reactor.h"
#define SW_GLOBAL_HOOK_ON_REACTOR_CREATE 1
#define SW_MALLOC_TRIM_INTERVAL 10
#define SW_MALLOC_TRIM_PAD 0
static std::vector<int> trace;
static bool hook_enabled,first_main,second_main;
static unsigned main_observations;
struct { bool enable_kqueue; } SwooleG;
static bool swoole_isset_hook(int) {trace.push_back(20);return hook_enabled;}
static bool swoole_is_main_thread() {trace.push_back(30);return main_observations++==0?first_main:second_main;}
static void swoole_signal_dispatch() {assert(false && "Not executed by constructor profile");}
static void malloc_trim(int) {assert(false && "Not executed by constructor profile");}
#define swoole_log_as(origin, ...) __VA_ARGS__
#define swoole_sys_warning(...) assert(false && "Not executed by constructor profile")
namespace swoole {
class Reactor;
static void swoole_call_hook(int,Reactor*) {trace.push_back(21);}
struct CallbackManager {void execute() {assert(false);}};
struct Task {void (*callback)(void*)=nullptr;void *data=nullptr;};
using DeferCallback=Task;
// The hybrid's backend sets ready_ when it opens; upstream's ready() reads it back.
struct ReactorImpl {bool ready(){return true;}};
static ReactorImpl native_backend;
#include "initialize-class.inc"
static ReactorImpl *make_reactor_epoll(Reactor*,int n){trace.push_back(100+n);return &native_backend;}
static ReactorImpl *make_reactor_kqueue(Reactor*,int n){trace.push_back(200+n);return &native_backend;}
static ReactorImpl *make_reactor_poll(Reactor*,int n){trace.push_back(300+n);return &native_backend;}
// The hybrid opens every backend through the Rust state; kind is the factory.
static int swr_v1_reactor_state_open(Reactor *r,int kind,int n){trace.push_back(kind*100+n);r->ready_=true;return 0;}
#include "initialize-method.inc"
}
int main(){
    unsigned cases=0,checks=0;
    for(int requested=0;requested<4;++requested)for(int enable=0;enable<2;++enable)
    for(int hook=0;hook<2;++hook)for(int mains=0;mains<4;++mains){
        SwooleG.enable_kqueue=enable;hook_enabled=hook;first_main=mains&1;second_main=mains&2;
        trace.clear();main_observations=0;
        swoole::Reactor r(7,static_cast<swoole::Reactor::Type>(requested));
        auto check=[&](bool v){assert(v);++checks;};
        int chosen=requested;
        if(chosen==0){
#ifdef HAVE_EPOLL
            chosen=1;
#else
            chosen=3;
#ifdef HAVE_KQUEUE
            if(enable)chosen=2;
#endif
#endif
        }
        check(r.type_==chosen);check(r.ready_);check(r.timeout_msec==-1);
        check(r.write==swoole::Reactor::_write && r.writev==swoole::Reactor::_writev && r.close==swoole::Reactor::_close);
        check(r.default_write_handler==swoole::Reactor::_writable_callback);
        std::vector<int> expected;
        switch(chosen){
#ifdef HAVE_EPOLL
        case 1:expected.push_back(107);break;
#endif
#ifdef HAVE_KQUEUE
        case 2:expected.push_back(207);break;
#endif
        default:expected.push_back(307);break;
        }
        expected.push_back(20);if(hook)expected.push_back(21);
        expected.insert(expected.end(),{41,51,42,30});if(first_main)expected.push_back(43);
        expected.push_back(44);
#ifdef SW_USE_MALLOC_TRIM
        expected.push_back(30);if(second_main)expected.push_back(45);
#endif
        expected.push_back(1049);check(trace==expected);
        check(r.ends.count(1)==1 && r.ends.count(2)==1 && r.ends.count(4)==1);
        check(r.exits.count(1)==1 && r.exits.count(999)==1);
        check(r.ends.count(3)==static_cast<unsigned>(first_main));
        check(r.exits.at(1)(&r,r.event_num));check(r.exits.at(999)(&r,r.event_num));
        ++cases;
    }
    std::cout<<"{\"cases\":"<<cases<<",\"checks\":"<<checks<<",\"constructor_only\":true,\"fault_or_lifetime_probes\":false}\n";
}
