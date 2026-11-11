//! `Swoole\Thread`: the object, its OS thread and every method. The thread's
//! body (TSRM, request startup, running the script) is [`crate::run`].
use crate::{
    array::{Arguments, Array},
    host::{self, handlers},
    run,
};
use std::{
    cell::Cell,
    ffi::c_int,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
};
use swoole_rs_phpstubs::thread;
use swoole_rs_zend::{
    Call, Class, Custom, CustomClass, E_WARNING, Message, Minit, OwnedArray, bailout, message,
    sys::{ZEND_ACC_FINAL, ZEND_ACC_PUBLIC, ZEND_ACC_READONLY},
};
use swoole_rs_zend::{method_table, sys as z};

static THREAD: CustomClass<PhpThread> = CustomClass::new();

/// `SW_ERROR_SYSTEM_CALL_FAIL`, `SW_ERROR_INVALID_PARAMS`.
const SYSTEM_CALL_FAIL: i64 = 502;
const INVALID_PARAMS: i64 = 504;

/// What `swoole::Thread` tracks for one thread: written by the thread itself.
#[derive(Debug, Default)]
struct State {
    living: AtomicBool,
    exit_status: AtomicI32,
}

/// Upstream's `PhpThread`.
#[derive(Default)]
struct PhpThread {
    /// The OS thread, as `std::thread` holds it.
    thread: swoole_rs_corebase::Thread,
    state: Arc<State>,
    constructed: Cell<bool>,
}

impl Custom for PhpThread {
    fn class() -> &'static CustomClass<Self> {
        &THREAD
    }
    fn new() -> Self {
        Self::default()
    }
}

impl Drop for PhpThread {
    /// The object's free: join the thread.
    fn drop(&mut self) {
        self.thread.join().ok();
    }
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_thread___construct, &thread::arginfo_class_Swoole_Thread___construct, z::ZEND_ACC_PUBLIC;
        c"isAlive" => zim_swoole_thread_isAlive, &thread::arginfo_class_Swoole_Thread_isAlive, z::ZEND_ACC_PUBLIC;
        c"join" => zim_swoole_thread_join, &thread::arginfo_class_Swoole_Thread_join, z::ZEND_ACC_PUBLIC;
        c"joinable" => zim_swoole_thread_joinable, &thread::arginfo_class_Swoole_Thread_joinable, z::ZEND_ACC_PUBLIC;
        c"getExitStatus" => zim_swoole_thread_getExitStatus, &thread::arginfo_class_Swoole_Thread_getExitStatus, z::ZEND_ACC_PUBLIC;
        c"detach" => zim_swoole_thread_detach, &thread::arginfo_class_Swoole_Thread_detach, z::ZEND_ACC_PUBLIC;
        c"getArguments" => zim_swoole_thread_getArguments, &thread::arginfo_class_Swoole_Thread_getArguments, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getId" => zim_swoole_thread_getId, &thread::arginfo_class_Swoole_Thread_getId, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getInfo" => zim_swoole_thread_getInfo, &thread::arginfo_class_Swoole_Thread_getInfo, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"activeCount" => zim_swoole_thread_activeCount, &thread::arginfo_class_Swoole_Thread_activeCount, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"yield" => zim_swoole_thread_yield, &thread::arginfo_class_Swoole_Thread_yield, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"setName" => zim_swoole_thread_setName, &thread::arginfo_class_Swoole_Thread_setName, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        c"setAffinity" => thread_affinity::zim_swoole_thread_setAffinity, &thread::arginfo_class_Swoole_Thread_setAffinity, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        c"getAffinity" => thread_affinity::zim_swoole_thread_getAffinity, &thread::arginfo_class_Swoole_Thread_getAffinity, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"setPriority" => zim_swoole_thread_setPriority, &thread::arginfo_class_Swoole_Thread_setPriority, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getPriority" => zim_swoole_thread_getPriority, &thread::arginfo_class_Swoole_Thread_getPriority, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getNativeId" => zim_swoole_thread_getNativeId, &thread::arginfo_class_Swoole_Thread_getNativeId, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
    ];
}

/// `php_swoole_thread_minit`'s class registration, then `Swoole\Thread\Error`.
pub(crate) fn register(minit: &Minit) -> &'static Class {
    minit.register_object::<PhpThread>(c"Swoole\\Thread", &METHODS.0, ZEND_ACC_FINAL);
    minit.property_long(&THREAD, c"id", 0, ZEND_ACC_PUBLIC | ZEND_ACC_READONLY);
    // After Swoole\Thread, as upstream's order has it.
    crate::request::error_minit(minit);
    &THREAD
}

/// `__construct(string $script_file, mixed ...$args)`.
fn construct(call: &Call) {
    let Ok(file) = call.arity(1, u32::MAX).and_then(|()| call.string(1, b"")) else {
        return;
    };
    // The instance methods run on our objects.
    let Some(thread) = call.custom::<PhpThread>() else {
        return;
    };
    if thread.constructed.get() {
        call.throw_constructed_twice();
        return;
    }
    if file.is_empty() {
        call.throw(
            swoole_rs_phpswoole::exception(),
            c"exec file name is empty",
            INVALID_PARAMS,
        );
        return;
    }
    let argv = if call.num_args() > 1 {
        let argv = Arc::<Array>::default();
        for value in (2..=call.num_args()).filter_map(|n| call.value(n)) {
            argv.push(value);
            if bailout::pending() {
                return;
            }
        }
        Some(Arguments(argv))
    } else {
        None
    };
    let (file, state) = (file.to_vec(), Arc::clone(&thread.state));
    // The OS thread's body, as `std::thread` would run upstream's lambda: straight
    // into the thread's body, which marks the thread alive before anything allocates
    // (a new thread's first malloc can map an arena, and `yield.phpt` gives it 10 µs).
    let started = thread.thread.start(move || {
        let enter = || {
            state.exit_status.store(0, Ordering::Release);
            state.living.store(true, Ordering::Release);
        };
        let exit = |status| {
            state.exit_status.store(status, Ordering::Release);
            state.living.store(false, Ordering::Release);
        };
        // SAFETY: the new OS thread, before anything of PHP's.
        unsafe { run::run(&file, argv, enter, exit) };
    });
    if let Err(errno) = started {
        // `std::system_error::what()`: the OS message.
        let text = std::ffi::CString::new(host::strerror(errno)).unwrap_or_default();
        call.throw(swoole_rs_phpswoole::exception(), &text, SYSTEM_CALL_FAIL);
        return;
    }
    thread.constructed.set(true);
    if let Some(id) = thread.thread.id() {
        #[expect(
            clippy::cast_possible_wrap,
            reason = "upstream's (zend_long) pthread_t"
        )]
        call.update_property_long(Some(&*THREAD), c"id", id as i64);
    }
}

/// `pthread_self()`.
fn me() -> libc::pthread_t {
    // SAFETY: no preconditions.
    unsafe { libc::pthread_self() }
}

/// An empty `sched_param`.
fn sched_param() -> libc::sched_param {
    // SAFETY: an all-zero sched_param is valid.
    unsafe { std::mem::zeroed() }
}

/// `body` on `$this`, which the instance methods run on.
fn with_this(call: &Call, body: impl FnOnce(&PhpThread)) {
    if let Some(thread) = call.custom::<PhpThread>() {
        body(thread);
    }
}

fn is_alive(call: &Call) {
    with_this(call, |this| {
        call.set_bool(this.state.living.load(Ordering::Acquire));
    });
}

fn join(call: &Call) {
    with_this(call, |this| call.set_bool(this.thread.join().is_ok()));
}

fn joinable(call: &Call) {
    with_this(call, |this| call.set_bool(this.thread.joinable()));
}

fn detach(call: &Call) {
    with_this(call, |this| call.set_bool(this.thread.detach()));
}

fn get_exit_status(call: &Call) {
    with_this(call, |this| {
        call.set_long(i64::from(this.state.exit_status.load(Ordering::Acquire)));
    });
}

fn get_arguments(call: &Call) {
    run::arguments(call);
}

fn get_id(call: &Call) {
    let id = me();
    #[expect(
        clippy::cast_possible_wrap,
        reason = "upstream's (zend_long) pthread_t"
    )]
    call.set_long(id as i64);
}

fn get_info(call: &Call) {
    call.set_owned(run::info());
}

fn active_count(call: &Call) {
    let count = run::sw_active_thread_count();
    call.set_long(i64::try_from(count).unwrap_or(i64::MAX));
}

fn yield_now(_call: &Call) {
    std::thread::yield_now();
}

fn set_name(call: &Call) {
    let Ok(name) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        return;
    };
    // The name as C reads it: up to its first NUL.
    let name = name.split(|&byte| byte == 0).next().unwrap_or_default();
    let name = std::ffi::CString::new(name).unwrap_or_default();
    call.set_bool(swoole_rs_ossupport::thread_set_name(&name));
}

/// `php_swoole_error(E_WARNING, "%s", message)`.
fn warn(message: &Message) {
    bailout::defer(host::error(E_WARNING, message.as_cstr()));
}

#[cfg(any(target_os = "linux", target_os = "android"))]
/// `setAffinity`/`getAffinity` (`HAVE_CPU_AFFINITY`: Linux here).
pub mod thread_affinity {
    use super::{Call, message, warn};
    use swoole_rs_ossupport::CpuSet;
    use swoole_rs_phpprocess::{cpu_set_from_array, cpu_set_to_array};

    pub(super) fn set(call: &Call) {
        let Ok(array) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
            return;
        };
        let Some(set) = cpu_set_from_array(array) else {
            call.set_bool(false);
            return;
        };
        if set.set_thread() != 0 {
            warn(&message!("pthread_setaffinity_np() failed"));
            call.set_bool(false);
            return;
        }
        call.set_bool(true);
    }

    pub(super) fn get(call: &Call) {
        let mut set = CpuSet::empty();
        if set.get_thread() < 0 {
            warn(&message!("pthread_getaffinity_np() failed"));
            call.set_bool(false);
            return;
        }
        call.set_owned(cpu_set_to_array(&set));
    }

    super::handlers! {
        zim_swoole_thread_setAffinity => set;
        zim_swoole_thread_getAffinity => get;
    }
}

fn set_priority(call: &Call) {
    let Ok((priority, policy)) = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, -1)?)))
    else {
        return;
    };
    let mut param = sched_param();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's int conversions"
    )]
    let mut sched_policy = policy as c_int;
    let me = me();
    if policy == -1 {
        // SAFETY: writable locals.
        unsafe { libc::pthread_getschedparam(me, &raw mut sched_policy, &raw mut param) };
    }
    #[expect(clippy::cast_possible_truncation, reason = "upstream's int conversion")]
    {
        param.sched_priority = priority as c_int;
    }
    // SAFETY: a valid sched_param.
    if unsafe { libc::pthread_setschedparam(me, sched_policy, &raw const param) } == 0 {
        call.set_bool(true);
    } else {
        bailout::defer(host::sys_error(
            E_WARNING,
            c"pthread_setschedparam() failed",
        ));
        call.set_bool(false);
    }
}

fn get_priority(call: &Call) {
    let mut param = sched_param();
    let mut policy: c_int = 0;
    // SAFETY: writable locals.
    if unsafe { libc::pthread_getschedparam(me(), &raw mut policy, &raw mut param) } != 0 {
        warn(&message!("pthread_getschedparam() failed"));
        call.set_bool(false);
        return;
    }
    let mut array = OwnedArray::default();
    array.insert_long(b"policy", i64::from(policy));
    array.insert_long(b"priority", i64::from(param.sched_priority));
    call.set_owned(array.into_owned());
}

fn get_native_id(call: &Call) {
    call.set_long(swoole_rs_ossupport::swr_v1_os_thread_native_id());
}

handlers! {
    zim_swoole_thread___construct => construct;
    zim_swoole_thread_isAlive => is_alive;
    zim_swoole_thread_join => join;
    zim_swoole_thread_joinable => joinable;
    zim_swoole_thread_getExitStatus => get_exit_status;
    zim_swoole_thread_detach => detach;
    zim_swoole_thread_getArguments => get_arguments;
    zim_swoole_thread_getId => get_id;
    zim_swoole_thread_getInfo => get_info;
    zim_swoole_thread_activeCount => active_count;
    zim_swoole_thread_yield => yield_now;
    zim_swoole_thread_setName => set_name;
    zim_swoole_thread_setPriority => set_priority;
    zim_swoole_thread_getPriority => get_priority;
    zim_swoole_thread_getNativeId => get_native_id;
}
