# Reactor construction and backends

Upstream's poll/epoll/kqueue backends are Rust over the reactor state
(`crates/reactor/src/reactor_state.rs`, `wait_loop.rs`, `change.rs`); the C++
constructor, destructor and `wait()` in `reactor.cc` pass it the effects over C++
objects.

## Construction

The constructor's backend plan (`swr_v1_reactor_backend_type`) preserves the
requested type and chooses the compiled factory separately. AUTO keeps epoll
priority, then runtime-enabled kqueue, then poll. An explicitly requested but
uncompiled type keeps its original `type_` value while selecting poll. kqueue
enablement is observed only when Rust asks at the AUTO decision point.

`swr_v1_reactor_init_next` then requests, in order: readiness observation, field
initialization, create-hook observation/invocation, defer-end/defer-exit/idle
installation, main-thread observation/signal installation, try-exit
installation, a fresh trim main-thread observation/installation, and
default-exit installation. C++ executes each action after Rust returns.

## Registration

`add`/`set`/`del` on every backend are one Rust call, `swr_v1_reactor_change`.

## Wait

`Reactor::wait()` is one Rust call over the reactor state, `swr_v1_reactor_wait`
(#336, #391, #448). Rust runs upstream's `ReactorEpoll`/`ReactorKqueue`/`ReactorPoll::wait`:
`before_wait`, begin, the kernel wait over the state's own arrays (zero timeout
while deferred work is queued), `catch_error`'s EINTR test, result, end and once
sequencing, decoding each `pollfd`, `epoll_event` or `kevent` entry (idle poll
entries and null kqueue tokens are skipped) and the readiness order per socket:
read, write, error unless read or write already ran, then the oneshot removal,
each guarded by a fresh `removed` read. kqueue routes socket, signal and unknown
entries. An uncaught wait failure returns 0 for poll and `SW_ERR` for
epoll/kqueue. Warnings keep each backend's upstream function name.

C++ passes the begin/end callbacks (which hold `std::function`s) and io_uring's
EINTR handler as `swr_v1_reactor_wait_ops`. Handlers run from the Rust tables as
`handler(reactor, event)`.

The kqueue timeout gives a positive millisecond timeout priority over deferred
work, keeps exact second/nanosecond arithmetic, and otherwise passes a null timeout.

## kqueue signals

A signal entry keeps upstream's short-circuit: activation, then handler presence,
then whether the handler differs from `SIG_IGN`, then invocation or warning. Each
field is read only where upstream read it, and nothing is read after the handler
runs.
