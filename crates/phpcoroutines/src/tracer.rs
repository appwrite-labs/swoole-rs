//! Swoole's tracer (upstream's `swoole_tracer.cc`): leak detection over the
//! request heap (`swoole.leak_detection`, `swoole_tracer_leak_detect()`), the
//! Chrome-trace profiler of user functions (`swoole.profile`,
//! `swoole_tracer_prof_begin()`/`_end()`) and blocking detection of internal
//! functions run inside a coroutine (`swoole.blocking_detection`). The INI
//! switches are `PHP_INI_SYSTEM`, so crates/phpswoole passes them once at MINIT.
//!
//! The heap hooks are C (their signatures follow the PHP build), and call back
//! here with the hooks removed while Rust may allocate.
use crate::{context, host::now_us, runtime};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, hash_map::DefaultHasher},
    ffi::{CStr, c_char, c_int, c_long, c_void},
    hash::BuildHasherDefault,
    io::Write,
    mem,
    ptr::{self, NonNull},
    sync::OnceLock,
};
use swoole_rs_zend::{
    Call, bailout, handlers,
    string::bytes,
    sys::{
        self as z, swr_zend_json_encode, swr_zend_mm_observe, swr_zend_mm_observer,
        swr_zend_mm_unobserve, swr_zend_number_format, swr_zend_string_release,
        swr_zend_tracer_lineno, swr_zend_try_write, swr_zval_try_addref, zend_execute_data,
        zend_long, zend_observer_fcall_handlers, zend_string, zval,
    },
    value,
};

handlers! {
    zif_swoole_tracer_leak_detect => leak_detect;
    zif_swoole_tracer_prof_begin => prof_begin;
    zif_swoole_tracer_prof_end => prof_end;
}

/// The tracer's INI switches, which are `PHP_INI_SYSTEM`: read once, at MINIT.
struct Config {
    blocking_threshold: zend_long,
    blocking_detection: bool,
    profile: bool,
    leak_detection: bool,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

fn config() -> Option<&'static Config> {
    CONFIG.get()
}

unsafe extern "C" {
    /// Swoole's stdout stream (crates/corebase's C shim).
    fn sw_printf(format: *const c_char, ...) -> c_int;
}

fn leak_detection() -> bool {
    config().is_some_and(|c| c.leak_detection)
}

fn profile() -> bool {
    config().is_some_and(|c| c.profile)
}

fn blocking_detection() -> bool {
    config().is_some_and(|c| c.blocking_detection)
}

/// `zend_function.type`'s `ZEND_INTERNAL_FUNCTION`.
const INTERNAL_FUNCTION: u8 = 1;
/// The `internal_function.reserved` slot holding a blocking-detection span.
const SPAN_SLOT: usize = 4;
/// `DEBUG_BACKTRACE_IGNORE_ARGS`.
const BACKTRACE_IGNORE_ARGS: i32 = 2;

type Hasher = BuildHasherDefault<DefaultHasher>;

/// Where a live request allocation was made.
struct Point {
    size: usize,
    count: usize,
    at: Vec<u8>,
}

/// Leak detection's state (upstream's `TracerG.points`, `counters`, `backtraces`).
struct Leaks {
    points: HashMap<usize, Point, Hasher>,
    /// Live allocations per site.
    counters: HashMap<Vec<u8>, u32, Hasher>,
    /// The first backtrace of each site that reached the threshold.
    backtraces: HashMap<Vec<u8>, zval, Hasher>,
    rounds: usize,
    threshold: zend_long,
}

/// One profiled call: `Prof`.
struct Prof {
    name: Vec<u8>,
    file_pos: Vec<u8>,
    began_at: c_long,
}

/// The profiler's state.
struct Profile {
    profiling: bool,
    /// Each coroutine's open calls.
    stacks: HashMap<c_long, Vec<Prof>, Hasher>,
    root_path: Vec<u8>,
    /// The trace events: a PHP array while profiling.
    events: zval,
}

per_thread! {
    static LEAKS: RefCell<Leaks> = RefCell::new(Leaks {
        points: HashMap::with_hasher(BuildHasherDefault::new()),
        counters: HashMap::with_hasher(BuildHasherDefault::new()),
        backtraces: HashMap::with_hasher(BuildHasherDefault::new()),
        rounds: 0,
        threshold: 0,
    });
    static PROFILE: RefCell<Profile> = RefCell::new(Profile {
        profiling: false,
        stacks: HashMap::with_hasher(BuildHasherDefault::new()),
        root_path: Vec::new(),
        events: undef(),
    });
}

/// An undefined zval (`IS_UNDEF`).
const fn undef() -> zval {
    // SAFETY: an all-zero zval is IS_UNDEF.
    unsafe { mem::zeroed() }
}

/// `swoole_thread_get_native_id()`.
fn native_thread_id() -> c_long {
    #[cfg(target_vendor = "apple")]
    {
        let mut id = 0u64;
        // SAFETY: the calling thread, a writable id.
        unsafe { libc::pthread_threadid_np(0, &raw mut id) };
        id.cast_signed()
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        // SAFETY: gettid takes no arguments.
        unsafe { libc::syscall(libc::SYS_gettid) }
    }
}

/// `swoole_coroutine_get_id()`: the running coroutine, -1 outside one.
fn coroutine_id() -> c_long {
    crate::host::current_cid()
}

/// `tracer_get_file_and_line`: `file:line` of the running user code, or
/// `None` outside any file.
fn file_and_line() -> Option<Vec<u8>> {
    // SAFETY: Zend's executor; it answers "[no active file]" when idle.
    let file = unsafe { z::zend_get_executed_filename() };
    // SAFETY: Zend's NUL-terminated file name.
    let file = unsafe { CStr::from_ptr(file) }.to_bytes();
    // SAFETY: as above.
    let line = unsafe { swr_zend_tracer_lineno() };
    if line == 0 && file == b"[no active file]" {
        return None;
    }
    let mut at = file.to_vec();
    let _infallible = write!(at, ":{line}");
    Some(at)
}

/// `format_bytes`: the size in the largest unit below 1024, two decimals.
fn format_bytes(size: u64) -> String {
    const UNITS: [&str; 9] = ["B", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
    #[expect(clippy::cast_precision_loss, reason = "upstream's double")]
    let mut adjusted = size as f64;
    let mut unit = 0;
    while adjusted >= 1024.0 && unit < 8 {
        adjusted /= 1024.0;
        unit += 1;
    }
    format!("{adjusted:.2} {}", UNITS.get(unit).unwrap_or(&"YB"))
}

/// `zend_fetch_debug_backtrace(.., 0, options, 0)`.
fn backtrace(options: i32) -> zval {
    // SAFETY: an all-zero zval is IS_UNDEF, which Zend overwrites.
    let mut trace: zval = unsafe { mem::zeroed() };
    // SAFETY: a writable zval; Zend walks its own frames.
    unsafe { z::zend_fetch_debug_backtrace(&raw mut trace, 0, options, 0) };
    trace
}

/// `zend_trace_to_string(Z_ARRVAL_P(trace), false)`: a request string.
/// # Safety
/// `trace` is a live backtrace array.
unsafe fn trace_to_string(trace: *const zval) -> *mut zend_string {
    // SAFETY: per the caller, an array zval holds `arr`.
    let value = unsafe { (*trace).value };
    // SAFETY: as above.
    let array = unsafe { value.arr };
    // SAFETY: a live array.
    unsafe { z::zend_trace_to_string(array, false) }
}

// --- leak detection ---------------------------------------------------------

static OBSERVER: swr_zend_mm_observer = swr_zend_mm_observer {
    alloc: Some(on_alloc),
    realloc: Some(on_realloc),
    free: Some(on_free),
};

/// `hook_emalloc`.
fn hook() {
    // SAFETY: a static observer; the request heap is live.
    unsafe { swr_zend_mm_observe(&raw const OBSERVER) };
}

/// `unhook_emalloc`.
fn unhook() {
    // SAFETY: the request heap is live.
    unsafe { swr_zend_mm_unobserve() };
}

impl Leaks {
    /// `save_backtrace` of `at`, when `count` asked for it.
    fn save_backtrace(&mut self, at: Option<Vec<u8>>) {
        if let Some(at) = at {
            let trace = backtrace(BACKTRACE_IGNORE_ARGS);
            self.backtraces.insert(at, trace);
        }
    }

    /// One more live allocation at `at`. A site nearing the threshold without
    /// a kept backtrace is returned, for `save_backtrace`.
    fn count(&mut self, at: &[u8], new_site: bool) -> Option<Vec<u8>> {
        if let Some(counter) = self.counters.get_mut(at) {
            *counter = counter.wrapping_add(1);
            if zend_long::from(*counter) >= self.threshold - 1 && !self.backtraces.contains_key(at)
            {
                return Some(at.to_vec());
            }
        } else if new_site {
            self.counters.insert(at.to_vec(), 1);
        }
        None
    }

    /// `add_point`.
    fn add(&mut self, ptr: usize, size: usize, at: Vec<u8>) -> Option<Vec<u8>> {
        let site = self.count(&at, true);
        self.points.insert(ptr, Point { size, count: 1, at });
        site
    }

    /// `tracer_realloc`'s bookkeeping.
    fn moved(&mut self, ptr: usize, new_ptr: usize, size: usize, at: Vec<u8>) -> Option<Vec<u8>> {
        let Some(point) = self.points.remove(&ptr) else {
            return self.add(new_ptr, size, at);
        };
        let site = self.count(&point.at, false);
        self.points.insert(
            new_ptr,
            Point {
                size,
                count: point.count + 1,
                at: point.at,
            },
        );
        site
    }

    /// `del_point`.
    fn freed(&mut self, ptr: usize) {
        if let Some(point) = self.points.remove(&ptr)
            && let Some(counter) = self.counters.get_mut(&point.at)
        {
            *counter = counter.wrapping_sub(1);
        }
    }

    /// Live bytes and allocations per site (`AllocStat`), in site order.
    fn stats(&self) -> BTreeMap<&[u8], (usize, usize)> {
        let mut stats = BTreeMap::new();
        for point in self.points.values() {
            stats
                .entry(point.at.as_slice())
                .and_modify(|(bytes, count): &mut (usize, usize)| {
                    *count += point.count;
                    *bytes += point.size;
                })
                .or_insert((point.size, point.count));
        }
        stats
    }
}

unsafe extern "C" fn on_alloc(ptr: *mut c_void, size: usize) {
    if let Some(at) = file_and_line() {
        LEAKS.with(|leaks| {
            let mut leaks = leaks.borrow_mut();
            let site = leaks.add(ptr as usize, size, at);
            leaks.save_backtrace(site);
        });
    }
}

unsafe extern "C" fn on_realloc(ptr: *mut c_void, new_ptr: *mut c_void, size: usize) {
    if let Some(at) = file_and_line() {
        LEAKS.with(|leaks| {
            let mut leaks = leaks.borrow_mut();
            let site = leaks.moved(ptr as usize, new_ptr as usize, size, at);
            leaks.save_backtrace(site);
        });
    }
}

unsafe extern "C" fn on_free(ptr: *mut c_void) {
    LEAKS.with(|leaks| {
        // Busy only while this module's own bookkeeping frees: not a user block.
        if let Ok(mut leaks) = leaks.try_borrow_mut() {
            leaks.freed(ptr as usize);
        }
    });
}

/// `ZEND_WRITE`, trapping a bailout from an output handler.
fn write(text: &[u8]) -> Result<(), bailout::Bailout> {
    // SAFETY: live bytes; the shim traps a bailout.
    let bailed = unsafe { swr_zend_try_write(text.as_ptr().cast(), text.len()) };
    if bailed != 0 {
        Err(bailout::Bailout)
    } else {
        Ok(())
    }
}

/// Print the sites with at least `threshold` live allocations, with their
/// backtraces, and forget them.
fn report(threshold: zend_long) -> Result<(), bailout::Bailout> {
    let rounds = LEAKS.with(|leaks| leaks.borrow().rounds);
    let leaked: Vec<(Vec<u8>, usize, usize)> = LEAKS.with(|leaks| {
        leaks
            .borrow()
            .stats()
            .into_iter()
            .filter(|(_, (_, count))| *count as u64 >= threshold.cast_unsigned())
            .map(|(at, (bytes, count))| (at.to_vec(), bytes, count))
            .collect()
    });
    for (at, total, count) in leaked {
        let mut line = format!(
            "[Round#{rounds}] leak {} bytes, alloc {count} times at ",
            format_bytes(total as u64)
        )
        .into_bytes();
        line.extend_from_slice(&at);
        line.push(b'\n');
        write(&line)?;
        let trace = LEAKS.with(|leaks| {
            leaks
                .borrow()
                .backtraces
                .get(&at)
                // SAFETY: a kept backtrace array.
                .map(|trace| unsafe { trace_to_string(trace) })
        });
        if let Some(trace) = trace {
            // SAFETY: a live string.
            let written = write(unsafe { bytes(trace) });
            // SAFETY: the reference trace_to_string returned.
            unsafe { swr_zend_string_release(trace) };
            written?;
        }
        LEAKS.with(|leaks| leaks.borrow_mut().points.retain(|_, point| point.at != at));
        write(b"\n")?;
    }
    Ok(())
}

fn leak_detect(call: &Call) {
    if !leak_detection() {
        return;
    }
    let Ok(threshold) = call.arity(0, 1).and_then(|()| call.long(1, 64)) else {
        call.set_bool(false);
        return;
    };
    unhook();
    if let Err(bailout) = report(threshold) {
        bailout.defer();
        return;
    }
    LEAKS.with(|leaks| {
        let mut leaks = leaks.borrow_mut();
        leaks.threshold = threshold;
        leaks.rounds += 1;
    });
    hook();
}

// --- profiling --------------------------------------------------------------

fn profiling() -> bool {
    PROFILE.with(|profile| profile.borrow().profiling)
}

/// `profiling_begin`: open a call of a user function.
/// # Safety
/// `frame` is the live frame of a user function.
unsafe fn profiling_begin(frame: *mut zend_execute_data) {
    // SAFETY: per the caller.
    let func = unsafe { (*frame).func };
    // SAFETY: a user function's common header.
    let common = unsafe { &*func };
    // SAFETY: as above.
    let common = unsafe { &common.common };
    let mut name = Vec::new();
    if !common.scope.is_null() {
        // SAFETY: a live class entry and its name.
        name.extend_from_slice(c_bytes(unsafe { (*common.scope).name }));
        name.extend_from_slice(b"::");
    }
    name.extend_from_slice(c_bytes(common.function_name));
    let began_at = now_us();
    let cid = coroutine_id();
    PROFILE.with(|profile| {
        let mut profile = profile.borrow_mut();
        let file_pos = file_and_line().map_or_else(Vec::new, |at| {
            at.strip_prefix(profile.root_path.as_slice())
                .map_or(at.clone(), <[u8]>::to_vec)
        });
        profile.stacks.entry(cid).or_default().push(Prof {
            name,
            file_pos,
            began_at,
        });
    });
}

/// `ZSTR_VAL` up to its first NUL, as `%s` and `std::string(const char *)` read it.
fn c_bytes<'a>(string: *const zend_string) -> &'a [u8] {
    // SAFETY: a live string.
    let all = unsafe { bytes(string) };
    all.split(|&b| b == 0).next().unwrap_or(all)
}

/// `profiling_end`: close the coroutine's innermost call as a trace event.
fn profiling_end() {
    let cid = coroutine_id();
    PROFILE.with(|profile| {
        let mut profile = profile.borrow_mut();
        let Some(prof) = profile.stacks.get_mut(&cid).and_then(Vec::pop) else {
            return;
        };
        let ended_at = now_us();
        let mut name = prof.name;
        if !prof.file_pos.is_empty() {
            name.extend_from_slice(b" (");
            name.extend_from_slice(&prof.file_pos);
            name.push(b')');
        }
        // SAFETY: an all-zero zval is IS_UNDEF, which array_init overwrites.
        let mut event: zval = unsafe { mem::zeroed() };
        let event_ptr = &raw mut event;
        // SAFETY: a writable zval.
        unsafe { value::array_init(event_ptr) };
        add_bytes(event_ptr, c"name", &name);
        add_bytes(event_ptr, c"ph", b"X");
        add_bytes(event_ptr, c"cat", b"FEE");
        #[expect(clippy::cast_precision_loss, reason = "upstream's double")]
        add_double(event_ptr, c"ts", prof.began_at as f64);
        #[expect(clippy::cast_precision_loss, reason = "upstream's double")]
        add_double(event_ptr, c"dur", (ended_at - prof.began_at) as f64);
        // SAFETY: a live array.
        unsafe { value::add_long(event_ptr, c"pid", native_thread_id()) };
        // SAFETY: a live array.
        unsafe { value::add_long(event_ptr, c"tid", cid) };
        // SAFETY: the events array is live while profiling.
        let events = unsafe { profile.events.value.arr };
        // SAFETY: a live array; it takes the event.
        unsafe { z::zend_hash_next_index_insert(events, event_ptr) };
    });
}

fn add_bytes(array: *mut zval, key: &CStr, text: &[u8]) {
    // SAFETY: a live array; Zend copies the key and the bytes.
    unsafe {
        z::add_assoc_stringl_ex(
            array,
            key.as_ptr(),
            key.count_bytes(),
            text.as_ptr().cast(),
            text.len(),
        );
    }
}

fn add_double(array: *mut zval, key: &CStr, number: f64) {
    // SAFETY: a live array; Zend copies the key.
    unsafe { z::add_assoc_double_ex(array, key.as_ptr(), key.count_bytes(), number) };
}

/// `profiling_clear`.
fn profiling_clear() {
    PROFILE.with(|profile| {
        let mut profile = profile.borrow_mut();
        // SAFETY: the events array or UNDEF/NULL, which it ignores.
        unsafe { z::zval_ptr_dtor(&raw mut profile.events) };
        // SAFETY: an all-zero zval is IS_UNDEF; upstream's ZVAL_NULL reads the same.
        profile.events = unsafe { mem::zeroed() };
        profile.stacks.clear();
        profile.profiling = false;
    });
}

fn prof_begin(call: &Call) {
    if !profile() || profiling() {
        call.set_bool(false);
        return;
    }
    PROFILE.with(|profile| {
        let mut profile = profile.borrow_mut();
        // SAFETY: a writable zval holding nothing (cleared after each profile).
        unsafe { value::array_init(&raw mut profile.events) };
        profile.profiling = true;
    });
    let Ok(options) = call.arity(0, 1).and_then(|()| call.array_opt(1)) else {
        call.set_bool(false);
        return;
    };
    if let Some(options) = options {
        // SAFETY: an array argument.
        let table = unsafe { value::table(options) };
        let key = c"root_path";
        // SAFETY: a live table; Zend reads the key.
        let found = unsafe { z::zend_hash_str_find(table, key.as_ptr(), key.count_bytes()) };
        if let Some(found) = NonNull::new(found) {
            // SAFETY: a live zval; a `__toString` bailout is deferred.
            let root = unsafe { value::get_string(found.as_ptr()) };
            if bailout::pending() {
                return;
            }
            let mut path = c_bytes(root).to_vec();
            // SAFETY: the reference get_string returned.
            unsafe { swr_zend_string_release(root) };
            if path.last() != Some(&b'/') {
                path.push(b'/');
            }
            PROFILE.with(|profile| profile.borrow_mut().root_path = path);
        }
    }
    call.set_bool(true);
}

/// `zend::array_set(array, key, value)`: the array takes a new reference.
/// # Safety
/// `array` is a live array zval and `value` a live zval.
unsafe fn array_set(array: *mut zval, key: &CStr, value: *mut zval) {
    // SAFETY: per the caller.
    unsafe { swr_zval_try_addref(value) };
    // SAFETY: per the caller; Zend copies the key.
    unsafe { z::add_assoc_zval_ex(array, key.as_ptr(), key.count_bytes(), value) };
}

/// The profile as viztracer's JSON, or `None` when it cannot be encoded.
fn trace_json() -> Option<*mut zend_string> {
    PROFILE.with(|profile| {
        let mut profile = profile.borrow_mut();
        // SAFETY: all-zero zvals are IS_UNDEF, which array_init overwrites.
        let (mut json, mut metadata) = (undef(), undef());
        let (json_ptr, metadata_ptr) = (&raw mut json, &raw mut metadata);
        // SAFETY: a writable zval.
        unsafe { value::array_init(json_ptr) };
        // SAFETY: live arrays.
        unsafe { array_set(json_ptr, c"traceEvents", &raw mut profile.events) };
        // SAFETY: a writable zval.
        unsafe { value::array_init(metadata_ptr) };
        add_bytes(metadata_ptr, c"version", b"0.17.1");
        // SAFETY: a live array; Zend copies the key.
        unsafe { z::add_assoc_bool_ex(metadata_ptr, c"overflow".as_ptr(), 8, false) };
        // SAFETY: live arrays.
        unsafe { array_set(json_ptr, c"viztracer_metadata", metadata_ptr) };
        // SAFETY: a live array of strings and numbers: no user code runs.
        let encoded = unsafe { swr_zend_json_encode(json_ptr) };
        // SAFETY: our references.
        unsafe { z::zval_ptr_dtor(metadata_ptr) };
        // SAFETY: as above; the events keep their own reference.
        unsafe { z::zval_ptr_dtor(json_ptr) };
        NonNull::new(encoded).map(NonNull::as_ptr)
    })
}

fn prof_end(call: &Call) {
    if !profile() || !profiling() {
        call.set_bool(false);
        return;
    }
    let Ok(Some(file)) = call.arity(1, 1).and_then(|()| call.str(1)) else {
        call.set_bool(false);
        return;
    };
    let Some(json) = trace_json() else {
        call.set_bool(false);
        return;
    };
    let path = <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(c_bytes(file));
    // SAFETY: a live string.
    let written = std::fs::write(path, unsafe { bytes(json) });
    // SAFETY: the encoder's string.
    unsafe { swr_zend_string_release(json) };
    if written.is_err() {
        call.set_bool(false);
        return;
    }
    profiling_clear();
    call.set_bool(true);
}

// --- blocking detection -----------------------------------------------------

/// `BlockingDetectionSpan`: an internal function's call in a coroutine.
struct Span {
    began_at: c_long,
    switch_count: usize,
}

/// `&execute_data->func->internal_function.reserved[SPAN_SLOT]`.
/// # Safety
/// `frame` is the live frame of an internal function.
unsafe fn span_slot(frame: *mut zend_execute_data) -> *mut *mut c_void {
    // SAFETY: per the caller.
    let func = unsafe { (*frame).func };
    // SAFETY: an internal function's header.
    let internal = unsafe { &raw mut (*func).internal_function };
    // SAFETY: as above.
    unsafe { &raw mut (*internal).reserved[SPAN_SLOT] }
}

/// `blocking_detection_begin`.
/// # Safety
/// As [`span_slot`].
unsafe fn blocking_begin(frame: *mut zend_execute_data) {
    let task = context::current_task();
    if task.is_null() {
        return;
    }
    let began_at = now_us();
    let switch_count = context::watch_switches(true);
    let span = Box::into_raw(Box::new(Span {
        began_at,
        switch_count,
    }));
    // SAFETY: per the caller.
    let slot = unsafe { span_slot(frame) };
    // SAFETY: as above.
    unsafe { *slot = span.cast() };
}

/// `blocking_detection_end`: report the call when it ran longer than the
/// threshold without switching coroutines.
/// # Safety
/// As [`span_slot`].
unsafe fn blocking_end(frame: *mut zend_execute_data) {
    let switches = context::watch_switches(false);
    // SAFETY: per the caller.
    let slot = unsafe { span_slot(frame) };
    // SAFETY: as above.
    let span = unsafe { mem::replace(&mut *slot, ptr::null_mut()) };
    if span.is_null() {
        return;
    }
    // SAFETY: boxed by blocking_begin, taken out of the slot.
    let span = unsafe { Box::from_raw(span.cast::<Span>()) };
    let duration = now_us() - span.began_at;
    let Some(config) = config() else { return };
    if span.switch_count != switches || duration <= config.blocking_threshold {
        return;
    }
    // SAFETY: Zend's number formatting.
    let duration = unsafe { swr_zend_number_format(duration) };
    let mut trace = backtrace(0);
    // SAFETY: a backtrace array.
    let trace_text = unsafe { trace_to_string(&raw const trace) };
    // SAFETY: our backtrace.
    unsafe { z::zval_ptr_dtor(&raw mut trace) };
    // SAFETY: per the caller.
    let func = unsafe { (*frame).func };
    // SAFETY: an internal function's header.
    let common = unsafe { &*func };
    // SAFETY: as above.
    let common = unsafe { &common.common };
    let mut message = format!(
        " >>> [Detected blocking I/O in Coroutine#{}, internal function `",
        runtime::get_cid()
    )
    .into_bytes();
    if !common.scope.is_null() {
        // SAFETY: a live class entry and its name.
        message.extend_from_slice(c_bytes(unsafe { (*common.scope).name }));
        message.extend_from_slice(b"::");
    }
    message.extend_from_slice(c_bytes(common.function_name));
    message.extend_from_slice(b"()` blocked for ");
    message.extend_from_slice(c_bytes(duration));
    message.extend_from_slice(b" us]\n");
    message.extend_from_slice(c_bytes(trace_text));
    message.push(0);
    // SAFETY: a NUL-terminated message.
    unsafe { sw_printf(c"%s".as_ptr(), message.as_ptr()) };
    // SAFETY: our references.
    unsafe { swr_zend_string_release(duration) };
    // SAFETY: as above.
    unsafe { swr_zend_string_release(trace_text) };
}

// --- observer ---------------------------------------------------------------

/// Whether the function is an internal one.
/// # Safety
/// `frame` is a live frame with a function.
unsafe fn internal(frame: *mut zend_execute_data) -> bool {
    // SAFETY: per the caller.
    let func = unsafe { (*frame).func };
    // SAFETY: as above.
    let func = unsafe { &*func };
    // SAFETY: every function starts with its type.
    let kind = unsafe { func.type_ };
    kind == INTERNAL_FUNCTION
}

/// # Safety
/// `frame` is a live frame with a function.
unsafe fn profiles(frame: *mut zend_execute_data) -> bool {
    // SAFETY: per the caller.
    profile() && profiling() && !unsafe { internal(frame) }
}

/// # Safety
/// `frame` is a live frame with a function.
unsafe fn detects_blocking(frame: *mut zend_execute_data) -> bool {
    // SAFETY: per the caller.
    blocking_detection() && unsafe { internal(frame) }
}

unsafe extern "C" fn observer_begin(frame: *mut zend_execute_data) {
    // SAFETY: Zend observes a live frame.
    if unsafe { profiles(frame) } {
        // SAFETY: a user function's frame.
        unsafe { profiling_begin(frame) };
    }
    // SAFETY: as above.
    if unsafe { detects_blocking(frame) } {
        // SAFETY: an internal function's frame.
        unsafe { blocking_begin(frame) };
    }
}

unsafe extern "C" fn observer_end(frame: *mut zend_execute_data, _: *mut zval) {
    // SAFETY: Zend observes a live frame.
    if unsafe { profiles(frame) } {
        profiling_end();
    }
    // SAFETY: as above.
    if unsafe { detects_blocking(frame) } {
        // SAFETY: an internal function's frame.
        unsafe { blocking_end(frame) };
    }
}

/// `tracer_observer`: observe a function's calls when, at its first call,
/// profiling covers it (a user function) or blocking detection does (an
/// internal one).
unsafe extern "C" fn observer(frame: *mut zend_execute_data) -> zend_observer_fcall_handlers {
    let none = zend_observer_fcall_handlers {
        begin: None,
        end: None,
    };
    // SAFETY: Zend passes a live frame.
    let func = unsafe { (*frame).func };
    // SAFETY: a function's common header, when there is one.
    if func.is_null() {
        return none;
    }
    // SAFETY: a live function.
    let func = unsafe { &*func };
    // SAFETY: every function starts with its common header.
    if unsafe { func.common.function_name }.is_null() {
        return none;
    }
    // SAFETY: a live frame with a function.
    if !unsafe { profiles(frame) } && !unsafe { detects_blocking(frame) } {
        return none;
    }
    zend_observer_fcall_handlers {
        begin: Some(observer_begin),
        end: Some(observer_end),
    }
}

/// `php_swoole_tracer_minit`: keep the switches and observe calls when
/// profiling or blocking detection is on.
/// # Safety
/// At MINIT, once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpcoroutines_tracer_minit(
    blocking_threshold: zend_long,
    blocking_detection: bool,
    profile: bool,
    leak_detection: bool,
) {
    CONFIG.get_or_init(|| Config {
        blocking_threshold,
        blocking_detection,
        profile,
        leak_detection,
    });
    if blocking_detection || profile {
        // SAFETY: MINIT, when Zend takes observers.
        unsafe { z::zend_observer_fcall_register(Some(observer)) };
    }
}

/// `php_swoole_tracer_rinit`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_tracer_rinit() {
    if leak_detection() {
        hook();
    }
}

/// `php_swoole_tracer_rshutdown`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpcoroutines_tracer_rshutdown() {
    if leak_detection() {
        unhook();
    }
    let traces = LEAKS.with(|leaks| mem::take(&mut leaks.borrow_mut().backtraces));
    for (_, mut trace) in traces {
        // SAFETY: a kept backtrace array.
        unsafe { z::zval_ptr_dtor(&raw mut trace) };
    }
    profiling_clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_formatted_in_the_largest_unit() {
        assert_eq!(format_bytes(0), "0.00 B");
        assert_eq!(format_bytes(1023), "1023.00 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.00 MB");
        assert_eq!(format_bytes(u64::MAX), "16.00 EB");
    }

    fn leaks() -> Leaks {
        Leaks {
            points: HashMap::default(),
            counters: HashMap::default(),
            backtraces: HashMap::default(),
            rounds: 0,
            threshold: 1000,
        }
    }

    #[test]
    fn points_follow_allocations() {
        let mut leaks = leaks();
        assert_eq!(leaks.add(1, 10, b"a.php:1".to_vec()), None);
        assert_eq!(leaks.add(2, 20, b"a.php:1".to_vec()), None);
        assert_eq!(leaks.add(3, 5, b"b.php:2".to_vec()), None);
        assert_eq!(leaks.moved(1, 4, 30, b"c.php:3".to_vec()), None);
        assert_eq!(leaks.moved(9, 8, 7, b"c.php:3".to_vec()), None);
        leaks.freed(2);
        assert_eq!(leaks.counters[b"a.php:1".as_slice()], 2);
        let stats = leaks.stats();
        assert_eq!(stats[b"a.php:1".as_slice()], (30, 2));
        assert_eq!(stats[b"b.php:2".as_slice()], (5, 1));
        assert_eq!(stats[b"c.php:3".as_slice()], (7, 1));
    }

    #[test]
    fn a_site_near_the_threshold_asks_for_its_backtrace() {
        let mut leaks = leaks();
        leaks.threshold = 3;
        assert_eq!(leaks.add(1, 1, b"a.php:1".to_vec()), None);
        assert_eq!(
            leaks.add(2, 1, b"a.php:1".to_vec()),
            Some(b"a.php:1".to_vec())
        );
    }
}
