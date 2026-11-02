//! `Swoole\Thread`: the object, its OS thread and every method. The thread's
//! body (TSRM, request startup, running the script) stays C++ glue, reached
//! through `Host::thread_run`; C++ also declares the class constants.
use crate::{
    array::{Array, handle},
    host::{handlers, host},
};
use std::{
    ffi::{CStr, c_int, c_void},
    os::unix::thread::JoinHandleExt,
    ptr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    thread::JoinHandle,
};
use swoole_rs_zend::{
    Call, Class, E_WARNING, Message, Object, message,
    sys::{
        ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, ZEND_ACC_READONLY,
        add_assoc_long_ex, swr_zend_array_init, zend_class_entry, zend_declare_property_long,
        zend_function_entry, zend_object,
    },
    throw_constructed_twice, throw_exception, unset_property_deny,
};

static THREAD: Class = Class::new();

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
#[derive(Debug, Default)]
struct PhpThread {
    handle: Option<JoinHandle<()>>,
    state: Arc<State>,
    constructed: bool,
}

impl PhpThread {
    /// `PhpThread::join`: false when not joinable.
    fn join(&mut self) -> bool {
        match self.handle.take() {
            Some(handle) => {
                // A thread's body never unwinds (C++ catches its bailouts).
                drop(handle.join());
                true
            }
            None => false,
        }
    }
}

type ThreadObject = Object<*mut PhpThread>;

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class.
    let std = unsafe { ThreadObject::create(&THREAD, ce) };
    // SAFETY: the object just created.
    unsafe { *ThreadObject::data(std) = Box::into_raw(Box::default()) };
    std
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees one of our objects.
    let thread = std::mem::replace(unsafe { ThreadObject::data(std) }, ptr::null_mut());
    if !thread.is_null() {
        // SAFETY: the object owned it since `create`.
        let mut thread = unsafe { Box::from_raw(thread) };
        thread.join();
    }
    // SAFETY: as above.
    unsafe { ThreadObject::dtor(std) };
}

/// `php_swoole_thread_minit`'s class registration; C++ adds the constants.
/// # Safety
/// MINIT, after `swr_v1_phpthread_minit`, with a static method table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpthread_thread_minit(
    methods: *const zend_function_entry,
) -> *mut zend_class_entry {
    // SAFETY: MINIT, with the caller's table.
    unsafe {
        THREAD.register(c"Swoole\\Thread", methods, |entry, handlers| {
            entry.ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
            handlers.clone_obj = None;
            handlers.unset_property = Some(unset_property_deny);
            ThreadObject::install(entry, handlers, create, free);
        });
    }
    #[expect(clippy::cast_possible_wrap, reason = "small flags")]
    // SAFETY: MINIT, after `register`; Zend copies the name.
    unsafe {
        zend_declare_property_long(
            THREAD.entry(),
            c"id".as_ptr(),
            2,
            0,
            (ZEND_ACC_PUBLIC | ZEND_ACC_READONLY) as i32,
        );
    }
    THREAD.entry()
}

fn this(call: &Call) -> &'static mut PhpThread {
    // SAFETY: the instance methods run on our objects.
    let thread = unsafe { *ThreadObject::data(call.this()) };
    // SAFETY: the object holds its `PhpThread` from `create` until `free`.
    unsafe { &mut *thread }
}

/// The stack size of a thread `std::thread` would start: the pthread default.
fn stack_size() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        // SAFETY: an all-zero attribute object is initialised below.
        let mut attr: libc::pthread_attr_t = unsafe { std::mem::zeroed() };
        let mut size = 0;
        // SAFETY: a local attribute object.
        if unsafe { libc::pthread_attr_init(&raw mut attr) } != 0 {
            return 8 << 20;
        }
        // SAFETY: initialised above; `size` is writable.
        unsafe { libc::pthread_attr_getstacksize(&raw const attr, &raw mut size) };
        // SAFETY: initialised above.
        unsafe { libc::pthread_attr_destroy(&raw mut attr) };
        size
    })
}

extern "C" fn enter(state: *mut c_void) {
    // SAFETY: the thread's closure keeps the state alive while C++ runs.
    let state = unsafe { &*state.cast::<State>() };
    state.exit_status.store(0, Ordering::Release);
    state.living.store(true, Ordering::Release);
}

extern "C" fn exit(state: *mut c_void, status: c_int) {
    // SAFETY: as in `enter`.
    let state = unsafe { &*state.cast::<State>() };
    state.exit_status.store(status, Ordering::Release);
    state.living.store(false, Ordering::Release);
}

/// A handle C++ may send to another thread.
struct Handle(*mut c_void);
// SAFETY: a shared table handle (an `Arc`), released by the thread.
unsafe impl Send for Handle {}

/// `__construct(string $script_file, mixed ...$args)`.
fn construct(call: &Call) {
    let Ok(file) = call.arity(1, u32::MAX).and_then(|()| call.string(1, b"")) else {
        return;
    };
    let thread = this(call);
    if thread.constructed {
        // SAFETY: in a handler, with `$this` live.
        unsafe { throw_constructed_twice(call.this()) };
        return;
    }
    if file.is_empty() {
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe { throw_exception(host().exception, c"exec file name is empty", INVALID_PARAMS) };
        return;
    }
    let file = file.to_vec();
    let argv = if call.num_args() > 1 {
        let argv = Arc::<Array>::default();
        for n in 2..=call.num_args() {
            // SAFETY: in a handler, with a passed argument.
            unsafe { argv.push(call.zval(n)) };
        }
        Handle(handle(argv))
    } else {
        Handle(ptr::null_mut())
    };
    let state = Arc::clone(&thread.state);
    let spawned = std::thread::Builder::new()
        .stack_size(stack_size())
        .spawn(move || {
            let argv = argv;
            let context = Arc::as_ptr(&state).cast_mut().cast();
            // SAFETY: C++'s thread body; `state` outlives it.
            unsafe {
                (host().thread_run)(
                    file.as_ptr().cast(),
                    file.len(),
                    argv.0,
                    context,
                    enter,
                    exit,
                );
            }
        });
    match spawned {
        Ok(handle) => {
            #[expect(
                clippy::cast_possible_wrap,
                reason = "upstream's (zend_long) pthread_t"
            )]
            let id = handle.as_pthread_t() as i64;
            thread.handle = Some(handle);
            thread.constructed = true;
            call.update_property_long(Some(&THREAD), c"id", id);
        }
        Err(error) => {
            // `std::system_error::what()`: the OS message.
            let text = error
                .raw_os_error()
                .map_or_else(|| error.to_string(), strerror);
            let text = std::ffi::CString::new(text).unwrap_or_default();
            // SAFETY: in a handler, with Swoole's exception class.
            unsafe { throw_exception(host().exception, &text, SYSTEM_CALL_FAIL) };
        }
    }
}

fn strerror(errno: i32) -> String {
    // SAFETY: no preconditions.
    let text = unsafe { libc::strerror(errno) };
    // SAFETY: strerror returns a static or thread-local NUL-terminated message.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

fn is_alive(call: &Call) {
    call.set_bool(this(call).state.living.load(Ordering::Acquire));
}

fn join(call: &Call) {
    call.set_bool(this(call).join());
}

fn joinable(call: &Call) {
    call.set_bool(this(call).handle.is_some());
}

fn detach(call: &Call) {
    // Dropping a handle detaches its thread.
    call.set_bool(this(call).handle.take().is_some());
}

fn get_exit_status(call: &Call) {
    call.set_long(i64::from(
        this(call).state.exit_status.load(Ordering::Acquire),
    ));
}

fn get_arguments(call: &Call) {
    // SAFETY: the return slot.
    unsafe { (host().thread_arguments)(call.return_value()) };
}

fn get_id(call: &Call) {
    // SAFETY: no preconditions.
    let id = unsafe { libc::pthread_self() };
    #[expect(
        clippy::cast_possible_wrap,
        reason = "upstream's (zend_long) pthread_t"
    )]
    call.set_long(id as i64);
}

fn get_info(call: &Call) {
    // SAFETY: the return slot.
    unsafe { (host().thread_info)(call.return_value()) };
}

fn active_count(call: &Call) {
    // SAFETY: C++'s thread counter.
    let count = unsafe { (host().thread_count)() };
    call.set_long(i64::try_from(count).unwrap_or(i64::MAX));
}

fn yield_now(_call: &Call) {
    std::thread::yield_now();
}

fn set_name(call: &Call) {
    let Ok(name) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        return;
    };
    // SAFETY: the parsed string is NUL-terminated and lives for the call.
    call.set_bool(unsafe { (host().thread_set_name)(name.as_ptr().cast()) });
}

/// `php_swoole_error(level, "%s", message)`.
fn warn(message: &Message) {
    // SAFETY: in a handler.
    unsafe { (host().error)(E_WARNING, message.as_cstr().as_ptr()) };
}

#[cfg(any(target_os = "linux", target_os = "android"))]
/// `setAffinity`/`getAffinity` (`HAVE_CPU_AFFINITY`: Linux here).
pub mod thread_affinity {
    use super::{Call, message, warn};
    use swoole_rs_phpprocess::{
        swr_v1_phpprocess_array_to_cpu_set, swr_v1_phpprocess_cpu_set_to_array,
    };

    pub(super) fn set(call: &Call) {
        let Ok(Some(_)) = call.arity(1, 1).and_then(|()| call.array_table(1)) else {
            return;
        };
        // SAFETY: an all-zero cpu set is empty.
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        // SAFETY: the array argument and a local set.
        if !unsafe { swr_v1_phpprocess_array_to_cpu_set(call.zval(1), (&raw mut set).cast()) } {
            call.set_bool(false);
            return;
        }
        // SAFETY: no preconditions.
        let me = unsafe { libc::pthread_self() };
        // SAFETY: a valid cpu set.
        if unsafe { libc::pthread_setaffinity_np(me, size_of::<libc::cpu_set_t>(), &raw const set) }
            != 0
        {
            warn(&message!("pthread_setaffinity_np() failed"));
            call.set_bool(false);
            return;
        }
        call.set_bool(true);
    }

    pub(super) fn get(call: &Call) {
        // SAFETY: an all-zero cpu set is empty.
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        // SAFETY: no preconditions.
        let me = unsafe { libc::pthread_self() };
        // SAFETY: `set` is writable.
        if unsafe { libc::pthread_getaffinity_np(me, size_of::<libc::cpu_set_t>(), &raw mut set) }
            < 0
        {
            warn(&message!("pthread_getaffinity_np() failed"));
            call.set_bool(false);
            return;
        }
        // SAFETY: the return slot and a valid set.
        unsafe { swr_v1_phpprocess_cpu_set_to_array(call.return_value(), (&raw const set).cast()) };
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
    // SAFETY: an all-zero sched_param is valid.
    let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's int conversions"
    )]
    let mut sched_policy = policy as c_int;
    // SAFETY: no preconditions.
    let me = unsafe { libc::pthread_self() };
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
        // SAFETY: in a handler.
        unsafe { (host().sys_error)(E_WARNING, c"pthread_setschedparam() failed".as_ptr()) };
        call.set_bool(false);
    }
}

fn get_priority(call: &Call) {
    // SAFETY: an all-zero sched_param is valid.
    let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
    let mut policy: c_int = 0;
    // SAFETY: no preconditions.
    let me = unsafe { libc::pthread_self() };
    // SAFETY: writable locals.
    if unsafe { libc::pthread_getschedparam(me, &raw mut policy, &raw mut param) } != 0 {
        warn(&message!("pthread_getschedparam() failed"));
        call.set_bool(false);
        return;
    }
    let out = call.return_value();
    // SAFETY: the return slot.
    unsafe { swr_zend_array_init(out) };
    // SAFETY: an array zval; Zend copies the keys.
    unsafe { add_assoc_long_ex(out, c"policy".as_ptr(), 6, i64::from(policy)) };
    // SAFETY: as above.
    unsafe {
        add_assoc_long_ex(
            out,
            c"priority".as_ptr(),
            8,
            i64::from(param.sched_priority),
        );
    };
}

fn get_native_id(call: &Call) {
    // SAFETY: C++'s `swoole_thread_get_native_id`.
    call.set_long(unsafe { (host().thread_native_id)() });
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
