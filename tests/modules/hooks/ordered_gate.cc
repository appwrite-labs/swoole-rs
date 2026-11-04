#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>
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
#define SW_LOOP_N(n) for(int loop_index=0;loop_index<(n);++loop_index)
#define FIXTURE(NS) \
namespace NS { \
struct Coroutine { static Coroutine *current; static Coroutine *get_current(){ record("co:current");return current; } std::function<void()> on_resume;void resume(){record("event:resume");if(on_resume)on_resume();} }; \
Coroutine *Coroutine::current=nullptr; \
static struct { void *reactor; } SwooleTG={}; \
struct System {static bool canceled;static int sleep(double s){record("sleep:"+std::to_string(s));return canceled?SW_ERR:SW_OK;}}; \
bool System::canceled=false; \
bool wait_for(const std::function<bool(void)> &); \
struct Reactor {std::vector<std::function<void(void*)>> callbacks;void defer(void(*)(void*),void*){record("poll:defer");}void defer(const std::function<void(void*)> &fn){record("event:defer");callbacks.push_back(fn);}};static int16_t translate_events_to_poll(int);static int translate_events_from_poll(int16_t); \
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

/* 4..1003 and 1023..1059 moved to Rust: the hook_* families (crates/phpruntime),
   socket_poll_trigger_event (crates/hooks/src/poll.rs), and the coroutine Socket's
   transfers and TimerController (crates/hooks/src/transfer.rs and co_io.rs, ported_*). */
#define CASES(NS) \
static std::vector<std::string> run_##NS(int scenario){using namespace NS;trace.clear();errno=0;Coroutine co;Coroutine::current=&co;SwooleTG.reactor=&co;System::canceled=false; \
 if(scenario<4){SwooleTG.reactor=scenario&1?&co:nullptr;Coroutine::current=scenario&2?&co:nullptr;record("no-coro:"+std::to_string(is_no_coro()));} \
 if(scenario>=1003 && scenario<1023){int k=scenario-1003;int calls=0;bool rv=wait_for([&](){record("callback:"+std::to_string(calls));if(k>=10 && calls==k-10)System::canceled=true;return ++calls>k%10+14;});record("wait-result:"+std::to_string(rv));} \
 if(scenario>=1059 && scenario<1061){int calls=0;try{wait_for([&]()->bool{record("callback:"+std::to_string(calls));if(calls++==scenario-1059+2)throw std::runtime_error("nativecallback");return false;});}catch(const std::runtime_error &){record("callback:exception");}} \
 return trace;}
CASES(original)
CASES(selected)
int main(){
 for(int scenario=0;scenario<1061;++scenario){auto a=run_original(scenario),b=run_selected(scenario);if(a!=b){std::fprintf(stderr,"mismatch scenario %d\n",scenario);for(auto &s:a)std::fprintf(stderr,"original %s\n",s.c_str());for(auto &s:b)std::fprintf(stderr,"selected %s\n",s.c_str());return 1;}}
 std::puts("26 actual pinned/selected ordered native scenarios passed (facade execution, no real runtime claim)");
}
