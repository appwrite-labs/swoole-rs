//! `Server`'s commands: `add_command`, `command` and the tables behind them (the
//! named commands, their handlers and the callbacks of requests awaiting a reply).
//! A handler or callback is a Rust closure, which the tables keep for as long as
//! the server can call it.
use crate::hybrid::hybrid;
use crate::view::Server;
use crate::{Mode, ServerState};
use core::ffi::{CStr, c_int};
use std::collections::BTreeMap;
use std::sync::{Arc, MutexGuard, PoisonError};
use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode as E;
use swoole_rs_process::command::{CommandInput, Rejected, Rejection, Target, route};
#[cfg(all(feature = "upstream-symbols", not(test)))]
use {
    swoole_rs_corebase::{CCast, DataHead, ServerEvent},
    swoole_rs_reactor::Socket,
};

const SW_LOG_NOTICE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_NOTICE as c_int;
const SW_LOG_ERROR: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_ERROR as c_int;
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

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the closures run on the thread their owner gives them from, as upstream's \
              std::functions, and the lock keeps the tables apart"
)]
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
    bus: crate::objects::Bus<'_>,
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
    (bus, sock): (crate::objects::Bus<'_>, Socket<'_>),
    fd: i64,
    reactor_id: u16,
) -> bool {
    let mut reply = Vec::<u8>::new();
    handler(serv, message, &mut reply);
    let mut head = DataHead {
        fd,
        reactor_id: reactor_id.cast_signed(),
        server_fd: u16::MAX,
        kind: ServerEvent::CommandResponse.code(),
        ..DataHead::default()
    };
    bus.write(sock, &mut head, &reply)
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
/// `serv->get_command_reply_socket()`: worker 0's master pipe in base mode, else the
/// command pipe's worker end.
pub(crate) fn reply_socket<'a>(
    state: &'a ServerState,
    serv: crate::view::Server<'a>,
) -> Option<Socket<'a>> {
    let config = state.config_ref();
    if config.mode_ == Mode::Base {
        return serv.worker_pipe(0, true);
    }
    // The server's live command pipe, a `UnixSocket`.
    config.command_pipe()?.worker_end()
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
/// `Server::command`'s `SW_SERVER_EVENT_COMMAND_REQUEST` to its target: a task worker
/// through the task pool, the manager through the event pool's queue, else a worker
/// or reactor thread's pipe. Whether it went.
pub(crate) fn send(serv: Server<'_>, c: &CommandSend<'_>) -> bool {
    let process_id = u16::try_from(c.process_id).unwrap_or(u16::MAX);
    let head = |info: &mut DataHead| {
        info.kind = ServerEvent::CommandRequest.code();
        info.fd = c.request_id;
        info.server_fd = c.command_id.c_cast::<u16>();
    };
    if c.target == Target::Manager {
        // As much of the message as an `EventData` holds.
        let (data, _) = c.message.split_at(c.message.len().min(IPC_BUFFER_SIZE));
        let mut info = DataHead {
            len: u32::try_from(data.len()).unwrap_or(u32::MAX),
            ..DataHead::default()
        };
        head(&mut info);
        let pool = serv.event_pool_ref();
        return swoole_rs_process::methods::push_event(pool, &info, data);
    }
    if c.target == Target::TaskWorker {
        let mut buf = swoole_rs_process::task_file::TaskBuffer::default();
        if !buf.pack(c.message) {
            return false;
        }
        buf.set_kind(ServerEvent::CommandRequest.code());
        buf.set_fd(c.request_id);
        buf.set_server_fd(c.command_id.c_cast::<u16>());
        let task = buf.packed();
        let mut dst = c_int::from(process_id);
        let (gs, pool) = (serv.counters(), serv.task_pool_ref());
        let dispatch = || {
            Ok(swoole_rs_process::methods::dispatch(
                pool, task, &mut dst, false,
            ))
        };
        return swoole_rs_process::task_sync::count_task(&gs.tasking_num, &gs.task_count, dispatch)
            .unwrap_or(false);
    }
    let mut info = DataHead {
        reactor_id: process_id.cast_signed(),
        ..Default::default()
    };
    head(&mut info);
    // The worker's pipe: its worker end to a reactor thread, else its master end.
    let to_worker = c.target != Target::ReactorThread;
    let Some(pipe) = serv.worker_pipe(process_id, to_worker) else {
        return false;
    };
    // The bus sets the head's length to the message's.
    serv.state().message_bus().write(pipe, &mut info, c.message)
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
pub(crate) fn add_command(serv: Server<'_>, add: &AddCommand<'_>) -> bool {
    let state = serv.state();
    if serv.is_started() || commands(state).named.contains_key(add.name) {
        return false;
    }
    let config = serv.config();
    if config.mode_ != Mode::Base && config.command_pipe().is_none() && !new_command_pipe(serv) {
        return false;
    }
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
    true
}

/// `serv->pipe_command = new UnixSocket(false, SOCK_DGRAM)`: whether it opened.
fn new_command_pipe(serv: Server<'_>) -> bool {
    use swoole_rs_tls::socket_pair::{drop, unix_socket};
    let mut pipe = Box::new(unix_socket(false, libc::SOCK_DGRAM));
    if pipe.master_socket.is_null() || pipe.worker_socket.is_null() {
        drop(&mut pipe);
        return false;
    }
    serv.config().pipe_command.keep(pipe);
    true
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

/// A command request for another process.
pub(crate) struct CommandSend<'a> {
    pub(crate) target: Target,
    pub(crate) process_id: u32,
    pub(crate) command_id: i32,
    pub(crate) request_id: i64,
    pub(crate) message: &'a [u8],
}

/// `Server::command`: sends `message` to command `name` in the given process, whose
/// reply goes to `callback`. The master answers its own commands at once.
pub(crate) fn command(send: &SendCommand<'_>) -> bool {
    let (serv, state) = (send.serv, send.serv.state());
    let config = serv.config();
    let mode = config.mode_;
    let found = commands(state)
        .named
        .get(send.name)
        .map(|c| (c.id, c.accepted_types));
    let worker_id = swoole_rs_process::worker_globals::with_current(|worker| worker.id());
    let input = CommandInput {
        started: serv.is_started(),
        found: found.is_some(),
        process_mode: mode == Mode::Process,
        base_mode: mode == Mode::Base,
        master: swoole_rs_process::methods::worker_type()
            == Some(swoole_rs_process::worker_record::WorkerType::Master),
        accepted_types: found.map_or(0, |(_, types)| types),
        worker_id: worker_id.filter(|_| mode == Mode::Base).unwrap_or(0),
        process_id: send.process_id,
        process_type: send.process_type,
        reactor_num: u32::from(config.reactor_num.get()),
        worker_num: config.worker_num.get(),
        task_worker_num: config.task_worker_num.get(),
        length: send.message.len(),
        manager_capacity: IPC_BUFFER_SIZE,
    };
    let take_request = || {
        let mut commands = commands(state);
        let id = commands.next_request;
        commands.next_request = id.wrapping_add(1);
        id
    };
    let route = match route(&input) {
        Ok(route) => route,
        Err(rejected) => {
            log_rejected(rejected, send);
            // Upstream takes a request id before its later checks.
            if rejected.reason.took_request() {
                take_request();
            }
            return false;
        }
    };
    // A route means the name was found.
    let Some((command_id, _)) = found else {
        return false;
    };
    let request_id = take_request();
    if route.target == Target::Master {
        let mut reply = Vec::<u8>::new();
        let handler = commands(state).handlers.get(&command_id).cloned();
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
        return true;
    }
    // Only a reply from another process looks the callback up.
    commands(state)
        .callbacks
        .insert(request_id, send.callback.clone());
    let message = CommandSend {
        target: route.target,
        process_id: route.process_id,
        command_id,
        request_id,
        message: send.message,
    };
    let sent = hybrid().command_send(serv, &message);
    if !sent {
        commands(state).callbacks.remove(&request_id);
    }
    sent
}

fn log_rejected(Rejected { reason, process_id }: Rejected, send: &SendCommand<'_>) {
    let origin = c"Server::command";
    let notice = |code, m| error(SW_LOG_NOTICE, code, origin, m);
    match reason {
        Rejection::NotStarted => {}
        Rejection::Unknown => notice(
            E::SW_ERROR_SERVER_INVALID_COMMAND,
            format_args!("Unknown command[{}]", String::from_utf8_lossy(send.name)),
        ),
        Rejection::NotMaster => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("command() can only be used in master process"),
        ),
        Rejection::NotWorker0 => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("command() can only be used in worker process 0"),
        ),
        Rejection::UnsupportedType => notice(
            E::SW_ERROR_OPERATION_NOT_SUPPORT,
            format_args!("unsupported [process_type]"),
        ),
        Rejection::UnsupportedMode => notice(
            E::SW_ERROR_OPERATION_NOT_SUPPORT,
            format_args!("unsupported [server_mode]"),
        ),
        Rejection::InvalidThread => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("invalid thread_id[{process_id}]"),
        ),
        Rejection::InvalidWorker => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("invalid worker_id[{process_id}]"),
        ),
        Rejection::InvalidTaskWorker => notice(
            E::SW_ERROR_INVALID_PARAMS,
            format_args!("invalid task_worker_id[{process_id}]"),
        ),
        Rejection::TooLarge => notice(
            E::SW_ERROR_DATA_LENGTH_TOO_LARGE,
            format_args!(
                "message is too large, maximum length is {IPC_BUFFER_SIZE}, the given length is {}",
                send.message.len()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use crate::hybrid::tests::{Call, calls};
    use core::sync::atomic::Ordering::Relaxed;

    fn handler(prefix: &'static [u8]) -> CommandHandler {
        Arc::new(move |_, message, reply| {
            reply.extend_from_slice(prefix);
            reply.extend_from_slice(message);
        })
    }

    /// The live server in `mode`, with four event workers.
    fn live(mode: Mode) -> LiveServer {
        let live = LiveServer::new();
        // SAFETY: the fixture's configuration, which nothing else reads during the write.
        let config = unsafe { &mut *live.state().config() };
        config.mode_ = mode;
        config.worker_num.set(4);
        live
    }

    fn start(live: &LiveServer) {
        live.counters().start.store(1, Relaxed);
    }

    fn add(live: &LiveServer, name: &[u8], types: i32) -> bool {
        let add = AddCommand {
            name,
            accepted_types: types,
            handler: handler(b"re:"),
        };
        add_command(live.server(), &add)
    }

    /// The command pipe's address, which the test then closes.
    fn close_command_pipe(live: &LiveServer) -> usize {
        let pipe = live.server().config().pipe_command.as_ptr();
        if !pipe.is_null() {
            // SAFETY: the pair `add_command` boxed, which nothing uses after the test.
            let mut pair =
                unsafe { Box::from_raw(pipe.cast::<swoole_rs_tls::socket_pair::SocketPair>()) };
            swoole_rs_tls::socket_pair::drop(&mut pair);
        }
        pipe.addr()
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn commands_need_a_pipe_outside_base_mode() {
        let base = live(Mode::Base);
        assert!(add(&base, b"a", 2));
        assert!(add(&base, b"b", 2));
        assert!(!add(&base, b"a", 2), "names are unique");
        let ids: Vec<_> = commands(base.state())
            .named
            .values()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec![1, 2], "ids count from 1");
        assert_eq!(close_command_pipe(&base), 0, "no pipe in base mode");
        drop(base);

        let process = live(Mode::Process);
        assert!(add(&process, b"a", 2));
        let pipe = process.server().config().command_pipe().unwrap();
        assert!(pipe.master_end().is_some() && pipe.worker_end().is_some());
        let made = process.server().config().pipe_command.as_ptr();
        assert!(add(&process, b"b", 2));
        assert_eq!(
            process.server().config().pipe_command.as_ptr(),
            made,
            "made once"
        );
        close_command_pipe(&process);
        drop(process);

        let started = live(Mode::Base);
        start(&started);
        assert!(!add(&started, b"a", 2));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn commands_keep_binary_names_for_the_listing() {
        let live = live(Mode::Base);
        let name = b"admin\0command";
        assert!(add(&live, name, 62));
        assert_eq!(listing(live.state()), [(name.to_vec(), 1, 62)]);
        let mut reply = Vec::new();
        super::handler(live.state(), 1).unwrap()(live.server(), b"x", &mut reply);
        assert_eq!(reply, b"re:x");
        assert!(super::handler(live.state(), 2).is_none());
    }

    type Out = Arc<std::sync::Mutex<Vec<u8>>>;

    fn send<'a>(
        live: &'a LiveServer,
        name: &'a [u8],
        process_type: i32,
        out: &Out,
    ) -> SendCommand<'a> {
        let out = out.clone();
        SendCommand {
            serv: live.server(),
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
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_base_master_answers_its_own_commands_at_once() {
        let live = live(Mode::Base);
        assert!(add(&live, b"hi", MASTER | EVENT_WORKER));
        start(&live);
        let out = Out::default();
        calls();
        // Base mode's worker 0 is the master.
        assert!(command(&send(&live, b"hi", EVENT_WORKER, &out)));
        assert_eq!(*out.lock().unwrap(), b"re:ping");
        assert_eq!(calls(), [], "nothing sent");
        assert!(
            commands(live.state()).callbacks.is_empty(),
            "an answered request keeps no callback"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_failed_send_forgets_its_callback_and_unknown_names_take_no_request() {
        let live = live(Mode::Base);
        assert!(add(&live, b"hi", EVENT_WORKER));
        start(&live);
        let out = Out::default();
        calls();
        let mut to_worker = send(&live, b"hi", EVENT_WORKER, &out);
        to_worker.process_id = 9;
        assert!(!command(&to_worker), "there is no worker 9");
        assert_eq!(calls(), [], "nothing sent");
        assert!(commands(live.state()).callbacks.is_empty());
        assert_eq!(commands(live.state()).next_request, 2);
        assert!(!command(&send(&live, b"nope", EVENT_WORKER, &out)));
        assert_eq!(
            commands(live.state()).next_request,
            2,
            "no request id taken"
        );
        to_worker.process_id = 1;
        assert!(command(&to_worker));
        assert_eq!(
            calls(),
            [Call::CommandSend(Target::EventWorker, 1, 1, 2, 4)]
        );
        let waiting = commands(live.state())
            .callbacks
            .keys()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(waiting, vec![2]);
        command_result(live.state(), 2, b"pong");
        assert_eq!(*out.lock().unwrap(), b"pong");
        assert!(
            commands(live.state()).callbacks.is_empty(),
            "a reply is taken once"
        );
    }
}
