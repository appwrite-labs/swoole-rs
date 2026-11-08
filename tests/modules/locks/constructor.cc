// Rust's lock init for every kind, faulted only at the native pthread boundaries it calls.
// There is no lock class in the hybrid: Swoole\Lock, Table and the server call this ABI.
#include "swoole_rs_locks.h"
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <dlfcn.h>
#include <iostream>
#include <new>
#include <pthread.h>

static bool watching;
static int fault_kind, fault_stage, allocated, released, attr_destroyed, init_calls, attr_init_calls, shared_calls;

// Rust's init must allocate nothing through the native allocator.
void *operator new(size_t size) { auto p=std::malloc(size?size:1);if(!p)throw std::bad_alloc();if(watching)allocated++;return p; }
void operator delete(void *p) noexcept { if(watching&&p)released++;std::free(p); }
void operator delete(void *p,size_t) noexcept { ::operator delete(p); }

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
    for(int kind:{1,3}) {
        for(bool shared:{false,true})for(int stage:{0,1})direct(kind,shared,stage);
        direct(kind,true,2);
    }
#ifdef __linux__
    for(bool shared:{false,true})direct(5,shared,0);
#endif
}
