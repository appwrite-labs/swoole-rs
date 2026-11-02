static int last_error=0,ssl_error=0,shutdown_result=0,shutdown_error=-1,option_mode=0,probe_error=0;
static bool timer_ok=true,wake_ok=true,close_on_wait=false,probe_ok=true,throw_wait=false;
static std::vector<int> connect_results;
static size_t connect_index=0;
static void swoole_set_last_error(int error){last_error=error;record("last-error:"+std::to_string(error));}
static const char *swoole_strerror(int error){record("strerror:"+std::to_string(error));return "native error";}
using EventType=uint32_t;
struct Coroutine {
 long cid;std::function<void()> callback;
 long get_cid(){record("cid:"+std::to_string(cid));return cid;}
 void resume(){record("resume:"+std::to_string(cid));if(callback)callback();}
 static void print_socket_bound_error(int fd,const char *event,long cid){record("bound:"+std::to_string(fd)+":"+event+":"+std::to_string(cid));}
};
struct NativeSocket {
 bool close_wait=false;void *ssl=nullptr;double connect_timeout=1;
 void ssl_shutdown(){record("ssl-shutdown");errno=ssl_error;}
 int get_name(){record("get-name");return -1;}
 int get_option(int,int,int *error){record("get-option:"+std::to_string(*error));if(option_mode==0)*error=0;if(option_mode==1)*error=ECONNREFUSED;if(option_mode==3)*error=0;errno=EIO;return option_mode>=2?-1:0;}
 bool check_liveness(){record("probe");errno=probe_error;return probe_ok;}
};
struct Socket {
 int sock_fd=77,errCode=9;const char *errMsg="";
#ifdef SELECTED_STATE
 swr_v1_hook_socket_state *state_=swr_v1_hook_socket_state_new();
#else
 bool connected=false,shutdown_read=false,shutdown_write=false;
#endif
#ifndef SELECTED_STATE
 Coroutine *read_co=nullptr,*write_co=nullptr;
 Coroutine *event_co(uint32_t e) const {return e==SW_EVENT_READ?read_co:write_co;}
 void bind_co(uint32_t e,Coroutine *c){if(e&SW_EVENT_READ)read_co=c;if(e&SW_EVENT_WRITE)write_co=c;}
#endif
 NativeSocket native;NativeSocket *socket=&native;
 std::function<void(Socket*)> dtor_;
 void *read_timer=nullptr,*write_timer=nullptr;
 ~Socket(){record("socket:destroy");
#ifdef SELECTED_STATE
  swr_v1_hook_socket_state_free(state_);
#endif
 }
 static void timer_callback(){}
 struct TimerController {
  TimerController(void *,double,Socket *,void(*)()){record("timer:new");}
  TimerController(Socket *,int,double){record("timer:new");}
  ~TimerController(){record("timer:destroy");}
  bool start(){record("timer:start");return timer_ok;}
 };
 bool wait_event(int event){record("wait:"+std::to_string(event));if(throw_wait)throw std::runtime_error("native-wait");if(close_on_wait)sock_fd=SW_BAD_SOCKET;if(!wake_ok)set_err(ETIMEDOUT);return wake_ok;}
 const char *get_event_str(EventType event) const;
#include ACTUAL_INLINE
 bool shutdown(int how=SHUT_RDWR);bool cancel(EventType event);bool close();bool connect(const sockaddr*,socklen_t);bool check_liveness();
 swr_v1_hook_socket_ops lifecycle_ops();
};
#include ACTUAL_BODIES
// The selected socket's connection state is Rust's (#439); scenarios set and read it the same way.
#ifdef SELECTED_STATE
static void set_state(Socket *s,bool c,bool r,bool w){swr_v1_hook_socket_set_flags(s->state_,{c,r,w});}
static swr_v1_hook_socket_flags_t state_of(Socket *s){return swr_v1_hook_socket_flags(s->state_);}
#else
static void set_state(Socket *s,bool c,bool r,bool w){s->connected=c;s->shutdown_read=r;s->shutdown_write=w;}
static swr_v1_hook_socket_flags_t state_of(Socket *s){return {s->connected,s->shutdown_read,s->shutdown_write};}
#endif
