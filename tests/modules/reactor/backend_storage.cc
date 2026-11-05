#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <iostream>
#include <new>
#include <unistd.h>
#ifdef __linux__
#include <sys/epoll.h>
using NativeEvent=epoll_event;
#else
#include <sys/event.h>
using NativeEvent=struct kevent;
#endif
#include "swoole_rs_reactor.h"
#define swoole_sys_warning(...) std::fprintf(stderr, __VA_ARGS__)
struct Reactor {
    swr_v1_reactor_prepared owner=swr_v1_reactor_state_new();
    int &max_event_num=*reinterpret_cast<int *>(&owner.fields->max_event_num);
    int &native_handle=owner.fields->native_handle;
    bool &ready_=owner.fields->ready;
    swr_v1_reactor_state *handle() const { return owner.owner; }
    ~Reactor(){ swr_v1_reactor_state_free(owner.owner); }
};
struct ReactorImpl { Reactor *reactor_; explicit ReactorImpl(Reactor *r):reactor_(r){} };
#include "backend-storage.inc"
static unsigned waits=0;
template<class Backend> void positive(int count) {
    int pipefds[2]; assert(pipe(pipefds)==0);
    { Reactor reactor; Backend backend(&reactor,count);
      assert(backend.ready()); assert(reactor.max_event_num==count);
      assert(reactor.native_handle==backend.epfd_);
      auto *address=backend.events_;
#ifdef __linux__
      epoll_event interest{}; interest.events=EPOLLIN; interest.data.u64=0x12345678;
      assert(epoll_ctl(reactor.native_handle,EPOLL_CTL_ADD,pipefds[0],&interest)==0);
#else
      int token=23; struct kevent interest;
      EV_SET(&interest,pipefds[0],EVFILT_READ,EV_ADD|EV_ENABLE,0,0,&token);
      assert(kevent(reactor.native_handle,&interest,1,nullptr,0,nullptr)==0);
#endif
      char byte='x'; assert(write(pipefds[1],&byte,1)==1);
      for(unsigned round=0;round<2;++round) {
#ifdef __linux__
        assert(epoll_wait(reactor.native_handle,backend.events_,count,0)==1);
        assert(backend.events_[0].events & EPOLLIN);
        assert(backend.events_[0].data.u64==0x12345678);
#else
        timespec timeout{};
        assert(kevent(reactor.native_handle,nullptr,0,backend.events_,count,&timeout)==1);
        assert(backend.events_[0].ident==static_cast<uintptr_t>(pipefds[0]));
        assert(backend.events_[0].filter==EVFILT_READ);
        assert(backend.events_[0].udata==&token);
#endif
        assert(backend.events_==address); ++waits;
      }
    }
    close(pipefds[0]); close(pipefds[1]);
}
int main() {
#ifdef __linux__
    const auto layout=swr_v1_reactor_epoll_layout();
    assert(layout.size==sizeof(NativeEvent) && layout.alignment==alignof(NativeEvent));
    assert(layout.fields[0]==offsetof(NativeEvent,events));
    assert(layout.fields[1]==offsetof(NativeEvent,data));
#else
    const auto layout=swr_v1_reactor_kqueue_layout();
    assert(layout.size==sizeof(NativeEvent) && layout.alignment==alignof(NativeEvent));
    const size_t expected[]={offsetof(NativeEvent,ident),offsetof(NativeEvent,filter),offsetof(NativeEvent,flags),offsetof(NativeEvent,fflags),offsetof(NativeEvent,data),offsetof(NativeEvent,udata)};
    for(unsigned i=0;i<6;++i) assert(layout.fields[i]==expected[i]);
#endif
    for(int count:{1,3,17,65}) { positive<BackendOriginal>(count); positive<BackendSelected>(count); }
    std::cout<<"{\"schemas_per_engine\":4,\"native_kernel_waits\":"<<waits
             <<",\"size\":"<<sizeof(NativeEvent)<<",\"alignment\":"<<alignof(NativeEvent)<<",\"field_offsets\":[";
    for(unsigned i=0;i<6;++i) std::cout<<(i?",":"")<<layout.fields[i];
    std::cout<<"],\"callbacks\":false,\"allocation_failure_probes\":false}\n";
}
