// Included in each variant namespace; all objects and effects remain native.
struct AsyncRequest { virtual ~AsyncRequest(){record("request:delete");} };
struct GethostbynameRequest:AsyncRequest {
 std::string addr;
 GethostbynameRequest(const std::string &name,int domain){record("host-request:"+name+":"+std::to_string(domain));}
};
struct GetaddrinfoRequest:AsyncRequest {
 int error=0;bool ready=false;
 GetaddrinfoRequest(const std::string &name,int family,int kind,int protocol,const std::string &service){record("addr-request:"+name+":"+std::to_string(family)+":"+std::to_string(kind)+":"+std::to_string(protocol)+":"+service);}
 void parse_result(std::vector<std::string> &result){record("addr:parse");if(ready)result.push_back("native-result");}
};
// Upstream's layout: the hybrid's async hands the event to Rust (crates/hooks/src/offload.rs).
struct AsyncEvent {
 size_t task_id=0;
 uint8_t canceled=0;
 int error=0;
 std::shared_ptr<AsyncRequest> data;
 ssize_t retval=0;
 void *pipe_socket=nullptr;
 double timestamp=0;
 void *object=nullptr;
 void (*handler)(AsyncEvent *)=nullptr;
 void (*callback)(AsyncEvent *)=nullptr;
};
static_assert(sizeof(AsyncEvent)==80,"crates/hooks/src/offload.rs mirrors AsyncEvent");
static bool dispatch_ok=true,yield_ok=true,context_canceled=false,queued=false,worker_canceled=false;
static int worker_error=0,request_error=0,last_error=0;
static ssize_t worker_result=1;
static AsyncEvent worker;
static std::function<void()> on_resume;
static std::string number(double value){std::ostringstream out;out<<std::hexfloat<<value;return out.str();}
static int swoole_get_last_error(){record("last:get:"+std::to_string(last_error));return last_error;}
static void swoole_set_last_error(int value){last_error=value;record("last:set:"+std::to_string(value));}
static void complete_worker(){
 if(!queued)return;
 queued=false;
 if(worker.handler && !worker.canceled)worker.handler(&worker);
 if(worker.callback)worker.callback(&worker);
}
struct Coroutine {
 static Coroutine *current;
 static Coroutine *get_current_safe(){record("current");return current;}
 bool yield_ex(double seconds){record("yield-ex:"+number(seconds));if(yield_ok)complete_worker();return yield_ok;}
 void yield(){record("yield");complete_worker();}
 bool is_canceled(){record("canceled:"+std::to_string(context_canceled));return context_canceled;}
 void resume(){record("resume");if(on_resume)on_resume();}
};
Coroutine *Coroutine::current=nullptr;
namespace async {
using Handler=void(*)(AsyncEvent *);
static AsyncEvent *dispatch(const AsyncEvent *event){
 record("dispatch:"+std::to_string(event->handler!=nullptr)+":"+std::to_string(event->callback!=nullptr));
 if(!dispatch_ok)return nullptr;
 worker=*event;worker.canceled=worker_canceled;worker.error=worker_error;worker.retval=worker_result;queued=true;
 return &worker;
}
static void handler_gethostbyname(AsyncEvent *event){
 record("handler:host");auto *request=static_cast<GethostbynameRequest*>(event->data.get());request->addr="127.0.0.1";event->retval=worker_result;event->error=worker_error;
}
static void handler_getaddrinfo(AsyncEvent *event){
 record("handler:addr");auto *request=static_cast<GetaddrinfoRequest*>(event->data.get());request->ready=true;request->error=request_error;event->retval=worker_result;event->error=worker_error;
}
}
namespace coroutine {
struct System {
 static int sleep(double seconds);
 static std::vector<std::string> getaddrinfo(const std::string &,int,int,int,const std::string &,double);
};
bool async(async::Handler,AsyncEvent &,double);
bool async(const std::function<void()> &);
bool async_job(void (*)(void *),void *);
#include ACTUAL_BODIES
}
static void typed_handler(AsyncEvent *event){record("handler:typed");event->error=worker_error;event->retval=worker_result;}
