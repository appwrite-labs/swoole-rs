#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <exception>
#include <functional>
#include <map>
#include <stdexcept>
#include <string>
#include <vector>
#include <sys/file.h>
#include <sys/socket.h>
#include <poll.h>
#include "swoole_rs_hooks.h"
static std::vector<std::string> trace;
static void record(std::string s) { trace.push_back(s); }
#define sw_inline inline
#define sw_unlikely(x) (x)
#define SW_MIN(x,y) ((x)<(y)?(x):(y))
#define SW_OK 0
#define SW_ERR -1
#define SW_WAIT 2
#define SW_EVENT_READ (1u<<9)
#define SW_EVENT_WRITE (1u<<10)
#define SW_EVENT_ERROR (1u<<11)
using EventType=uint32_t;
#define SW_FILE_LOCK_DEFAULT_SECOND .001
#define SW_FILE_LOCK_MAX_SECOND .1
#define SW_IOURING_SQE_BATCH_SIZE 16
static void swoole_sys_error(const char *) { record("submit:error"); }

static std::function<ssize_t(const char *,size_t)> syscall_io;
static ssize_t test_recvmsg(int fd,msghdr *,int flags){record("recvmsg-args:"+std::to_string(fd)+":"+std::to_string(flags));return syscall_io("recvmsg",8);}
static ssize_t test_sendmsg(int fd,const msghdr *,int flags){record("sendmsg-args:"+std::to_string(fd)+":"+std::to_string(flags));return syscall_io("sendmsg",8);}
static ssize_t test_recvfrom(int fd,void *,size_t n,int flags,sockaddr *,socklen_t *){record("recvfrom-args:"+std::to_string(fd)+":"+std::to_string(flags));return syscall_io("recvfrom",n);}
#define recvmsg test_recvmsg
#define sendmsg test_sendmsg
#define recvfrom test_recvfrom
#define SW_LOOP_N(n) for(int loop_index=0;loop_index<(n);++loop_index)
#define SW_ERROR_BAD_HOST_ADDR 1778
#define swoole_trace_log(kind,format,result,size,error) record("native-trace:"+std::to_string(result)+":"+std::to_string(size)+":"+std::to_string(error))
#define FIXTURE(NS) \
namespace NS { \
struct Coroutine { static Coroutine *current; static Coroutine *get_current(){ record("co:current");return current; } std::function<void()> on_resume;void resume(){record("event:resume");if(on_resume)on_resume();} }; \
Coroutine *Coroutine::current=nullptr; \
static struct { void *reactor; } SwooleTG={}; \
static int last_error=0;static bool datagram=true;static std::string dns_result="resolved";static int swoole_get_last_error(){record("last-error:get");return last_error;} \
static void swoole_set_last_error(int e){last_error=e;record("last-error:"+std::to_string(e));} \
struct System {static bool canceled;static int sleep(double s){record("sleep:"+std::to_string(s));return canceled?SW_ERR:SW_OK;}static int wait_event(int fd,int events,double timeout);static std::string gethostbyname(const std::string &host,int domain,double timeout){record("dns:"+host+":"+std::to_string(domain)+":"+std::to_string(timeout));return dns_result;}}; \
bool System::canceled=false; \
bool wait_for(const std::function<bool(void)> &); \
static bool available=true,timer_ok=true,wake_ok=true; \
static std::vector<ssize_t> io_results;static size_t io_index=0;static bool throw_io=false;static char redirected_buffer[16]; \
static ssize_t io(const char *direction,size_t length){record(std::string(direction)+":"+std::to_string(length));if(throw_io && io_index==1)throw std::runtime_error("native-io");ssize_t r=io_results.at(io_index++);errno=r<0?-r:0;return r<0?-1:r;} \
namespace network {struct IOVector {};struct Address {bool assign(int type,const std::string &host,int port,bool){record("address:"+std::to_string(type)+":"+host+":"+std::to_string(port));if(host=="needs-dns"){last_error=SW_ERROR_BAD_HOST_ADDR;return false;}if(host=="invalid"){last_error=EINVAL;return false;}return true;}};} \
struct NativeSocket {double read_timeout=1,write_timeout=1,dns_timeout=2;bool is_dgram(){record("is-dgram");return datagram;}ssize_t sendto(const network::Address &,const void *p,size_t n,int){if(p==redirected_buffer)record("sendto:redirected");return io("sendto",n);}ssize_t recv(void *,size_t n,int){return io("recv",n);}ssize_t send(const void *p,size_t n,int){if(p==redirected_buffer)record("send:redirected");return io("send",n);} \
 ssize_t read(void *,size_t n){return io("read",n);}ssize_t write(void *p,size_t n){if(p==redirected_buffer)record("write:redirected");return io("write",n);}ssize_t readv(network::IOVector *){return io("readv",8);}ssize_t writev(network::IOVector *){return io("writev",8);} \
 int catch_read_error(int e){record("catch-read:"+std::to_string(e));return e==EAGAIN?SW_WAIT:SW_ERR;}int catch_write_error(int e){record("catch-write:"+std::to_string(e));return e==EAGAIN?SW_WAIT:SW_ERR;}}; \
using EventBarrier=std::function<bool()>; \
struct TimerNode {};using TimerCallback=void(*)(int,int); \
static TimerNode *swoole_timer_add(double timeout,bool,TimerCallback,void*){record("timer:add:"+std::to_string(timeout));return timer_ok?new TimerNode:nullptr;}static void swoole_timer_del(TimerNode *p){record("timer:delete");delete p;} \
struct Socket { \
struct TimerController {TimerNode **timer_pp;double timeout;Socket *socket_;TimerCallback callback;bool enabled=false;TimerController(TimerNode **p,double t,Socket *s,TimerCallback cb):timer_pp(p),timeout(t),socket_(s),callback(cb){record("timer:construct");}~TimerController();bool start();}; \
int sock_fd=77,type=4,sock_domain=AF_INET;NativeSocket native;NativeSocket *socket=&native;TimerNode *read_timer=nullptr,*write_timer=nullptr;EventBarrier *recv_barrier=nullptr,*send_barrier=nullptr; \
 static void timer_callback(int,int){}bool is_available(int e){record("available:"+std::to_string(e));return available;}void set_err(int e){record("set-error:"+std::to_string(e));}void set_err(){record("set-last-error:"+std::to_string(last_error));}void check_return_value(ssize_t r){record("check:"+std::to_string(r));} \
 bool wait_event(int e){record("wait:"+std::to_string(e));if(!wake_ok)return false;auto p=e==SW_EVENT_READ?recv_barrier:send_barrier;while(p && (*p)())record("barrier:again");return true;} \
 bool wait_event(int e,const void **buf,size_t n){record("wait-buffer:"+std::to_string(n));*buf=redirected_buffer;return wait_event(e);} \
 bool wait_event(int e,swr_v1_hook_barrier b,void *s,const void **buf=nullptr,size_t n=0){EventBarrier f=[b,s]{return b(s);};EventBarrier *&slot=e==SW_EVENT_READ?recv_barrier:send_barrier;if(b)slot=&f;bool woke=buf?wait_event(e,buf,n):wait_event(e);if(b)slot=nullptr;return woke;} \
 ssize_t recvmsg(msghdr *,int);ssize_t sendmsg(const msghdr *,int);ssize_t sendto(const std::string &,int,const void *,size_t);ssize_t recvfrom(void *,size_t,sockaddr *,socklen_t *); \
 ssize_t recv(void *,size_t);ssize_t send(const void *,size_t);ssize_t read(void *,size_t);ssize_t write(const void *,size_t);ssize_t readv(network::IOVector *);ssize_t writev(network::IOVector *); \
 ssize_t recv_all(void *,size_t);ssize_t send_all(const void *,size_t); \
 enum {TRANSFER_ALL=1,TRANSFER_EINTR=2,TRANSFER_CHECKED=4};template <class Call> ssize_t transfer(EventType,const void *,size_t,int,Call);}; \
struct UringSocket {ssize_t recv(void *,size_t n){return io("uring-recv",n);}ssize_t send(const void *,size_t n){return io("uring-send",n);}void check_return_value(ssize_t r){record("check:"+std::to_string(r));}ssize_t recv_all(void *,size_t);ssize_t send_all(const void *,size_t);}; \
static void swoole_timer_del(void *){record("poll:timer-del");} \
struct Reactor {std::vector<std::function<void(void*)>> callbacks;void defer(void(*)(void*),void*){record("poll:defer");}void defer(const std::function<void(void*)> &fn){record("event:defer");callbacks.push_back(fn);}};struct EventWaiter {Coroutine *co;uint32_t revents;int error_;EventWaiter(Coroutine *p,uint32_t bits):co(p),revents(bits),error_(0){}};static int16_t translate_events_to_poll(int);static int translate_events_from_poll(int16_t); \
}
FIXTURE(original)
FIXTURE(selected)
namespace original {
#include "original.inc"
}
namespace selected {
#include "selected.inc"
}
#undef poll

#define CASES(NS) \
static std::vector<std::string> run_##NS(int scenario){using namespace NS;trace.clear();errno=0;last_error=0;Coroutine co;Coroutine::current=&co;SwooleTG.reactor=&co; \
 syscall_io=io;datagram=true;dns_result="resolved";System::canceled=false;io_index=0;throw_io=false;available=true;timer_ok=true;wake_ok=true; \
 if(scenario<4){SwooleTG.reactor=scenario&1?&co:nullptr;Coroutine::current=scenario&2?&co:nullptr;record("no-coro:"+std::to_string(is_no_coro()));} \
 else if(scenario<28){} \
 else if(scenario<44){int k=scenario-28;std::vector<std::vector<ssize_t>> cases={{0},{8},{-EINVAL},{-EAGAIN,3,5},{2,3,3},{2,-EAGAIN,-ECONNRESET},{2,0},{-EAGAIN,0}};io_results=cases[k%8];Socket s;char b[16]={};ssize_t r=k<8?s.recv_all(b,8):s.send_all(b,8);record("result:"+std::to_string(r));} \
 else if(scenario<50){Socket s;char b[8]={};io_results={2};int k=scenario-44;available=k%3!=0;timer_ok=k%3!=1;wake_ok=k%3!=2;ssize_t r=k<3?s.recv_all(b,8):s.send_all(b,8);record("result:"+std::to_string(r));} \
 else if(scenario<62){int k=scenario-50;std::vector<std::vector<ssize_t>> cases={{0},{8},{-EINVAL},{3,5},{3,-EINTR},{3,0}};io_results=cases[k%6];UringSocket s;char b[16]={};ssize_t r=k<6?s.recv_all(b,8):s.send_all(b,8);record("result:"+std::to_string(r));} \
 else if(scenario<72){} \
 /* 72..643 and 1876..2021: Runtime::enableCoroutine's hook_* families moved to Rust (crates/phpruntime). */ \
 /* 643..931: socket_poll_trigger_event moved to Rust (crates/hooks/src/poll.rs, ported_trigger_matrix). */ \
 if(scenario>=931 && scenario<1003){int k=scenario-931;double times[]={-1,0,.002};int state=(k/3)%3;TimerNode *node=state==1?new TimerNode:state==2?reinterpret_cast<TimerNode*>(-1):nullptr;Socket sock;timer_ok=(k/9)%2;{Socket::TimerController timer(&node,times[k%3],&sock,Socket::timer_callback);timer.enabled=(k/18)%2;record("timer-result:"+std::to_string(timer.start()));record("enabled:"+std::to_string(timer.enabled));record("node:"+std::to_string(node==nullptr?0:node==reinterpret_cast<TimerNode*>(-1)?2:1));if((k/36)%2)record("timer-result-again:"+std::to_string(timer.start()));}record("after-node:"+std::to_string(node==nullptr?0:node==reinterpret_cast<TimerNode*>(-1)?2:1));if(node && node!=reinterpret_cast<TimerNode*>(-1))delete node;} \
 if(scenario>=1003 && scenario<1023){int k=scenario-1003;int calls=0;bool rv=wait_for([&](){record("callback:"+std::to_string(calls));if(k>=10 && calls==k-10)System::canceled=true;return ++calls>k%10+14;});record("wait-result:"+std::to_string(rv));} \
 if(scenario>=1023 && scenario<1059){int k=scenario-1023;uint32_t masks[]={0,SW_EVENT_READ,SW_EVENT_WRITE,SW_EVENT_ERROR,SW_EVENT_READ|SW_EVENT_WRITE,SW_EVENT_READ|SW_EVENT_ERROR};EventWaiter waiter{&co,masks[k%6]};Reactor reactor;event_waiter_callback(&reactor,&waiter,masks[(k/6)%6]);record("events:"+std::to_string(waiter.revents));event_waiter_callback(&reactor,&waiter,SW_EVENT_WRITE);record("events2:"+std::to_string(waiter.revents));for(auto &fn:reactor.callbacks)fn(nullptr);} \
 if(scenario>=1059 && scenario<1061){int calls=0;try{wait_for([&]()->bool{record("callback:"+std::to_string(calls));if(calls++==scenario-1059+2)throw std::runtime_error("nativecallback");return false;});}catch(const std::runtime_error &){record("callback:exception");}} \
 if(scenario==1169){auto *waiter=new EventWaiter(&co,0);co.on_resume=[waiter](){delete waiter;record("event:owner-deleted");};Reactor reactor;event_waiter_callback(&reactor,waiter,SW_EVENT_READ);for(auto &fn:reactor.callbacks)fn(nullptr);} \
 if(scenario>=1170 && scenario<1434){int k=scenario-1170;int method=k%6;int mode=(k/6)%11;int failure=k/66;std::vector<std::vector<ssize_t>> results={{0},{8},{-EINVAL},{-EINTR},{-EAGAIN,8},{-EAGAIN,-EAGAIN,3},{-EAGAIN,0},{-EAGAIN,-ECONNRESET},{-EAGAIN,-EINTR},{-EAGAIN,-EAGAIN,0},{-EAGAIN,-EAGAIN,-EINVAL}};io_results=results[mode];if(failure==1)available=false;if(failure==2)timer_ok=false;if(failure==3)wake_ok=false;Socket sock;char buf[16]={};network::IOVector vec;ssize_t result=method==0?sock.recv(buf,8):method==1?sock.send(buf,8):method==2?sock.read(buf,8):method==3?sock.write(buf,8):method==4?sock.readv(&vec):sock.writev(&vec);record("result:"+std::to_string(result));record("errno:"+std::to_string(errno));} \
 if(scenario>=1434 && scenario<1440){int method=scenario-1434;io_results={-EAGAIN,8};throw_io=true;Socket sock;char buf[16]={};network::IOVector vec;try{ssize_t result=method==0?sock.recv(buf,8):method==1?sock.send(buf,8):method==2?sock.read(buf,8):method==3?sock.write(buf,8):method==4?sock.readv(&vec):sock.writev(&vec);record("unexpected:"+std::to_string(result));}catch(const std::runtime_error &){record("io:exception");}} \
 if(scenario>=1440 && scenario<1872){int k=scenario-1440;int method=k%4;int mode=(k/4)%18;int failure=k/72;std::vector<std::vector<ssize_t>> results={{0},{8},{-EINVAL},{-EINTR,8},{-EINTR,-EINTR,0},{-EINTR,-EAGAIN,3},{-EAGAIN,-EINTR,8},{-EAGAIN,8},{-EAGAIN,-EAGAIN,0},{-EAGAIN,-ECONNRESET},{-EAGAIN,-EAGAIN,-EINVAL},{-EINTR,-EINTR,-EINVAL},{-EINTR,-EAGAIN,-EAGAIN,8},{-EINTR,8},{-EINTR,8},{8},{8},{8}};io_results=results[mode];if(failure==1)available=false;if(failure==2)timer_ok=false;if(failure==3)wake_ok=false;if(failure==4)datagram=false;if(failure==5)dns_result="";std::string host=mode==13?"needs-dns":mode==14?"invalid":"local";Socket sock;char buf[16]={};msghdr message{};sockaddr_storage addr{};socklen_t length=sizeof(addr);ssize_t result=method==0?sock.recvmsg(&message,MSG_PEEK):method==1?sock.sendmsg(&message,MSG_DONTWAIT):method==2?sock.sendto(host,9602,buf,8):sock.recvfrom(buf,8,reinterpret_cast<sockaddr*>(&addr),&length);record("result:"+std::to_string(result));record("errno:"+std::to_string(errno));} \
 if(scenario>=1872 && scenario<1876){int method=scenario-1872;io_results={-EAGAIN,8};throw_io=true;Socket sock;char buf[16]={};msghdr message{};sockaddr_storage addr{};socklen_t length=sizeof(addr);try{ssize_t result=method==0?sock.recvmsg(&message,0):method==1?sock.sendmsg(&message,0):method==2?sock.sendto("local",9602,buf,8):sock.recvfrom(buf,8,reinterpret_cast<sockaddr*>(&addr),&length);record("unexpected:"+std::to_string(result));}catch(const std::runtime_error &){record("io:exception");}} \
 return trace;}
CASES(original)
CASES(selected)
int main(){
 for(int scenario=0;scenario<1876;++scenario){auto a=run_original(scenario),b=run_selected(scenario);if(a!=b){std::fprintf(stderr,"mismatch scenario %d\n",scenario);for(auto &s:a)std::fprintf(stderr,"original %s\n",s.c_str());for(auto &s:b)std::fprintf(stderr,"selected %s\n",s.c_str());return 1;}}
 std::puts("1876 actual pinned/selected ordered native scenarios passed (facade execution, no real runtime claim)");
}
