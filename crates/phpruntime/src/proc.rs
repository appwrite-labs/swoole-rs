//! `proc_terminate`, `proc_close` and `proc_get_status` for the coroutine `proc_open`
//! (`HOOK_PROC`), and the argv and environment `proc_open` builds before it forks.
//! The process record, its resource type and destructor stay in thirdparty
//! `proc_open.cc`, read here through its accessors.
use core::{
    ffi::{c_char, c_int, c_void},
    marker::PhantomData,
    ptr::{self, NonNull},
};
use std::ffi::CString;
use swoole_rs_corebase::{CCast, Switch, built_with, sys::Text};
use swoole_rs_zend::{
    Array, ArrayOrStr, Call, Emalloc, Key, ReturnSlot, Value, ZStr, ZString,
    sys::{IS_RESOURCE, php_stream, zend_ulong},
    value_error,
};

/// `proc_open.cc`'s `sw_php_process_handle` behind the resource argument `'a` borrows.
/// Not `Copy`: closing takes it, after which the record is freed.
#[repr(transparent)]
struct Process<'a>(NonNull<c_void>, PhantomData<&'a Value>);

unsafe extern "C" {
    /// The record behind `zproc`'s resource, or none once Zend has thrown.
    safe fn swr_proc_fetch(zproc: &Value) -> Option<Process<'_>>;
    safe fn swr_proc_child(proc: &Process<'_>) -> libc::pid_t;
    safe fn swr_proc_command<'a>(proc: &'a Process<'_>) -> &'a ZStr;
    safe fn swr_proc_set_running(proc: &Process<'_>, running: bool);
    /// Closes `zproc`'s resource, freeing the record; the wait status its destructor
    /// reaped.
    safe fn swr_proc_close(proc: Process<'_>, zproc: &Value) -> c_int;
}

swoole_rs_zend::handlers! {
    zif_swoole_proc_open => open;
    zif_swoole_proc_terminate => terminate;
    zif_swoole_proc_close => close;
    zif_swoole_proc_get_status => get_status;
}

/// `_co_waitpid`: `System::waitpid_safe` in a Swoole-thread build, else `System::waitpid`.
fn waitpid(pid: libc::pid_t, status: &mut c_int, options: c_int) -> libc::pid_t {
    if built_with(Switch::Thread) {
        swoole_rs_hooks::exec::system_waitpid_safe(pid, Some(status), options)
    } else {
        swoole_rs_corebase::waits::wait_upstream::system_waitpid(pid, status, options, -1.0)
    }
}

/// `proc_terminate(resource $process, int $signal = SIGTERM): bool`.
fn terminate(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.resource(1)?, call.long(2, libc::SIGTERM.into())?)));
    let Ok((zproc, signal)) = parsed else {
        return;
    };
    let Some(proc) = swr_proc_fetch(zproc) else {
        return;
    };
    // `kill(pid, sig_no)`: the int conversion keeps the low 32 bits.
    let killed = swoole_rs_corebase::sys::kill(swr_proc_child(&proc), signal.c_cast::<c_int>());
    call.set_bool(killed.is_ok());
}

/// `proc_close(resource $process): int`.
fn close(call: &Call) {
    let Ok(zproc) = call.arity(1, 1).and_then(|()| call.resource(1)) else {
        return;
    };
    let Some(proc) = swr_proc_fetch(zproc) else {
        return;
    };
    call.set_long(swr_proc_close(proc, zproc).into());
}

/// `proc_get_status(resource $process): array`.
fn get_status(call: &Call) {
    let Ok(zproc) = call.arity(1, 1).and_then(|()| call.resource(1)) else {
        return;
    };
    let Some(proc) = swr_proc_fetch(zproc) else {
        return;
    };
    let child = swr_proc_child(&proc);
    call.set_array();
    call.add_assoc_stringl(c"command", swr_proc_command(&proc).bytes());
    call.add_assoc_long(c"pid", child.into());
    let (mut running, mut signaled, mut stopped) = (true, false, false);
    let (mut exitcode, mut termsig, mut stopsig) = (-1, 0, 0);
    let mut wstatus = 0;
    swoole_rs_corebase::sys::set_errno(0);
    let waited = waitpid(child, &mut wstatus, libc::WNOHANG | libc::WUNTRACED);
    if waited == child {
        if libc::WIFEXITED(wstatus) {
            running = false;
            exitcode = libc::WEXITSTATUS(wstatus);
        }
        if libc::WIFSIGNALED(wstatus) {
            running = false;
            signaled = true;
            termsig = libc::WTERMSIG(wstatus);
        }
        if libc::WIFSTOPPED(wstatus) {
            stopped = true;
            stopsig = libc::WSTOPSIG(wstatus);
        }
    } else if waited == -1 {
        // ECHILD: the pid is gone or not this process's child.
        running = false;
    }
    swr_proc_set_running(&proc, running);
    call.add_assoc_bool(c"running", running);
    call.add_assoc_bool(c"signaled", signaled);
    call.add_assoc_bool(c"stopped", stopped);
    call.add_assoc_long(c"exitcode", exitcode.into());
    call.add_assoc_long(c"termsig", termsig.into());
    call.add_assoc_long(c"stopsig", stopsig.into());
}

/// `estrdup`: `bytes` and a NUL in request memory.
fn emalloc_string(bytes: &[u8]) -> Box<[u8], Emalloc> {
    let mut copy = Vec::with_capacity_in(bytes.len() + 1, Emalloc::default());
    copy.extend_from_slice(bytes);
    copy.push(0);
    copy.into_boxed_slice()
}

/// A request allocation handed to C, which `efree`s it.
fn to_c<T>(allocation: Box<[T], Emalloc>) -> *mut T {
    Box::into_raw_with_allocator(allocation).0.cast()
}

/// `get_valid_arg_string`: element `number` (1-based) of the command array as a
/// string, or none once a `ValueError` is thrown.
fn arg_string(value: &Value, number: usize) -> Option<ZString> {
    let string = value.string();
    if number == 1 && string.bytes().is_empty() {
        value_error(c"First element must contain a non-empty program name");
        return None;
    }
    if string.bytes().contains(&0) {
        let message = format!("Command array element {number} contains a null byte");
        value_error(&CString::new(message).unwrap_or_default());
        return None;
    }
    Some(string)
}

/// `get_command_from_array`: the program (the command array's first element) and
/// each element as a NUL-terminated string in request memory, which stays Rust's
/// until [`argv_to_c`] hands it over. None once an element threw.
/// A command array's elements as NUL-terminated strings in request memory.
type ArgStrings = Vec<Box<[u8], Emalloc>>;

fn command_from_array(command: &Array) -> Option<(ZString, ArgStrings)> {
    let mut program = None;
    let mut strings = Vec::with_capacity(command.count().c_cast::<usize>());
    let mut failed = false;
    command.for_each(|_, value| {
        if failed {
            return;
        }
        let Some(string) = arg_string(value, strings.len() + 1) else {
            failed = true;
            return;
        };
        strings.push(emalloc_string(string.bytes()));
        program.get_or_insert(string);
    });
    if failed {
        return None;
    }
    Some((program?, strings))
}

/// The argv `execvp` takes: each string, then a null, in request memory C frees
/// (`efree_argv`) from here on.
fn argv_to_c(strings: ArgStrings) -> *mut *mut c_char {
    let mut pointers = Vec::with_capacity_in(strings.len() + 1, Emalloc::default());
    pointers.extend(
        strings
            .into_iter()
            .map(|string| to_c(string).cast::<c_char>()),
    );
    pointers.push(ptr::null_mut());
    to_c(pointers.into_boxed_slice())
}

/// `_php_array_to_envp`: `environment` (an array) as `name=value` strings, NUL
/// after each, into one request allocation (`envp`, four NULs longer) and a
/// null-terminated array of pointers into it (`envarray`), which C frees
/// (`_php_free_envp`). An element whose string is empty is left out; one with an
/// integer or empty key is its value alone.
fn array_to_envp(environment: &Array, envp: &mut *mut c_char, envarray: &mut *mut *mut c_char) {
    let count = environment.count().c_cast::<usize>();
    let mut block = Vec::new_in(Emalloc::default());
    let mut starts = Vec::with_capacity(count);
    environment.for_each(|key, value| {
        let string = value.string();
        if string.bytes().is_empty() {
            return;
        }
        starts.push(block.len());
        if let Key::Name(name) = key
            && !name.is_empty()
        {
            block.extend_from_slice(name);
            block.push(b'=');
        }
        block.extend_from_slice(string.bytes());
        block.push(0);
    });
    block.extend_from_slice(&[0; 4]);
    let base = to_c(block.into_boxed_slice()).cast::<c_char>();
    // `ecalloc(cnt + 1, ...)`: a slot per element, the unused ones null.
    let mut pointers = Vec::with_capacity_in(count + 1, Emalloc::default());
    pointers.extend(starts.iter().map(|&start| base.wrapping_add(start)));
    pointers.resize(count + 1, ptr::null_mut());
    *envp = base;
    *envarray = to_c(pointers.into_boxed_slice());
}

unsafe extern "C" {
    /// `openpty(master, slave, NULL, NULL, NULL)`.
    safe fn openpty(
        master: &mut c_int,
        slave: &mut c_int,
        name: Option<&mut c_char>,
        termp: Option<&libc::termios>,
        winp: Option<&libc::winsize>,
    ) -> c_int;
}

/// `php_error_docref(NULL, E_WARNING, "<message>")`, the message's bytes as given.
fn warn(message: impl Into<Vec<u8>>) {
    let message = CString::new(message).unwrap_or_default();
    swoole_rs_zend::bailout::defer(crate::report(
        crate::Report::Docref,
        swoole_rs_zend::E_WARNING,
        &message,
    ));
}

/// `strerror(errno)`.
fn last_error() -> String {
    crate::strerror(swoole_rs_corebase::sys::errno())
        .to_string_lossy()
        .into_owned()
}

/// `make_descriptor_cloexec`.
fn cloexec(fd: c_int) {
    // Upstream ignores whether it took.
    let _cloexec = swoole_rs_corebase::sys::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
}

/// `set_proc_descriptor_to_socket`, which `pipe` uses too: a Unix stream socket pair,
/// the parent's end close-on-exec. False once it warned.
fn setup_socket(parentend: &mut c_int, childend: &mut c_int) -> bool {
    let Some([parent, child]) =
        swoole_rs_corebase::sys::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0)
    else {
        warn(format!("Unable to create socket pair: {}", last_error()));
        return false;
    };
    cloexec(parent);
    (*parentend, *childend) = (parent, child);
    true
}

/// `set_proc_descriptor_to_blackhole`: `/dev/null`, read-write. False once it warned.
fn setup_null(childend: &mut c_int) -> bool {
    let Ok(fd) = swoole_rs_corebase::sys::open(c"/dev/null", libc::O_RDWR, 0) else {
        warn(format!("Failed to open /dev/null: {}", last_error()));
        return false;
    };
    *childend = fd;
    true
}

/// `set_proc_descriptor_to_pty`: every `pty` descriptor shares one pseudoterminal,
/// opened by the first (`master` and `slave` start at -1); the child gets a copy of
/// the slave end and the parent one of the master. False once it warned.
fn setup_pty(
    master: &mut c_int,
    slave: &mut c_int,
    parentend: &mut c_int,
    childend: &mut c_int,
) -> bool {
    if *master == -1 && openpty(master, slave, None, None, None) != 0 {
        warn(format!(
            "Could not open PTY (pseudoterminal): {}",
            last_error()
        ));
        return false;
    }
    // Upstream keeps whatever dup returns, -1 included.
    *childend = swoole_rs_corebase::sys::dup(*slave).unwrap_or(-1);
    *parentend = swoole_rs_corebase::sys::dup(*master).unwrap_or(-1);
    true
}

/// `redirect_proc_descriptor` once C looked `target` up: `found` is the child end of
/// the descriptor with that index, or -1 when none has it, where 0, 1 and 2 mean the
/// parent's own. `to` gets a copy. False once it warned.
fn setup_redirect(found: c_int, target: c_int, to: &mut c_int, nindex: c_int) -> bool {
    let from = if found != -1 {
        found
    } else if (0..=2).contains(&target) {
        target
    } else {
        warn(format!("Redirection target {target} not found"));
        return false;
    };
    setup_dup(from, to, nindex)
}

/// `dup_proc_descriptor`: a copy of `from` into `to` (-1 when it fails). False once
/// it warned, naming descriptor `nindex`.
fn setup_dup(from: c_int, to: &mut c_int, nindex: c_int) -> bool {
    let Ok(fd) = swoole_rs_corebase::sys::dup(from) else {
        *to = -1;
        warn(format!(
            "Failed to dup() for descriptor {nindex}: {}",
            last_error()
        ));
        return false;
    };
    *to = fd;
    true
}

/// `proc_open.cc`'s `descriptorspec_item` array, read only through
/// `swr_proc_descriptor_child_of` while the walk that owns it runs.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Descriptors<'a>(NonNull<c_void>, PhantomData<&'a ()>);

unsafe extern "C" {
    /// The child end of the first of the `count` descriptors set up so far whose
    /// index is `target`, or -1.
    safe fn swr_proc_descriptor_child_of(
        descriptors: Descriptors<'_>,
        count: c_int,
        target: c_int,
    ) -> c_int;
    /// Records item `i`'s child fd `index` and its setup (type, mode, parent end,
    /// child end).
    safe fn swr_proc_descriptor_set(
        descriptors: Descriptors<'_>,
        i: c_int,
        index: c_int,
        setup: &[c_int; 4],
    );
    /// `set_proc_descriptor_from_resource`'s stream access: the descriptor of the
    /// stream `resource` names (`zend_fetch_resource` of a `stream`, then
    /// `php_stream_cast` as an fd), into `fd`. False once it threw or reported.
    safe fn swr_proc_stream_fd(resource: &Value, fd: &mut c_int) -> bool;
    /// `set_proc_descriptor_to_file`: `path` opened in `mode` through its stream
    /// wrapper and released as a descriptor, into `fd`. False once it reported.
    safe fn swr_proc_open_file(path: &ZStr, mode: &ZStr, fd: &mut c_int) -> bool;
}

/// `descriptor_type`'s `DESCRIPTOR_TYPE_PIPE` and `DESCRIPTOR_TYPE_SOCKET`.
const TYPE_PIPE: c_int = 1;
const TYPE_SOCKET: c_int = 2;

/// `get_string_parameter`: element `index` of a descriptor item as a string
/// (`zval_try_get_string`), or none once a `ValueError` or the conversion threw.
fn string_parameter(item: &Array, index: zend_ulong, name: &str) -> Option<ZString> {
    let Some(value) = item.index(index) else {
        value_error(&CString::new(format!("Missing {name}")).unwrap_or_default());
        return None;
    };
    let string = value.string();
    swoole_rs_zend::host::exception()
        .is_null()
        .then_some(string)
}

/// A descriptor the item set up: its type, mode and ends.
struct Setup {
    kind: c_int,
    mode: c_int,
    parentend: c_int,
    childend: c_int,
}

/// `set_proc_descriptor_from_array`'s choice of kind for one item, none once it
/// warned or threw.
fn descriptor(
    item: &Array,
    descriptors: Descriptors<'_>,
    ndesc: c_int,
    nindex: c_int,
    pty: &mut [c_int; 2],
) -> Option<Setup> {
    let kind = string_parameter(item, 0, "handle qualifier")?;
    let mut setup = Setup {
        kind: 0,
        mode: 0,
        parentend: 0,
        childend: 0,
    };
    let ok = match kind.bytes() {
        // Upstream's own pipe branch is compiled out: a pipe is a socket pair too.
        b"socket" | b"pipe" => {
            setup.kind = TYPE_SOCKET;
            setup_socket(&mut setup.parentend, &mut setup.childend)
        }
        b"file" => {
            let path = string_parameter(item, 1, "file name parameter for 'file'")?;
            let mode = string_parameter(item, 2, "mode parameter for 'file'")?;
            swr_proc_open_file(&path, &mode, &mut setup.childend)
        }
        b"redirect" => {
            let Some(target) = item.index(1).map(Value::referred) else {
                value_error(c"Missing redirection target");
                return None;
            };
            let Some(target) = target.as_long() else {
                let message = format!(
                    "Redirection target must be of type int, {} given",
                    target.type_name().to_string_lossy()
                );
                value_error(&CString::new(message).unwrap_or_default());
                return None;
            };
            // `(int) Z_LVAL_P(ztarget)`.
            let target = target.c_cast::<c_int>();
            let found = swr_proc_descriptor_child_of(descriptors, ndesc, target);
            setup_redirect(found, target, &mut setup.childend, nindex)
        }
        b"null" => setup_null(&mut setup.childend),
        b"pty" => {
            setup.kind = TYPE_PIPE;
            setup.mode = libc::O_RDWR;
            let [master, slave] = pty;
            setup_pty(master, slave, &mut setup.parentend, &mut setup.childend)
        }
        other => {
            // `%s` of the qualifier: its bytes up to the first NUL, whatever their encoding.
            let shown = other.split(|&b| b == 0).next().unwrap_or_default();
            warn([shown, b" is not a valid descriptor spec/mode"].concat());
            false
        }
    };
    ok.then_some(setup)
}

/// One `$descriptorspec` item (`item`, the `ndesc`-th, for child fd `nindex`) set
/// up into `setup`: its type, mode, parent end and child end. A stream resource is
/// a copy of its descriptor (`set_proc_descriptor_from_resource`); an array is
/// `set_proc_descriptor_from_array`, with `pty` holding the shared
/// pseudoterminal's ends (-1 until a `pty` item opens it). False once it warned or
/// threw, `setup` untouched.
fn descriptor_from_item(
    item: &Value,
    descriptors: Descriptors<'_>,
    ndesc: c_int,
    nindex: c_int,
    pty: &mut [c_int; 2],
    setup: &mut [c_int; 4],
) -> bool {
    let item = item.referred();
    let made = if item.kind() == IS_RESOURCE {
        let mut fd = -1;
        let mut childend = -1;
        // Upstream names the item's position, not its fd, should the copy fail.
        if !swr_proc_stream_fd(item, &mut fd) || !setup_dup(fd, &mut childend, ndesc) {
            return false;
        }
        Setup {
            kind: 0,
            mode: 0,
            parentend: 0,
            childend,
        }
    } else if let Some(array) = item.array() {
        let Some(made) = descriptor(array, descriptors, ndesc, nindex, pty) else {
            return false;
        };
        made
    } else {
        swoole_rs_zend::bailout::defer(crate::report(
            crate::Report::Fatal,
            swoole_rs_zend::E_WARNING,
            c"Descriptor item must be either an array or a File-Handle",
        ));
        return false;
    };
    *setup = [made.kind, made.mode, made.parentend, made.childend];
    true
}

/// What `proc_open.cc`'s child execs (`sw_proc_exec`: cwd, command, argv, environment),
/// passed back to it untouched.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Exec<'a>(NonNull<c_void>, PhantomData<&'a ()>);

unsafe extern "C" {
    /// Descriptor `i`'s index, type, child end, parent end and mode flags.
    safe fn swr_proc_descriptor_at(descriptors: Descriptors<'_>, i: c_int, out: &mut [c_int; 5]);
    /// In the child: `chdir` to the cwd if one was given, then exec the command with
    /// its argv or under `/bin/sh -c`, in its environment if one was given. Returns
    /// only when the exec failed, with errno set.
    safe fn swr_proc_exec(exec: Exec<'_>);
    /// Leaves the process at once, running no exit handler and no destructor.
    safe fn _exit(status: c_int) -> !;
}

/// `close_parentends_of_pipes` in the child: each pipe or socket's parent end
/// closed, and each child end moved to the descriptor the item names. False once
/// it warned.
fn move_descriptors(moves: &[[c_int; 5]]) -> bool {
    for &[index, kind, childend, parentend, _] in moves {
        if kind != 0 {
            // As upstream, a parent end that won't close is left.
            let _closed = swoole_rs_corebase::sys::close(parentend);
        }
        if childend != index {
            if swoole_rs_corebase::sys::dup2(childend, index).is_err() {
                warn(format!(
                    "Unable to copy file descriptor {childend} (for pipe) into file descriptor {index}: {}",
                    last_error()
                ));
                return false;
            }
            // As upstream, the moved end's close isn't checked.
            let _closed = swoole_rs_corebase::sys::close(childend);
        }
    }
    true
}

swoole_rs_zend::exports! {
/// `proc_open`'s fork (`swoole_fork(SW_FORK_EXEC)`). The child moves its
/// `ndesc` descriptors into place and execs `exec`, all from what was read before
/// the fork, then exits 127 if that fails. The parent gets the child's pid, or
/// false once it warned that the fork failed.
#[unsafe(no_mangle)]
extern "C" fn swr_proc_fork(
    descriptors: Descriptors<'_>,
    ndesc: c_int,
    exec: Exec<'_>,
    child: &mut libc::pid_t,
) -> bool {
    // Read before the fork: the child only closes, duplicates and execs.
    let moves: Vec<[c_int; 5]> = (0..ndesc)
        .map(|i| {
            let mut descriptor = [0; 5];
            swr_proc_descriptor_at(descriptors, i, &mut descriptor);
            descriptor
        })
        .collect();
    let fork_exec = swoole_rs_corebase::constants::ForkType::SW_FORK_EXEC as c_int;
    let pid = swoole_rs_corebase::swoole_fork(fork_exec);
    if pid == 0 {
        if !move_descriptors(&moves) {
            _exit(127);
        }
        swr_proc_exec(exec);
        warn(format!("Exec failed: {}", last_error()));
        _exit(127);
    }
    if pid < 0 {
        warn(format!("Fork failed: {}", last_error()));
        return false;
    }
    *child = pid;
    true
}
}

swoole_rs_zend::exports! {
/// `proc_open`'s walk over `$descriptorspec` into `descriptors` (one slot per
/// element): each item's child fd and setup, in order, with `pty` holding the
/// shared pseudoterminal's ends. `ndesc` counts the items set up, which the
/// caller's cleanup closes. False once a string key's `ValueError` was thrown or an
/// item warned or threw.
#[unsafe(no_mangle)]
extern "C" fn swr_proc_descriptors(
    spec: &Array,
    descriptors: Descriptors<'_>,
    pty: &mut [c_int; 2],
    ndesc: &mut c_int,
) -> bool {
    let mut entries = spec.entries();
    while let Some((key, item)) = entries.next_keyed() {
        let Key::Index(index) = key else {
            swoole_rs_zend::argument_value_error(2, c"must be an integer indexed array");
            return false;
        };
        // `(int) nindex`.
        let index = index.c_cast::<c_int>();
        let mut setup = [0; 4];
        if !descriptor_from_item(item, descriptors, *ndesc, index, pty, &mut setup) {
            return false;
        }
        swr_proc_descriptor_set(descriptors, *ndesc, index, &setup);
        *ndesc += 1;
    }
    true
}
}

unsafe extern "C" {
    /// `php_swoole_signal_isset_handler` (crates/phpprocess).
    safe fn php_swoole_signal_isset_handler(signo: c_int) -> bool;
    /// The rest of `proc_open`, in `proc_open.cc`: the descriptor array, the walk
    /// over `spec`, the fork, and the parent's streams into `pipes`, leaving the
    /// process resource or false in `out`. It takes `argv` and the environment
    /// (`efree_argv`, `_php_free_envp` or the record); `command` and `cwd` stay
    /// borrowed. It runs inside `zend_try`: true when PHP bailed out (a warning's
    /// error handler), for this frame to resume once its own values are dropped.
    safe fn swr_proc_open_spawn(
        command: &ZStr,
        argv: Option<NonNull<*mut c_char>>,
        spec: &Array,
        pipes: &Value,
        cwd: Option<&ZStr>,
        envp: Option<NonNull<c_char>>,
        envarray: Option<NonNull<*mut c_char>>,
        out: ReturnSlot<'_>,
    ) -> bool;
}

/// `proc_open(array|string $command, array $descriptor_spec, &$pipes, ?string $cwd =
/// null, ?array $env_vars = null, ?array $options = null)`: its arguments, the
/// command as argv or a shell string, the checks before it forks, and the
/// environment; the rest is `swr_proc_open_spawn`'s.
fn open(call: &Call) {
    // `Z_PARAM_ARRAY_OR_NULL` of an optional argument: none when not passed.
    let optional_array = |n| {
        if call.value(n).is_some() {
            call.array_or_null(n)
        } else {
            Ok(None)
        }
    };
    let parsed = call.arity(3, 6).and_then(|()| {
        Ok((
            call.array_or_str(1)?,
            call.array_ref(2)?,
            call.zstr_or_null(4)?,
            optional_array(5)?,
            // Win32's options only.
            optional_array(6)?,
        ))
    });
    let Ok((command, spec, cwd, environment, _options)) = parsed else {
        return;
    };
    let Some(pipes) = call.value(3) else {
        return;
    };
    // The argv stays Rust's, freed on an early return, until the spawn takes it.
    let (command, argv) = match command {
        ArrayOrStr::Array(array) => {
            if array.count() == 0 {
                call.argument_value_error(1, c"must have at least one element");
                return;
            }
            let Some((program, strings)) = command_from_array(array) else {
                call.set_bool(false);
                return;
            };
            (program, Some(strings))
        }
        ArrayOrStr::Str(string) => (string.to_owned(), None),
    };
    swoole_rs_phpswoole::check_reactor();
    if php_swoole_signal_isset_handler(libc::SIGCHLD) {
        swoole_rs_zend::bailout::defer(crate::report(
            crate::Report::Swoole,
            swoole_rs_zend::E_WARNING,
            c"The signal [SIGCHLD] is registered, cannot execute swoole_proc_open",
        ));
        call.set_bool(false);
        return;
    }
    if let Err(bailout) = crate::coroutine_check() {
        bailout.defer();
        return;
    }
    let (mut envp, mut envarray) = (ptr::null_mut(), ptr::null_mut());
    if let Some(environment) = environment.as_deref().and_then(Value::array) {
        array_to_envp(environment, &mut envp, &mut envarray);
    }
    let bailed = swr_proc_open_spawn(
        &command,
        argv.and_then(|strings| NonNull::new(argv_to_c(strings))),
        spec,
        pipes,
        cwd,
        NonNull::new(envp),
        NonNull::new(envarray),
        call.return_slot(),
    );
    if bailed {
        // Resumed once `open` returns, after `command` and the arguments drop.
        swoole_rs_zend::bailout::Bailout.defer();
    }
}

unsafe extern "C" {
    /// `php_swoole_create_stream_from_pipe(fd, mode)`: a stream that takes `fd`, or
    /// none.
    safe fn swr_proc_pipe_stream(fd: c_int, mode: Option<Text<'_>>) -> Option<NonNull<php_stream>>;
    /// Records `stream` as `proc`'s pipe `i` and adds it to `pipes` under `index`
    /// (unseekable, as upstream marks it), or records none for no stream.
    safe fn swr_proc_attach(
        proc: &Process<'_>,
        i: c_int,
        stream: Option<NonNull<php_stream>>,
        pipes: &Value,
        index: c_int,
    );
}

swoole_rs_zend::exports! {
/// `proc_open`'s parent once the child forked: each descriptor's child end closed,
/// and a pipe's or socket's parent end opened as a stream in `pipes` and recorded in
/// `proc`.
#[unsafe(no_mangle)]
extern "C" fn swr_proc_parent(
    descriptors: Descriptors<'_>,
    ndesc: c_int,
    proc: &Process<'_>,
    pipes: &Value,
) {
    for i in 0..ndesc {
        let mut descriptor = [0; 5];
        swr_proc_descriptor_at(descriptors, i, &mut descriptor);
        let [index, kind, childend, parentend, mode] = descriptor;
        // As upstream, a child end that won't close is left.
        let _closed = swoole_rs_corebase::sys::close(childend);
        let stream = match kind {
            TYPE_PIPE => {
                let mode = match mode {
                    libc::O_WRONLY => Some(c"w"),
                    libc::O_RDONLY => Some(c"r"),
                    libc::O_RDWR => Some(c"r+"),
                    _ => None,
                };
                swr_proc_pipe_stream(parentend, mode.map(Text::from))
            }
            TYPE_SOCKET => crate::stream::socket_stream(parentend, libc::AF_UNIX, libc::SOCK_STREAM, 0),
            _ => None,
        };
        swr_proc_attach(proc, i, stream, pipes, index);
    }
}
}

swoole_rs_zend::exports! {
/// `proc_co_rsrc_dtor`'s wait once the pipes are closed: a still-running child is
/// reaped (`_co_waitpid`) into `wstatus`, 0 otherwise.
#[unsafe(no_mangle)]
extern "C" fn swr_proc_reap(running: bool, child: libc::pid_t, wstatus: &mut c_int) {
    *wstatus = 0;
    if running {
        waitpid(child, wstatus, 0);
    }
}
}
