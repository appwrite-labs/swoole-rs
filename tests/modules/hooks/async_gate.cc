#include <cassert>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <functional>
#include <limits>
#include <memory>
#include <sstream>
#include <string>
#include <vector>
#include <sys/socket.h>
#include "swoole_rs_hooks.h"
#include "swoole_rs_corebase_resolver.h"
static std::vector<std::string> trace;
static void record(std::string text){trace.push_back(text);}
#define SW_USE_IOURING 0
#define SW_TIMER_MIN_SEC .001
#define SW_ERROR_AIO_TIMEOUT 1779
#define SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT 1780
#define SW_OK 0
#define SW_ERR -1
#define ACTUAL_BODIES "original-async.inc"
namespace original {
#include "async_fixture.h"
}
#undef ACTUAL_BODIES
#define ACTUAL_BODIES "selected-async.inc"
namespace selected {
#include "async_fixture.h"
}
#define CASES(NS) \
static std::vector<std::string> run_##NS(int scenario){using namespace NS;trace.clear();errno=E2BIG;dispatch_ok=true;yield_ok=true;context_canceled=false;queued=false;worker_canceled=false;worker_error=EIO;request_error=0;last_error=ECANCELED;worker_result=1;on_resume=nullptr;Coroutine co;Coroutine::current=&co;worker=AsyncEvent{}; \
 if(scenario<96){int k=scenario;dispatch_ok=k&1;yield_ok=k&2;worker_canceled=k&4;int errors[]={0,ETIMEDOUT,EIO};ssize_t results[]={-1,0,1,123};worker_error=errors[(k/8)%3];worker_result=results[k/24];AsyncEvent event;bool result=coroutine::async(typed_handler,event,.25);record("result:"+std::to_string(result));record("caller-canceled:"+std::to_string(event.canceled));record("caller-retval:"+std::to_string(event.retval));record("caller-error:"+std::to_string(event.error));record("worker-canceled:"+std::to_string(worker.canceled));record("errno:"+std::to_string(errno));if(queued){record("late-completion");complete_worker();}} \
 if(scenario>=96 && scenario<102){int k=scenario-96;dispatch_ok=k&1;int errors[]={0,EIO,EINVAL};worker_error=errors[(k/2)%3];bool result=coroutine::async([&](){record("lambda:execute");errno=worker_error;});record("result:"+std::to_string(result));record("errno:"+std::to_string(errno));} \
 if(scenario>=102 && scenario<138){int k=scenario-102;double seconds[]={-std::numeric_limits<double>::infinity(),-1,-0.0,0,.0001,.001,1,std::numeric_limits<double>::quiet_NaN(),std::numeric_limits<double>::infinity()};context_canceled=(k/9)%2;yield_ok=k/18;record("sleep:"+std::to_string(coroutine::System::sleep(seconds[k%9])));record("errno:"+std::to_string(errno));} \
 if(scenario>=138 && scenario<234){int k=scenario-138;ssize_t results[]={-1,0,1};int errors[]={0,SW_ERROR_AIO_TIMEOUT,EIO,EINVAL};worker_result=results[k%3];worker_error=errors[(k/3)%4];dispatch_ok=(k/12)%2;yield_ok=(k/24)%2;auto value=coroutine::gethostbyname_impl_with_async("private.test",k/48?AF_INET6:AF_INET,.5);record("host:"+value);record("errno:"+std::to_string(errno));record("last:"+std::to_string(last_error));if(queued){record("late-completion");complete_worker();}} \
 if(scenario>=234 && scenario<426){int k=scenario-234;ssize_t results[]={-1,0,1};int errors[]={0,SW_ERROR_AIO_TIMEOUT,EIO,EINVAL};worker_result=results[k%3];worker_error=errors[(k/3)%4];dispatch_ok=(k/12)%2;yield_ok=(k/24)%2;request_error=(k/48)%2;auto values=coroutine::System::getaddrinfo("private.test",k/96?AF_INET6:AF_INET,SOCK_STREAM,0,"443",.5);record("addresses:"+std::to_string(values.size()));if(!values.empty())record(values[0]);record("errno:"+std::to_string(errno));record("last:"+std::to_string(last_error));if(queued){record("late-completion");complete_worker();}} \
 worker=AsyncEvent{};return trace;}
CASES(original)
CASES(selected)
int main(){
 for(int scenario=0;scenario<426;++scenario){auto a=run_original(scenario),b=run_selected(scenario);if(a!=b){std::fprintf(stderr,"async mismatch %d\n",scenario);for(auto &line:a)std::fprintf(stderr,"original %s\n",line.c_str());for(auto &line:b)std::fprintf(stderr,"selected %s\n",line.c_str());return 1;}}
 std::puts("426 pinned/hybrid async/sleep/DNS native-facade scenarios passed");
}
