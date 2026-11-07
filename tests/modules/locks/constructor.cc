// Two translation units: LOCKS_REFERENCE builds the pinned constructors against the pinned
// header, the other the hybrid tree's against its header, plus the faults and the driver.
#include "support.h"
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <iostream>
#include <new>

// One instance shared by both translation units.
inline bool watching;
inline int fault_kind, fault_stage, allocated, released, attr_destroyed, init_calls, attr_init_calls, shared_calls;
inline void *pointers[256];

#ifdef LOCKS_REFERENCE
#include "original.inc"
#else
#include "ported.inc"
#endif

template<class L> static void one(const char *engine,int kind,bool shared,bool fail,int stage=0) {
    allocated=released=attr_destroyed=init_calls=attr_init_calls=shared_calls=0;fault_kind=fail?kind:0;fault_stage=stage;
    for(auto p:pointers)if(p)std::abort();
    int code=0;bool threw=false;char message[256]{};
    watching=true;
    try { L lock(shared);code=lock.lock();if(!code)code=lock.unlock(); }
    catch(const std::system_error &error) { threw=true;code=error.code().value();std::strncpy(message,error.what(),sizeof(message)-1); }
    watching=false;
    std::cout<<"{\"engine\":\""<<engine<<"\",\"kind\":"<<kind<<",\"shared\":"<<shared
             <<",\"stage\":"<<stage<<",\"failed_init\":"<<fail<<",\"threw\":"<<threw<<",\"code\":"<<code
             <<",\"allocated\":"<<allocated<<",\"released\":"<<released<<",\"live\":"<<allocated-released
             <<",\"attr_destroyed\":"<<attr_destroyed<<",\"attr_init_calls\":"<<attr_init_calls
             <<",\"shared_calls\":"<<shared_calls<<",\"init_calls\":"<<init_calls
             <<",\"message\":\""<<message<<"\"}\n";
    // Test-only reclamation AFTER recording the actual constructor's leaked ownership.
    for(auto &p:pointers)if(p){std::free(p);p=nullptr;}
}
// The reference side's entry point: the upstream Mutex constructor. RW and spin locks have
// no C++ class in the hybrid; direct() faults their Rust init.
void upstream(bool shared,bool fail);

#ifdef LOCKS_REFERENCE
void upstream(bool shared,bool fail) { one<reference::Mutex>("upstream",3,shared,fail); }
#else
static void remember(void *p) {
    if(!watching||!p)return;
    for(auto &slot:pointers)if(!slot){slot=p;allocated++;return;}
    std::abort();
}
static void forget(void *p) {
    if(watching&&p)for(auto &slot:pointers)if(slot==p){slot=nullptr;released++;break;}
}
void *operator new(size_t size) { auto p=std::malloc(size?size:1);if(!p)throw std::bad_alloc();remember(p);return p; }
void operator delete(void *p) noexcept { forget(p);std::free(p);if(watching)errno=EPIPE; }
void operator delete(void *p,size_t) noexcept { ::operator delete(p); }
void *operator new[](size_t n) { return ::operator new(n); }
void operator delete[](void *p) noexcept { ::operator delete(p); }
void operator delete[](void *p,size_t) noexcept { ::operator delete(p); }
void *Pool::alloc(size_t n) { auto p=std::malloc(n);remember(p);return p; }
void Pool::free(void *p) { forget(p);std::free(p);if(watching)errno=EPIPE; }

template<class T> static T next_symbol(const char *name) {
    auto p=dlsym(RTLD_NEXT,name);if(!p)std::abort();return reinterpret_cast<T>(p);
}
static decltype(&pthread_mutexattr_init) real_mutex_attr_init;
static decltype(&pthread_rwlockattr_init) real_rw_attr_init;
static decltype(&pthread_mutexattr_setpshared) real_mutex_shared;
static decltype(&pthread_rwlockattr_setpshared) real_rw_shared;
static decltype(&pthread_mutex_init) real_mutex_init;
static decltype(&pthread_rwlock_init) real_rw_init;
static decltype(&pthread_mutexattr_destroy) real_mutex_attr_destroy;
static decltype(&pthread_rwlockattr_destroy) real_rw_attr_destroy;
extern "C" int pthread_mutexattr_init(pthread_mutexattr_t *p) {
    if(watching){attr_init_calls++;if(fault_stage==1&&fault_kind==3){errno=EBUSY;return EAGAIN;}}
    return real_mutex_attr_init(p);
}
extern "C" int pthread_rwlockattr_init(pthread_rwlockattr_t *p) {
    if(watching){attr_init_calls++;if(fault_stage==1&&fault_kind==1){errno=EBUSY;return EAGAIN;}}
    return real_rw_attr_init(p);
}
extern "C" int pthread_mutexattr_setpshared(pthread_mutexattr_t *p,int shared) {
    if(watching){shared_calls++;if(fault_stage==1&&fault_kind==3){errno=EBUSY;return EINVAL;}if(fault_stage==2&&fault_kind==3){errno=EBUSY;return ENOTSUP;}}
    return real_mutex_shared(p,shared);
}
extern "C" int pthread_rwlockattr_setpshared(pthread_rwlockattr_t *p,int shared) {
    if(watching){shared_calls++;if(fault_stage==1&&fault_kind==1){errno=EBUSY;return EINVAL;}if(fault_stage==2&&fault_kind==1){errno=EBUSY;return ENOTSUP;}}
    return real_rw_shared(p,shared);
}
extern "C" int pthread_mutex_init(pthread_mutex_t *p,const pthread_mutexattr_t *a) {
    if(watching){init_calls++;if(fault_kind==3&&fault_stage==1){errno=EBUSY;return EINVAL;}if(fault_stage==0&&fault_kind==3){errno=EBUSY;return ENOMEM;}}
    return real_mutex_init(p,a);
}
extern "C" int pthread_rwlock_init(pthread_rwlock_t *p,const pthread_rwlockattr_t *a) {
    if(watching){init_calls++;if(fault_kind==1&&fault_stage==1){errno=EBUSY;return EINVAL;}if(fault_stage==0&&fault_kind==1){errno=EBUSY;return ENOMEM;}}
    return real_rw_init(p,a);
}
extern "C" int pthread_mutexattr_destroy(pthread_mutexattr_t *p) {
    if(watching&&fault_stage==1&&fault_kind==3){attr_destroyed++;errno=ENOSPC;return EINVAL;}
    int result=real_mutex_attr_destroy(p);if(watching){attr_destroyed++;errno=ENOSPC;}return result;
}
extern "C" int pthread_rwlockattr_destroy(pthread_rwlockattr_t *p) {
    if(watching&&fault_stage==1&&fault_kind==1){attr_destroyed++;errno=ENOSPC;return EINVAL;}
    int result=real_rw_attr_destroy(p);if(watching){attr_destroyed++;errno=ENOSPC;}return result;
}
#ifdef __linux__
static decltype(&pthread_spin_init) real_spin_init;
extern "C" int pthread_spin_init(pthread_spinlock_t *p,int shared) {
    if(watching){init_calls++;if(fault_stage==0&&fault_kind==5){errno=EBUSY;return ENOMEM;}}
    return real_spin_init(p,shared);
}
#endif

static void direct(int kind,bool shared,int stage) {
    alignas(64) unsigned char storage[512];
    auto alignment=swr_v1_locks_align(kind);
    if(!alignment||swr_v1_locks_size(kind)>sizeof(storage)||reinterpret_cast<uintptr_t>(storage)%alignment)std::abort();
    allocated=released=attr_destroyed=init_calls=attr_init_calls=shared_calls=0;
    fault_kind=kind;fault_stage=stage;watching=true;
    int result=swr_v1_locks_init(storage,kind,shared);int error=errno;
    watching=false;
    // Also keep the fixture safe if a future regression accidentally initializes a live lock.
    if(result==0)swr_v1_locks_destroy(storage,kind);
    std::cout<<"{\"direct\":1,\"kind\":"<<kind<<",\"shared\":"<<shared<<",\"stage\":"<<stage
             <<",\"result\":"<<result<<",\"errno\":"<<error<<",\"attr_destroyed\":"<<attr_destroyed
             <<",\"attr_init_calls\":"<<attr_init_calls<<",\"shared_calls\":"<<shared_calls
             <<",\"init_calls\":"<<init_calls<<",\"allocated\":"<<allocated<<",\"released\":"<<released<<"}\n";
}
int main() {
    real_mutex_attr_init=next_symbol<decltype(real_mutex_attr_init)>("pthread_mutexattr_init");
    real_rw_attr_init=next_symbol<decltype(real_rw_attr_init)>("pthread_rwlockattr_init");
    real_mutex_shared=next_symbol<decltype(real_mutex_shared)>("pthread_mutexattr_setpshared");
    real_rw_shared=next_symbol<decltype(real_rw_shared)>("pthread_rwlockattr_setpshared");
    real_mutex_init=next_symbol<decltype(real_mutex_init)>("pthread_mutex_init");
    real_rw_init=next_symbol<decltype(real_rw_init)>("pthread_rwlock_init");
    real_mutex_attr_destroy=next_symbol<decltype(real_mutex_attr_destroy)>("pthread_mutexattr_destroy");
    real_rw_attr_destroy=next_symbol<decltype(real_rw_attr_destroy)>("pthread_rwlockattr_destroy");
#ifdef __linux__
    real_spin_init=next_symbol<decltype(real_spin_init)>("pthread_spin_init");
#endif
    for(bool shared:{false,true})for(bool fail:{false,true}) {
        upstream(shared,fail);one<swoole::Mutex>("rust",3,shared,fail);
    }
    // Failed attribute-stage shims reject follow-up calls before touching storage.
    // Never run the original unsafe continuation against real pthread storage.
    for(bool shared:{false,true})one<swoole::Mutex>("rust",3,shared,true,1);
    one<swoole::Mutex>("rust",3,true,true,2);
    for(int kind:{1,3}) {
        for(bool shared:{false,true})for(int stage:{0,1})direct(kind,shared,stage);
        direct(kind,true,2);
    }
#ifdef __linux__
    for(bool shared:{false,true})direct(5,shared,0);
#endif
}
#endif
