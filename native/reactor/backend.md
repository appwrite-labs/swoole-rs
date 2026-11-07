# Reactor construction and backends

Covers `Reactor` construction and the poll/epoll/kqueue backends
(`src/reactor/{poll,epoll,kqueue}.cc`). `src/reactor/base.cc` is deleted: its
write path, handler tables, `change`, `interest` and `_close` are Rust
(`crates/reactor/src/reactor_upstream.rs`, `crates/tls/src/reactor_write.rs`), and
the members over C++ objects (the `std::function` callbacks and tasks, the
constructor, destructor and `wait()`) are `native/reactor/reactor.cc`.

## Construction

The constructor's backend plan preserves the requested type and chooses the
compiled factory separately. AUTO keeps epoll priority, then runtime-enabled
kqueue, then poll. An explicitly requested but uncompiled type keeps its
original `type_` value while selecting the poll factory. Native observes
runtime kqueue enablement only when Rust asks at the AUTO decision point.

A zero-initialized phase requests, in order: readiness observation, field
initialization, create-hook observation/invocation, defer-end/defer-exit/idle
installation, main-thread observation/signal installation, try-exit
installation, a fresh trim main-thread observation/installation, and
default-exit installation. Native executes each action after Rust returns; the
factory switch is Native dispatch over the Rust-selected factory. No backend,
function, manager, clock or hook pointer is retained in Rust. The installed
lambda bodies themselves stay Native.

## Kernel event storage

Rust owns a fixed `Vec<MaybeUninit<T>>` of `libc::epoll_event` or
`libc::kevent`; the backend class keeps a borrowed pointer and an opaque owner.
The kernel initializes returned records. Rust neither interprets nor drops the
socket/udata tokens and never resizes the array, so Native wait counts must stay
within the construction bound.

- epoll: `epoll_create` runs before storage creation; `max_event_num` and
  `native_handle` are published after storage succeeds.
- kqueue: keeps its original publish-before-array-creation order. Its private
  pointer is now initialized, so a failed kernel create no longer reaches an
  uninitialized pointer in the destructor.
- Both destructors close the kernel handle before releasing the Rust array.

Checked layouts: macOS arm64 `kevent` 32 bytes, align 4, offsets
0/8/10/12/16/24; Linux aarch64 `epoll_event` 16 bytes, align 8, offsets 0/8.
Other targets are unqualified. The Rust vector plus owner use two allocations
where the original used one.

## Poll snapshot

`PollSnapshot` owns a bounded `Vec<libc::pollfd>` sized once at construction;
reset/push never grow or move it. Native caches a borrowed `pollfd*` and never
frees it. The original `const auto sockets = reactor_->get_sockets()` copy is
still taken before reset and marshalled in the same order. Rust stores fd values
only. Native calls `poll`, interprets the result, looks up the registry, and runs
callbacks, logging, timeout and destruction.

Both `translate_events_to_poll/from_poll` algorithms run in Rust (including
error/HUP suppression when read/write bits are set); the C++ functions are
forwarding shims. `swr_v1_reactor_poll_layout` exposes size, alignment and field
offsets for compiler comparison. Exceeding the configured limit returns failure;
Native throws `bad_alloc` after Rust returns. Negative counts keep Native
`bad_array_new_length`. No concurrent wait/reset/free is permitted.

## Registration

`add`/`set`/`del` on all three backends, poll's event translators and the
interest methods in `swoole_reactor.h` are upstream's code (#336).

## Wait

`Reactor::wait()` is one Rust call over the reactor state, `swr_v1_reactor_wait`
(#336, #391, #448); the backends no longer have a `wait()`. Rust runs upstream's
`ReactorEpoll`/`ReactorKqueue`/`ReactorPoll::wait`: `before_wait`, begin, the kernel
wait over the state's own arrays (zero timeout while deferred work is queued),
`catch_error`'s EINTR test, result, end and once sequencing,
decoding each `pollfd`, `epoll_event` or `kevent` entry (idle poll entries and
null kqueue tokens are skipped) and the readiness order per socket: read, write,
error unless read or write already ran, then the oneshot removal, each guarded by
a fresh `removed` read. kqueue routes socket, signal and unknown entries. An
uncaught wait failure returns 0 for poll and `SW_ERR` for epoll/kqueue.

Native passes the effects that hold `std::function`s or log (begin/end callbacks,
the `del`/`_del` oneshot removal, each backend's warnings under its upstream
function name, epoll's io_uring EINTR handler, kqueue signals) as
`swr_v1_reactor_wait_ops`, and `defer_tasks` as a slot read before each wait.
Handlers run from the Rust tables as `handler(reactor, event)`.

The kqueue timeout gives a positive millisecond timeout priority over deferred
work, keeps exact second/nanosecond arithmetic, and otherwise passes a null timeout.

## kqueue signals

The signal record path delegates its short-circuit sequence to Rust: observe
activation, then handler presence, then whether the handler differs from
`SIG_IGN`, then request invocation, warning or completion. Each observation is
fresh and taken only where the original code read it. A zeroed 4-byte POD holds
progress; no Signal or function pointer enters Rust. The dispatcher returns
immediately after invocation/logging with no further Signal read.
