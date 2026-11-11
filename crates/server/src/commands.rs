//! `Server`'s commands: `add_command`, `command` and the tables behind them (the
//! named commands, their handlers and the callbacks of requests awaiting a reply).
//! A handler or callback is a Rust closure, which the tables keep for as long as
//! the server can call it.
use crate::ops::{
    FIELD_REACTOR_NUM, FIELD_TASK_WORKER_NUM, FIELD_WORKER_NUM, OP_FIELD, OP_IS_STARTED, OP_MODE,
    OP_RT_HAS_PIPE_COMMAND, OP_ST_CMD_SEND, OP_ST_PIPE_COMMAND_NEW,
};
use crate::{Host, MODE_BASE, MODE_PROCESS, ServerState};
use core::ffi::{CStr, c_int};
use std::collections::BTreeMap;
use std::sync::{Arc, MutexGuard, PoisonError};
use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::command::{
    CommandInput, LOG_INVALID_TASK_WORKER, LOG_INVALID_THREAD, LOG_INVALID_WORKER, LOG_NOT_MASTER,
    LOG_NOT_WORKER_0, LOG_TOO_LARGE, LOG_UNKNOWN, LOG_UNSUPPORTED_MODE, LOG_UNSUPPORTED_TYPE,
    TO_MASTER, route,
};
use swoole_rs_process::worker_record::Worker;
#[cfg(all(feature = "upstream-symbols", not(test)))]
use {crate::ops::EVENT_COMMAND_RESPONSE, swoole_rs_corebase::DataHead, swoole_rs_reactor::Socket};

const SW_LOG_NOTICE: c_int = 3;
const SW_LOG_ERROR: c_int = 5;
/// `SW_MASTER`.
const SW_MASTER: c_int = 1;
const IPC_BUFFER_SIZE: usize = swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE;

/// `Server::Command::Handler`: answers `message` to the server by appending its
/// reply to the buffer.
pub type CommandHandler = Arc<dyn Fn(crate::view::Server<'_>, &[u8], &mut Vec<u8>)>;
/// `Server::Command::Callback`: takes the reply to a request.
pub type CommandCallback = Arc<dyn Fn(&[u8])>;

struct Command {
    id: i32,
    accepted_types: i32,
}

pub(crate) struct Commands {
    named: BTreeMap<Vec<u8>, Command>,
    handlers: BTreeMap<i32, CommandHandler>,
    callbacks: BTreeMap<i64, CommandCallback>,
    next_request: i64,
}

// SAFETY: the tables only hold the closures, as upstream's `std::function`s: each
// runs on the thread its owner gives it to the server from or answers commands on,
// and the lock keeps the tables themselves apart.
unsafe impl Send for Commands {}

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

/// Each command's name, id and accepted process types, in name order, for the
/// admin server's listing.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn listing(state: &ServerState) -> Vec<(Vec<u8>, i32, i32)> {
    let commands = commands(state);
    let named = commands.named.iter();
    named
        .map(|(name, command)| (name.clone(), command.id, command.accepted_types))
        .collect()
}

/// The handler of command `id`, if there is one.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn handler(state: &ServerState, id: i32) -> Option<CommandHandler> {
    commands(state).handlers.get(&id).cloned()
}

/// `Server::call_command_callback`: the reply to `request_id` goes to its callback,
/// which is then forgotten. The callback may resume a coroutine: this frame owns
/// nothing that needs a drop then.
pub(crate) fn command_result(state: &ServerState, request_id: i64, message: &[u8]) {
    let Some(callback) = commands(state).callbacks.remove(&request_id) else {
        let m = format_args!("Invalid command result[request_id={request_id}]");
        return error(
            SW_LOG_ERROR,
            E::SW_ERROR_SERVER_INVALID_COMMAND,
            c"Server::call_command_callback",
            m,
        );
    };
    callback(message);
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
/// `Server::call_command_handler(bus, worker_id, sock)`: answers the command request
/// in `bus`'s buffer with its handler and writes the reply back over `sock`, the
/// pipe the request came from.
pub(crate) fn call_command_handler(
    serv: crate::view::Server<'_>,
    bus: crate::objects::Bus,
    worker_id: u16,
    sock: Option<Socket<'_>>,
) {
    let request = bus.head();
    let command_id = i32::from(request.server_fd);
    let state = serv.state();
    let Some(handler) = handler(state, command_id) else {
        let m = format_args!("Unknown command[command_id={command_id}]");
        let origin = c"Server::call_command_handler";
        return error(SW_LOG_ERROR, E::SW_ERROR_SERVER_INVALID_COMMAND, origin, m);
    };
    let Some(sock) = sock else {
        return;
    };
    bus.with_packet(|message| answer(&handler, serv, message, (bus, sock), request.fd, worker_id));
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
/// `command_answer(handler, ...)`, written back as `SW_SERVER_EVENT_COMMAND_RESPONSE`
/// to request `fd` from `reactor_id`: whether `bus` wrote it to `sock`.
pub(crate) fn answer(
    handler: &CommandHandler,
    serv: crate::view::Server<'_>,
    message: &[u8],
    (bus, sock): (crate::objects::Bus, Socket<'_>),
    fd: i64,
    reactor_id: u16,
) -> bool {
    let mut reply = Vec::<u8>::new();
    handler(serv, message, &mut reply);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "SW_SERVER_EVENT_COMMAND_RESPONSE is 18"
    )]
    let mut head = DataHead {
        fd,
        reactor_id: reactor_id.cast_signed(),
        server_fd: u16::MAX,
        kind: EVENT_COMMAND_RESPONSE as u8,
        ..DataHead::default()
    };
    bus.write(sock, &mut head, &reply)
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
/// `serv->get_command_reply_socket()`: worker 0's master pipe in base mode, else the
/// command pipe's worker end.
pub(crate) fn reply_socket<'a>(
    state: &ServerState,
    serv: crate::view::Server<'a>,
) -> Option<Socket<'a>> {
    let config = state.config_ref();
    if u64::from(config.mode_) == MODE_BASE {
        return serv.worker_pipe(0, true);
    }
    let pair = config
        .pipe_command
        .cast::<swoole_rs_tls::socket_pair::SocketPair>();
    // SAFETY: the server's live command pipe, a `UnixSocket`.
    crate::objects::socket(unsafe { (*pair).worker_socket }.cast())
}

fn error(level: c_int, code: E, origin: &CStr, message: core::fmt::Arguments<'_>) {
    log::error_log(level, code as c_int, origin, message);
}

/// `Server::add_command`'s arguments.
pub(crate) struct AddCommand<'a> {
    pub name: &'a [u8],
    pub accepted_types: i32,
    pub handler: CommandHandler,
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
    commands.handlers.insert(id, add.handler.clone());
    let command = Command {
        id,
        accepted_types: add.accepted_types,
    };
    commands.named.insert(add.name.to_vec(), command);
    Ok(true)
}

/// `Server::command`'s arguments.
pub(crate) struct SendCommand<'a> {
    pub serv: crate::view::Server<'a>,
    pub process_id: u32,
    pub process_type: i32,
    pub name: &'a [u8],
    pub message: &'a [u8],
    pub callback: CommandCallback,
}

/// `swr_v1_server_command_send`: what `OP_ST_CMD_SEND` writes to the target process.
#[repr(C)]
pub(crate) struct CommandSend {
    /// `CommandRoute::action`.
    pub(crate) action: i32,
    pub(crate) process_id: u32,
    pub(crate) command_id: i32,
    pub(crate) request_id: i64,
    pub(crate) message: *const u8,
    pub(crate) length: usize,
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
    let worker = unsafe { (*swoole_rs_process::worker_globals::thread_globals()).worker };
    let input = CommandInput {
        started,
        found: found.is_some(),
        process_mode: mode == MODE_PROCESS,
        base_mode: mode == MODE_BASE,
        master: swoole_rs_process::methods::swoole_get_worker_type() == SW_MASTER,
        accepted_types: found.map_or(0, |(_, types)| types),
        worker_id: if mode == MODE_BASE && !worker.is_null() {
            // SAFETY: this process's current worker.
            unsafe { Worker::new(worker).id() }
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
        commands.callbacks.insert(id, send.callback.clone());
        id
    };
    if route.action == TO_MASTER {
        let mut reply = Vec::<u8>::new();
        let handler = commands(h.state()?).handlers.get(&command_id).cloned();
        match handler {
            Some(handler) => handler(send.serv, send.message, &mut reply),
            None => error(
                SW_LOG_ERROR,
                E::SW_ERROR_SERVER_INVALID_COMMAND,
                c"Server::call_command_handler_in_master",
                format_args!("Unknown command[{command_id}]"),
            ),
        }
        (send.callback)(&reply);
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

    fn handler(prefix: &'static [u8]) -> CommandHandler {
        Arc::new(move |_, message, reply| {
            reply.extend_from_slice(prefix);
            reply.extend_from_slice(message);
        })
    }

    /// A server none of these handlers reads.
    fn no_server() -> crate::view::Server<'static> {
        // SAFETY: the handlers here never read their server.
        unsafe { crate::view::Server::from_raw(core::ptr::null_mut()) }
    }

    fn add(h: &mut Script, name: &[u8], types: i32) -> bool {
        let add = AddCommand {
            name,
            accepted_types: types,
            handler: handler(b"re:"),
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
        assert_eq!(listing(&s.state), [(name.to_vec(), 1, 62)]);
        let mut reply = Vec::new();
        super::handler(&s.state, 1).unwrap()(no_server(), b"x", &mut reply);
        assert_eq!(reply, b"re:x");
        assert!(super::handler(&s.state, 2).is_none());
    }

    type Out = Arc<std::sync::Mutex<Vec<u8>>>;

    fn send<'a>(name: &'a [u8], process_type: i32, out: &Out) -> SendCommand<'a> {
        let out = out.clone();
        SendCommand {
            serv: no_server(),
            process_id: 0,
            process_type,
            name,
            message: b"ping",
            callback: Arc::new(move |message| *out.lock().unwrap() = message.to_vec()),
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
            handler: handler(b"re:"),
        };
        assert!(add_command(&mut s, &add).unwrap());
        s.facts.insert(OP_IS_STARTED, 1);
        let out = Out::default();
        // Base mode's worker 0 is the master.
        assert!(command(&mut s, &send(b"hi", EVENT_WORKER, &out)).unwrap());
        assert_eq!(*out.lock().unwrap(), b"re:ping");
        assert_eq!(s.called(OP_ST_CMD_SEND), 0);
    }

    #[test]
    fn a_failed_send_forgets_its_callback_and_unknown_names_take_no_request() {
        let mut s = Script::with(&[(OP_MODE, MODE_BASE)]);
        let add = AddCommand {
            name: b"hi",
            accepted_types: EVENT_WORKER,
            handler: handler(b""),
        };
        assert!(add_command(&mut s, &add).unwrap());
        s.facts.extend([(OP_IS_STARTED, 1), (OP_FIELD, 4)]);
        let out = Out::default();
        let mut to_worker = send(b"hi", EVENT_WORKER, &out);
        to_worker.process_id = 1;
        assert!(!command(&mut s, &to_worker).unwrap(), "the send failed");
        assert_eq!(s.called(OP_ST_CMD_SEND), 1);
        assert!(commands(&s.state).callbacks.is_empty());
        assert_eq!(commands(&s.state).next_request, 2);
        assert!(!command(&mut s, &send(b"nope", EVENT_WORKER, &out)).unwrap());
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
        command_result(&s.state, 2, b"pong");
        assert_eq!(*out.lock().unwrap(), b"pong");
        assert!(
            commands(&s.state).callbacks.is_empty(),
            "a reply is taken once"
        );
    }
}
