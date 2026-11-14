# Reactor construction and backends

Upstream's poll/epoll/kqueue backends are Rust over the reactor state
(`crates/reactor/src/reactor_state.rs`, `wait_loop.rs`, `change.rs`). The
`Reactor`'s constructor and destructor are Rust too (`swr_v1_reactor_new`,
`swr_v1_reactor_delete`, behind `swoole_event_init` and `swoole_event_free`).

## Construction

The constructor (`select` in `reactor_state.rs`) preserves the requested type and
chooses the compiled backend separately. AUTO keeps epoll
priority, then runtime-enabled kqueue, then poll. An explicitly requested but
uncompiled type keeps its original `type_` value while selecting poll. kqueue
enablement is read only at the AUTO decision point.

Once the backend is ready the constructor sets the write functions and timeout,
calls the create hook, then installs the built-in end
callbacks and exit conditions ([callbacks.md](callbacks.md)).

## Registration

`add`/`set`/`del` on every backend are one Rust call, `swr_v1_reactor_change`.

## Wait

`Reactor::wait()` is Rust under its upstream symbol (`callbacks.rs`, over
`wait_loop.rs`; #336, #391, #448). Rust runs upstream's `ReactorEpoll`/`ReactorKqueue`/`ReactorPoll::wait`:
`before_wait`, begin, the kernel wait over the state's own arrays (zero timeout
while deferred work is queued), `catch_error`'s EINTR test, result, end and once
sequencing, decoding each `pollfd`, `epoll_event` or `kevent` entry (idle poll
entries and null kqueue tokens are skipped) and the readiness order per socket:
read, write, error unless read or write already ran, then the oneshot removal,
each guarded by a fresh `removed` read. kqueue routes socket, signal and unknown
entries. An uncaught wait failure returns 0 for poll and `SW_ERR` for
epoll/kqueue. Warnings keep each backend's upstream function name.

Begin runs the future task, end the end callbacks; with the `iouring` feature an
EINTR runs io_uring's handler. Handlers run from the Rust tables as
`handler(reactor, event)`.

The kqueue timeout gives a positive millisecond timeout priority over deferred
work, keeps exact second/nanosecond arithmetic, and otherwise passes a null timeout.

## kqueue signals

A signal entry keeps upstream's short-circuit: activation, then handler presence,
then whether the handler differs from `SIG_IGN`, then invocation or warning. Each
field is read only where upstream read it, and nothing is read after the handler
runs.
