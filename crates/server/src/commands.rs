//! `Server`'s commands: `add_command`, `command` and the tables behind them (the
//! named commands, their handlers and the callbacks of requests awaiting a reply).
//! A handler or callback is a function with its context, which its owner keeps
//! alive for as long as the server can call it. Native code still invokes a
//! handler whose reply it writes back to the requester.
use crate::ops::{
    EVENT_COMMAND_RESPONSE, FIELD_REACTOR_NUM, FIELD_TASK_WORKER_NUM, FIELD_WORKER_NUM, OP_FIELD,
    OP_IS_STARTED, OP_MODE, OP_RT_HAS_PIPE_COMMAND, OP_ST_CMD_SEND, OP_ST_PIPE_COMMAND_NEW,
};
use crate::{Host, MODE_BASE, MODE_PROCESS, ServerState};
use core::ffi::{CStr, c_char, c_int, c_void};
use std::collections::BTreeMap;
use std::sync::{MutexGuard, PoisonError};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::command::{
    CommandInput, LOG_INVALID_TASK_WORKER, LOG_INVALID_THREAD, LOG_INVALID_WORKER, LOG_NOT_MASTER,
    LOG_NOT_WORKER_0, LOG_TOO_LARGE, LOG_UNKNOWN, LOG_UNSUPPORTED_MODE, LOG_UNSUPPORTED_TYPE,
    TO_MASTER, route,
};

const SW_LOG_NOTICE: c_int = 3;
const SW_LOG_ERROR: c_int = 5;
/// `SW_MASTER`.
const SW_MASTER: c_int = 1;
const IPC_BUFFER_SIZE: usize = swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE;

/// Writes a handler's reply into `out`.
pub type CommandReply = unsafe extern "C" fn(out: *mut c_void, data: *const c_char, length: usize);
/// `Server::Command::Handler`: answers `message` through `reply(out, ...)`.
type Handler = unsafe extern "C" fn(
    ctx: *mut c_void,
    serv: *mut c_void,
    message: *const c_char,
    length: usize,
    reply: CommandReply,
    out: *mut c_void,
);
/// `Server::Command::Callback`: takes the reply to a request.
type Callback = unsafe extern "C" fn(
    ctx: *mut c_void,
    serv: *mut c_void,
    message: *const c_char,
    length: usize,
);

/// `swr_v1_server_command_fn`: a `Handler` or `Callback` with its context.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CommandFn {
    pub invoke: *const c_void,
    pub ctx: *mut c_void,
}

// SAFETY: the tables only hold these; the owner of `ctx` decides where it is called.
unsafe impl Send for CommandFn {}

struct Command {
    id: i32,
    accepted_types: i32,
}

pub(crate) struct Commands {
    named: BTreeMap<Vec<u8>, Command>,
    handlers: BTreeMap<i32, CommandFn>,
    callbacks: BTreeMap<i64, CommandFn>,
    next_request: i64,
}

impl Commands {
    pub(crate) const fn new() -> Self {
        Self {
            named: BTreeMap::new(),
            handlers: BTreeMap::new(),
            callbacks: BTreeMap::new(),
            next_request: 1,
        }
    }
}

fn commands(state: &ServerState) -> MutexGuard<'_, Commands> {
    state
        .commands
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CommandInfo {
    pub name: *const u8,
    pub length: usize,
    pub id: i32,
    pub accepted_types: i32,
}

/// The command at `index` in name order, for the admin server's listing.
///
/// # Safety
/// The server is live; `output` is writable. The name is borrowed until the server
/// is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_at(
    state: *const ServerState,
    index: usize,
    output: *mut CommandInfo,
) -> bool {
    // SAFETY: the caller lends a live server.
    let commands = commands(unsafe { &*state });
    let Some((name, command)) = commands.named.iter().nth(index) else {
        return false;
    };
    // SAFETY: the caller supplies writable output and the table owns this name.
    unsafe {
        output.write(CommandInfo {
            name: name.as_ptr(),
            length: name.len(),
            id: command.id,
            accepted_types: command.accepted_types,
        });
    };
    true
}

/// The handler of command `id`, if there is one.
///
/// # Safety
/// `state` is live and `output` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_handler(
    state: *const ServerState,
    id: i32,
    output: *mut CommandFn,
) -> bool {
    // SAFETY: the caller lends a live server.
    let Some(handler) = commands(unsafe { &*state }).handlers.get(&id).copied() else {
        return false;
    };
    // SAFETY: per the caller.
    unsafe { output.write(handler) };
    true
}

/// `Server::call_command_callback`: the reply to `request_id` goes to its callback,
/// which is then forgotten.
///
/// # Safety
/// `state` is live, `serv` its server, and `message` holds `length` bytes. The
/// callback may resume a coroutine: this frame owns nothing that needs a drop then.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_result(
    state: *const ServerState,
    serv: *mut c_void,
    request_id: i64,
    message: *const c_char,
    length: usize,
) {
    // SAFETY: the caller lends a live server.
    let Some(callback) = commands(unsafe { &*state }).callbacks.remove(&request_id) else {
        let m = format_args!("Invalid command result[request_id={request_id}]");
        return error(
            SW_LOG_ERROR,
            E::SW_ERROR_SERVER_INVALID_COMMAND,
            c"Server::call_command_callback",
            m,
        );
    };
    // SAFETY: a stored callback, called with its context and the caller's reply.
    unsafe { call(callback, serv, message, length) };
}

/// `MessageBus::get_packet()`: the packet the bus just reassembled.
#[repr(C)]
struct BusPacket {
    length: usize,
    data: *const c_char,
}

// SAFETY: crates/protocols' bus, linked into the same archive.
unsafe extern "C" {
    fn swr_v1_bus_buffer(bus: *mut c_void) -> *mut DataHead;
    fn swr_v1_bus_packet(bus: *mut c_void) -> BusPacket;
    fn swr_v1_bus_write(
        bus: *mut c_void,
        socket: *mut c_void,
        head: *mut DataHead,
        data: *const u8,
    ) -> bool;
}

/// `Server::call_command_handler(mb, worker_id, sock)`: answers the command request
/// in `mb`'s buffer with its handler and writes the reply back over `sock`.
///
/// # Safety
/// `serv` is a live `Server`, `mb` a live `MessageBus` holding a command request
/// packet, and `sock` the pipe the request came from.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole6Server20call_command_handlerERNS_10MessageBusEtPNS_7network6SocketE"
    )
)]
pub unsafe extern "C-unwind" fn call_command_handler(
    serv: *mut c_void,
    mb: *mut c_void,
    worker_id: u16,
    sock: *mut c_void,
) {
    // SAFETY: `MessageBus` starts with its `swr_bus *`.
    let bus = unsafe { mb.cast::<*mut c_void>().read() };
    // SAFETY: the caller's bus, with its buffer set.
    let buffer = unsafe { swr_v1_bus_buffer(bus) };
    // SAFETY: the live buffer, holding the request's header.
    let request = unsafe { buffer.read() };
    let command_id = i32::from(request.server_fd);
    // SAFETY: per the caller.
    let state = unsafe { crate::view::state_ptr(serv) };
    // SAFETY: the state lives as long as the server.
    let Some(handler) = commands(unsafe { &*state })
        .handlers
        .get(&command_id)
        .copied()
    else {
        let m = format_args!("Unknown command[command_id={command_id}]");
        let origin = c"Server::call_command_handler";
        return error(SW_LOG_ERROR, E::SW_ERROR_SERVER_INVALID_COMMAND, origin, m);
    };
    // SAFETY: the bus's reassembled packet.
    let packet = unsafe { swr_v1_bus_packet(bus) };
    // SAFETY: `invoke` is a `Handler`, as `add_command` stored it.
    let invoke = unsafe { core::mem::transmute::<*const c_void, Handler>(handler.invoke) };
    let mut reply = Vec::<u8>::new();
    let out = (&raw mut reply).cast();
    // SAFETY: a stored handler with its context, over the packet's bytes.
    unsafe {
        invoke(
            handler.ctx,
            serv,
            packet.data,
            packet.length,
            reply_into,
            out,
        );
    }
    #[expect(
        clippy::cast_possible_wrap,
        clippy::cast_possible_truncation,
        reason = "DataHead's reactor_id and len are upstream's int16_t and uint32_t"
    )]
    let mut head = DataHead {
        fd: request.fd,
        reactor_id: worker_id as i16,
        server_fd: u16::MAX,
        kind: EVENT_COMMAND_RESPONSE as u8,
        len: reply.len() as u32,
        ..DataHead::default()
    };
    // SAFETY: the caller's bus and pipe, and a header over the reply's bytes.
    unsafe { swr_v1_bus_write(bus, sock, &raw mut head, reply.as_ptr()) };
}

/// # Safety
/// `callback` is a live `Callback` with its context.
unsafe fn call(callback: CommandFn, serv: *mut c_void, message: *const c_char, length: usize) {
    // SAFETY: `invoke` is a `Callback`, as `swr_v1_server_command` stored it.
    let invoke = unsafe { core::mem::transmute::<*const c_void, Callback>(callback.invoke) };
    // SAFETY: per the caller.
    unsafe { invoke(callback.ctx, serv, message, length) };
}

fn error(level: c_int, code: E, origin: &CStr, message: core::fmt::Arguments<'_>) {
    log::error_log(level, code as c_int, origin, message);
}

/// `Server::add_command`'s arguments.
pub(crate) struct AddCommand<'a> {
    pub name: &'a [u8],
    pub accepted_types: i32,
    pub handler: CommandFn,
}

/// `Server::add_command`: before start, a new name, and (outside base mode) a command
/// pipe, made once. Ids count from 1.
pub(crate) fn add_command<H: Host>(h: &mut H, add: &AddCommand<'_>) -> crate::Effect<bool> {
    if h.is(OP_IS_STARTED)? || commands(h.state()?).named.contains_key(add.name) {
        return Ok(false);
    }
    if h.get(OP_MODE)? != MODE_BASE
        && !h.is(OP_RT_HAS_PIPE_COMMAND)?
        && !h.is(OP_ST_PIPE_COMMAND_NEW)?
    {
        return Ok(false);
    }
    let state = h.state()?;
    let id = state
        .next_command
        .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    let mut commands = commands(state);
    commands.handlers.insert(id, add.handler);
    let command = Command {
        id,
        accepted_types: add.accepted_types,
    };
    commands.named.insert(add.name.to_vec(), command);
    Ok(true)
}

/// `Server::command`'s arguments.
pub(crate) struct SendCommand<'a> {
    pub serv: *mut c_void,
    pub process_id: u32,
    pub process_type: i32,
    pub name: &'a [u8],
    pub message: &'a [u8],
    pub callback: CommandFn,
}

/// `swr_v1_server_command_send`: what `OP_ST_CMD_SEND` writes to the target process.
#[repr(C)]
struct CommandSend {
    /// `swr_v1_process_command_route::action`.
    action: i32,
    process_id: u32,
    command_id: i32,
    request_id: i64,
    message: *const u8,
    length: usize,
}

unsafe extern "C" fn reply_into(out: *mut c_void, data: *const c_char, length: usize) {
    // SAFETY: the handler lends `length` bytes.
    let data = unsafe { core::slice::from_raw_parts(data.cast(), length) };
    // SAFETY: `command` lends its reply buffer.
    unsafe { (*out.cast::<Vec<u8>>()).extend_from_slice(data) };
}

/// `Server::command`: sends `message` to command `name` in the given process, whose
/// reply goes to `callback`. The master answers its own commands at once.
pub(crate) fn command<H: Host>(h: &mut H, send: &SendCommand<'_>) -> crate::Effect<bool> {
    let started = h.is(OP_IS_STARTED)?;
    let mode = h.get(OP_MODE)?;
    let found = commands(h.state()?)
        .named
        .get(send.name)
        .map(|c| (c.id, c.accepted_types));
    // SAFETY: this thread's worker globals; only the field is read.
    let worker = unsafe { (*swoole_rs_process::worker_globals::swr_v1_worker_globals()).worker };
    let input = CommandInput {
        started,
        found: found.is_some(),
        process_mode: mode == MODE_PROCESS,
        base_mode: mode == MODE_BASE,
        master: swoole_rs_process::methods::swoole_get_worker_type() == SW_MASTER,
        accepted_types: found.map_or(0, |(_, types)| types),
        worker_id: if mode == MODE_BASE && !worker.is_null() {
            // SAFETY: this process's current worker.
            unsafe { swoole_rs_process::worker_record::swr_v1_worker_id(worker) }
        } else {
            0
        },
        process_id: send.process_id,
        process_type: send.process_type,
        reactor_num: field(h, FIELD_REACTOR_NUM)?,
        worker_num: field(h, FIELD_WORKER_NUM)?,
        task_worker_num: field(h, FIELD_TASK_WORKER_NUM)?,
        length: send.message.len(),
        manager_capacity: IPC_BUFFER_SIZE,
    };
    let route = route(&input);
    log_route(route.log, route.process_id, send);
    let Some((command_id, _)) = found.filter(|_| route.request) else {
        return Ok(false);
    };
    let request_id = {
        let mut commands = commands(h.state()?);
        let id = commands.next_request;
        commands.next_request = id.wrapping_add(1);
        commands.callbacks.insert(id, send.callback);
        id
    };
    if route.action == TO_MASTER {
        let mut reply = Vec::<u8>::new();
        let handler = commands(h.state()?).handlers.get(&command_id).copied();
        match handler {
            Some(handler) => {
                // SAFETY: `invoke` is a `Handler`, as `add_command` stored it.
                let invoke =
                    unsafe { core::mem::transmute::<*const c_void, Handler>(handler.invoke) };
                let (message, out) = (send.message, (&raw mut reply).cast());
                // SAFETY: a stored handler, with its context and this reply buffer.
                unsafe {
                    invoke(
                        handler.ctx,
                        send.serv,
                        message.as_ptr().cast(),
                        message.len(),
                        reply_into,
                        out,
                    );
                };
            }
            None => error(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_INVALID_COMMAND,
                c"Server::call_command_handler_in_master",
                format_args!("Unknown command[{command_id}]"),
            ),
        }
        // SAFETY: the caller's callback, with the master's reply.
        unsafe { call(send.callback, send.serv, reply.as_ptr().cast(), reply.len()) };
        return Ok(true);
    }
    let message = CommandSend {
        action: route.action,
        process_id: route.process_id,
        command_id,
        request_id,
        message: send.message.as_ptr(),
        length: send.message.len(),
    };
    let sent = route.action != swoole_rs_process::command::FAIL
        && h.op(
            OP_ST_CMD_SEND,
            core::ptr::from_ref(&message).expose_provenance() as u64,
        )? != 0;
    if !sent {
        commands(h.state()?).callbacks.remove(&request_id);
    }
    Ok(sent)
}

fn field<H: Host>(h: &mut H, field: u32) -> crate::Effect<u32> {
    Ok(u32::try_from(h.op(OP_FIELD, field.into())?).unwrap_or(u32::MAX))
}

fn log_route(log: i32, process_id: u32, send: &SendCommand<'_>) {
    let origin = c"Server::command";
    let notice = |code, m| error(SW_LOG_NOTICE, code, origin, m);
    match log {
        LOG_UNKNOWN => notice(
            E::SW_ERROR_SERVER_INVALID_COMMAND,
            format_args!("Unknown command[{}]", String::from_utf8_lossy(send.name)),
        ),
        LOG_NOT_MASTER => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("command() can only be used in master process"),
        ),
        LOG_NOT_WORKER_0 => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("command() can only be used in worker process 0"),
        ),
        LOG_UNSUPPORTED_TYPE => notice(
            E::SW_ERROR_OPERATION_NOT_SUPPORT,
            format_args!("unsupported [process_type]"),
        ),
        LOG_UNSUPPORTED_MODE => notice(
            E::SW_ERROR_OPERATION_NOT_SUPPORT,
            format_args!("unsupported [server_mode]"),
        ),
        LOG_INVALID_THREAD => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("invalid thread_id[{process_id}]"),
        ),
        LOG_INVALID_WORKER => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("invalid worker_id[{process_id}]"),
        ),
        LOG_INVALID_TASK_WORKER => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("invalid task_worker_id[{process_id}]"),
        ),
        LOG_TOO_LARGE => notice(
            E::SW_ERROR_DATA_LENGTH_TOO_LARGE,
            format_args!(
                "message is too large, maximum length is {IPC_BUFFER_SIZE}, the given length is {}",
                send.message.len()
            ),
        ),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{OP_IS_STARTED, OP_MODE, OP_RT_HAS_PIPE_COMMAND, OP_ST_PIPE_COMMAND_NEW};
    use crate::script::Script;

    unsafe extern "C" fn echo(
        ctx: *mut c_void,
        _: *mut c_void,
        message: *const c_char,
        length: usize,
        reply: CommandReply,
        out: *mut c_void,
    ) {
        // SAFETY: the test's prefix and the caller's message.
        unsafe {
            let prefix = CStr::from_ptr(ctx.cast());
            reply(out, prefix.as_ptr(), prefix.count_bytes());
            reply(out, message, length);
        }
    }

    unsafe extern "C" fn keep(
        ctx: *mut c_void,
        _: *mut c_void,
        message: *const c_char,
        length: usize,
    ) {
        // SAFETY: the test's buffer and the caller's message.
        unsafe {
            *ctx.cast::<Vec<u8>>() = core::slice::from_raw_parts(message.cast(), length).to_vec();
        };
    }

    fn handler(prefix: &'static CStr) -> CommandFn {
        CommandFn {
            invoke: echo as *const c_void,
            ctx: prefix.as_ptr().cast_mut().cast(),
        }
    }

    fn add(h: &mut Script, name: &[u8], types: i32) -> bool {
        let add = AddCommand {
            name,
            accepted_types: types,
            handler: handler(c"re:"),
        };
        add_command(h, &add).unwrap()
    }

    #[test]
    fn commands_need_a_pipe_outside_base_mode() {
        let mut base = Script::with(&[(OP_MODE, MODE_BASE)]);
        assert!(add(&mut base, b"a", 2));
        assert!(add(&mut base, b"b", 2));
        assert!(!add(&mut base, b"a", 2), "names are unique");
        let ids: Vec<_> = commands(&base.state).named.values().map(|c| c.id).collect();
        assert_eq!(ids, vec![1, 2], "ids count from 1");
        assert_eq!(base.called(OP_ST_PIPE_COMMAND_NEW), 0);

        let mut process = Script::with(&[(OP_MODE, MODE_PROCESS)]);
        assert!(!add(&mut process, b"a", 2), "the pipe failed");
        assert!(commands(&process.state).named.is_empty());
        let mut process = Script::with(&[(OP_MODE, MODE_PROCESS), (OP_RT_HAS_PIPE_COMMAND, 1)]);
        assert!(add(&mut process, b"a", 2));
        assert_eq!(process.called(OP_ST_PIPE_COMMAND_NEW), 0, "made once");

        let mut started = Script::with(&[(OP_MODE, MODE_BASE), (OP_IS_STARTED, 1)]);
        assert!(!add(&mut started, b"a", 2));
    }

    #[test]
    fn commands_keep_binary_names_for_the_listing() {
        let mut s = Script::with(&[(OP_MODE, MODE_BASE)]);
        let name = b"admin\0command";
        assert!(add(&mut s, name, 62));
        let mut info = CommandInfo::default();
        unsafe {
            assert!(swr_v1_server_command_at(
                &raw const s.state,
                0,
                &raw mut info
            ));
            assert_eq!((info.id, info.accepted_types), (1, 62));
            assert_eq!(core::slice::from_raw_parts(info.name, info.length), name);
            assert!(!swr_v1_server_command_at(
                &raw const s.state,
                1,
                &raw mut info
            ));
            let mut found = CommandFn {
                invoke: core::ptr::null(),
                ctx: core::ptr::null_mut(),
            };
            assert!(swr_v1_server_command_handler(
                &raw const s.state,
                1,
                &raw mut found
            ));
            assert_eq!(found.invoke, echo as *const c_void);
            assert!(!swr_v1_server_command_handler(
                &raw const s.state,
                2,
                &raw mut found
            ));
        }
    }

    fn send<'a>(name: &'a [u8], process_type: i32, out: &mut Vec<u8>) -> SendCommand<'a> {
        SendCommand {
            serv: core::ptr::null_mut(),
            process_id: 0,
            process_type,
            name,
            message: b"ping",
            callback: CommandFn {
                invoke: keep as *const c_void,
                ctx: core::ptr::from_mut(out).cast(),
            },
        }
    }

    const MASTER: i32 = swoole_rs_process::command::MASTER;
    const EVENT_WORKER: i32 = swoole_rs_process::command::EVENT_WORKER;

    #[test]
    fn a_base_master_answers_its_own_commands_at_once() {
        let mut s = Script::with(&[(OP_MODE, MODE_BASE)]);
        let add = AddCommand {
            name: b"hi",
            accepted_types: MASTER | EVENT_WORKER,
            handler: handler(c"re:"),
        };
        assert!(add_command(&mut s, &add).unwrap());
        s.facts.insert(OP_IS_STARTED, 1);
        let mut out = Vec::new();
        // Base mode's worker 0 is the master.
        assert!(command(&mut s, &send(b"hi", EVENT_WORKER, &mut out)).unwrap());
        assert_eq!(out, b"re:ping");
        assert_eq!(s.called(OP_ST_CMD_SEND), 0);
    }

    #[test]
    fn a_failed_send_forgets_its_callback_and_unknown_names_take_no_request() {
        let mut s = Script::with(&[(OP_MODE, MODE_BASE)]);
        let add = AddCommand {
            name: b"hi",
            accepted_types: EVENT_WORKER,
            handler: handler(c""),
        };
        assert!(add_command(&mut s, &add).unwrap());
        s.facts.extend([(OP_IS_STARTED, 1), (OP_FIELD, 4)]);
        let mut out = Vec::new();
        let mut to_worker = send(b"hi", EVENT_WORKER, &mut out);
        to_worker.process_id = 1;
        assert!(!command(&mut s, &to_worker).unwrap(), "the send failed");
        assert_eq!(s.called(OP_ST_CMD_SEND), 1);
        assert!(commands(&s.state).callbacks.is_empty());
        assert_eq!(commands(&s.state).next_request, 2);
        assert!(!command(&mut s, &send(b"nope", EVENT_WORKER, &mut out)).unwrap());
        assert_eq!(commands(&s.state).next_request, 2, "no request id taken");
        s.facts.insert(OP_ST_CMD_SEND, 1);
        assert!(command(&mut s, &to_worker).unwrap());
        assert_eq!(
            commands(&s.state)
                .callbacks
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![2]
        );
        unsafe {
            swr_v1_server_command_result(
                &raw const s.state,
                core::ptr::null_mut(),
                2,
                c"pong".as_ptr(),
                4,
            );
        }
        assert_eq!(out, b"pong");
        assert!(
            commands(&s.state).callbacks.is_empty(),
            "a reply is taken once"
        );
    }
}
