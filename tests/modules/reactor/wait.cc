#include <cassert>
#include <cstddef>
#include <cstdint>
#include <iostream>
#include <poll.h>
#include <string>
#include <vector>
#ifdef __linux__
#include <sys/epoll.h>
#endif
#ifdef __APPLE__
#include <sys/event.h>
#endif
#include "swoole_rs_reactor.h"
constexpr int SW_OK=0,SW_ERR=-1,SW_EVENT_READ=512,SW_EVENT_WRITE=1024,SW_EVENT_ONCE=4096;
constexpr int SW_TRACE_EVENT=0,SW_LOG_WARNING=1,SW_ERROR_UNREGISTERED_SIGNAL=2,SW_ERROR_EVENT_UNKNOWN_DATA=3;
#ifndef __APPLE__
constexpr int EVFILT_READ=-1,EVFILT_WRITE=-2,EVFILT_SIGNAL=-6;
#endif
#define SW_UNREGISTERED_SIGNAL_FMT "unused"
#ifndef SIG_IGN
#define SIG_IGN ((void (*)(int))1)
#endif
#define sw_unlikely(x) (x)
#define sw_likely(x) (x)
#define SW_REACTOR_CONTINUE if (reactor_->once) { break; } else { continue; }
#define swoole_sys_warning(...) assert(false && "unexpected error in positive wait profile")
#define swoole_error_log(...) assert(false && "unexpected error in positive wait profile")
#define swoole_print_backtrace_on_error() assert(false && "unexpected backtrace in positive wait profile")
#define swoole_trace(...) do {} while(false)
#define swoole_trace_log(...) do {} while(false)
// The leading fields of the socket record (swoole_rs_socket.h), which Rust's loop reads in place.
struct Socket { int fd=7,fd_type=2,socket_type=0,events=0; bool nodelay=false,nobufs=false,removed=false; uint8_t flags[14]{}; bool event_hup=false; uint8_t rest[262]{}; };
static_assert(offsetof(Socket,events)==12&&offsetof(Socket,removed)==18&&offsetof(Socket,event_hup)==33,"socket record offsets");
// swoole::Event's layout: the selected bodies have Rust fill it.
struct Event { int fd=0; int16_t reactor_id=0; int type=0; Socket *socket=nullptr; };
struct Signal { bool activated=false; void (*handler)(int)=nullptr; int signo=0; };
struct Reactor;
using ReactorHandler=int (*)(Reactor*,Event*);
static std::vector<std::string> effects;
static int kernel_result=1, readiness=1, cycles=0;
static bool initial_running=true, begin_stops=false, end_sets_once=false;
struct Reactor {
    int id=1,timeout_msec=17;
    uint32_t max_event_num=1;
    bool running=true,once=false;
    void *defer_tasks=nullptr;
    Socket *socket;
    void before_wait() { effects.emplace_back("before"); running=initial_running; }
    void execute_begin_callback() { effects.emplace_back("begin"); if(begin_stops)running=false; }
    void execute_end_callbacks(bool timeout) { effects.emplace_back(timeout?"end-timeout":"end"); if(++cycles==2)running=false; if(end_sets_once)once=true; }
    int get_timeout_msec() { return timeout_msec; }
    bool catch_error() { assert(false && "error probes excluded");return false; }
    bool exists(int fd) { assert(fd==socket->fd);return true; }
    Socket *get_socket(int fd) { assert(fd==socket->fd);return socket; }
    static int read(Reactor*,Event*) { effects.emplace_back("read");return 0; }
    static int write(Reactor*,Event*) { effects.emplace_back("write");return 0; }
    ReactorHandler get_handler(int,int type) { return type==SW_EVENT_READ?read:write; }
    ReactorHandler get_error_handler(int) { assert(false && "errors excluded");return read; }
    // The selected bodies dispatch through the Rust tables, filled like get_handler above.
    swr_v1_reactor_handlers *handlers=[] {
        auto h=swr_v1_reactor_handlers_new();
        swr_v1_reactor_handlers_set(h,2,SW_EVENT_READ,reinterpret_cast<swr_v1_reactor_handler>(read));
        swr_v1_reactor_handlers_set(h,2,SW_EVENT_WRITE,reinterpret_cast<swr_v1_reactor_handler>(write));
        return h;
    }();
    ~Reactor() { swr_v1_reactor_handlers_free(handlers); }
    const swr_v1_reactor_handlers *get_handlers() const { return handlers; }
    void _del(Socket *s) { effects.emplace_back("delete");s->removed=true; }
};
#ifdef __APPLE__
using NativeKevent=struct kevent;
#endif
static Socket *current;
static int native_poll(pollfd *events,int count,int timeout) {
    assert(count==1 && timeout==17);effects.emplace_back("wait");
    events[0].revents=(readiness&1?POLLIN:0)|(readiness&2?POLLOUT:0);
    return kernel_result;
}
#ifdef __linux__
static int native_epoll_wait(int,epoll_event *events,int count,int timeout) {
    assert(count==1&&timeout==17);effects.emplace_back("wait");
    events[0].data.ptr=current;events[0].events=(readiness&1?EPOLLIN:0)|(readiness&2?EPOLLOUT:0);
    return kernel_result;
}
#endif
#ifdef __APPLE__
static int native_kevent(int,const NativeKevent*,int,NativeKevent *events,int count,const timespec *timeout) {
    assert(count==1 && timeout && timeout->tv_sec==0 && timeout->tv_nsec==17000000);effects.emplace_back("wait");
    events[0].udata=current;events[0].filter=readiness==2?EVFILT_WRITE:EVFILT_READ;
    return kernel_result;
}
#endif
struct WaitBase {
    Reactor *reactor_; int epfd_=3,event_max_=1;
    pollfd poll_events[1]{};
    Socket *poll_sockets[1]{};
#ifdef __APPLE__
    NativeKevent kqueue_events[1]{};
#endif
#ifdef __linux__
    epoll_event epoll_events[1]{};
#endif
    explicit WaitBase(Reactor *r):reactor_(r) {}
    int set_events() { poll_events[0].fd=current->fd;poll_sockets[0]=current;return 1; }
    int del(Socket *s) { reactor_->_del(s);return 0; }

};
#include "wait.inc"
template<class Backend> std::vector<std::string> run(int ready,int result,bool once,bool start,bool begin_stop,bool end_once,bool socket_once) {
    effects.clear();cycles=0;kernel_result=result;readiness=ready;initial_running=start;begin_stops=begin_stop;end_sets_once=end_once;
    Socket socket;socket.events=socket_once?SW_EVENT_ONCE:0;current=&socket;Reactor reactor;reactor.socket=&socket;reactor.once=once;
    Backend backend(&reactor);assert(backend.wait()==0);
    assert(!effects.empty() && effects.front()=="before");
    if(!start)assert(effects==std::vector<std::string>{"before"});
    else { assert(effects[1]=="begin" && effects[2]=="wait");assert(effects.back()==(result==0?"end-timeout":"end")); }
    return effects;
}
int main() {
    unsigned cases=0;
    // Stack tokens remain live throughout; ONCE unregisters without destruction.
    // No callback reentry, failure, allocation or lifetime probes.
    for(int ready:{1,2,3})for(int result:{0,1})for(bool once:{false,true})for(int mode:{0,1,2,3})for(bool socket_once:{false,true}) {
        bool start=mode!=1, begin_stop=mode==2, end_once=mode==3;
        auto p=run<PollOriginal>(ready,result,once,start,begin_stop,end_once,socket_once);
        assert(p==run<PollSelected>(ready,result,once,start,begin_stop,end_once,socket_once));++cases;
#ifdef __APPLE__
        auto k=run<KqueueOriginal>(ready,result,once,start,begin_stop,end_once,socket_once);
        assert(k==run<KqueueSelected>(ready,result,once,start,begin_stop,end_once,socket_once));++cases;
#endif
#ifdef __linux__
        auto e=run<EpollOriginal>(ready,result,once,start,begin_stop,end_once,socket_once);
        assert(e==run<EpollSelected>(ready,result,once,start,begin_stop,end_once,socket_once));++cases;
#endif
    }
    std::cout<<"{\"positive_wait_scenarios\":"<<cases<<",\"whole_wait_bodies\":true,\"native_effect_facades\":true,\"faults_or_lifetime_probes\":false}\n";
}
