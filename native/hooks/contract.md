# Coroutine I/O policy contract

Feature bit 65536 (`hooks`). ABI:
`swoole_rs_hooks.h`. Upstream's coroutine runtime, hook, socket, system, file-lock
and io_uring sources have left the build, and so has the module's last native source:
`coroutine::Socket` and `SocketImpl` are `swoole_rs_coroutine_socket.h`; `swoole_socket_hook.h` and
`swoole_file_hook.h` keep only what upstream's bundled C uses (`hiredis.c`/`net.c`, `ftp.c`, `plain_wrapper.c`).

## The coroutine socket

A `coroutine::Socket` is Rust's record (`swr_v1_co_socket`, crates/hooks/src/co_socket.rs)
and nothing more: no vtable and no C++ members, which the header asserts. Its strings,
proxies, TLS context and destructor callback are the record's `SocketExtra`, which the
constructors make and `~Socket` (`swr_v1_hook_co_socket_release`) frees. Rust makes the
sockets it hands out (`co_socket::make`: the coroutine streams, the hooked fds, the DNS
socket; a `UringSocket` when the build has `SW_USE_URING_SOCKET`), the accepted ones
(`Co::accept`) and frees them (`swr_v1_hook_co_socket_free`). Rust code reaches a socket
through the `Co` handle, whose members are plain Rust methods. C++ keeps only what the
bundled ext/ssh2 uses: `new SocketImpl`, `connect(host)` (`swr_v1_hook_co_socket_connect_host`),
`get_fd()` and `delete`.

## The build's switches

- The `HAVE_IOURING_*` and `SW_USE_URING_SOCKET` configure macros are
  `swoole_rs_corebase::built_with` switches (native/corebase/switches.cc), and the
  io_uring engine is the `iouring` feature's. `swoole_init` calls
  `swr_v1_hook_install`, which builds the hooks host from them and installs Rust's
  socket host (`co_io::Host`: the timers, the read `String`, `recv`/`recv_all`, the
  `Protocol` length callback and the packet diagnostics). The socket's other effects (the yield with its
  cancel, the write buffer, last error, `strerror`, the current coroutine and its
  id, the bound-socket report, the no-buffers sleep, the reactor and the TLS
  shutdown and liveness probe) are Rust too (`co_io::NATIVE`).

## Hooks and file locks

- hook.cc's socket functions (`swoole_coroutine_socket`, `send`, `recv`,
  `sendmsg`, `recvmsg`, `connect`, `accept`, `poll`, `poll_fake`, `close`, `read`,
  `write`, `socket_create`, `socket_unwrap`, `socket_exists`, the timeouts and
  `socket_wait_event`, under their C++ symbols) are Rust, and
  `swoole_coroutine_get_socket_object[_ex]` are plain Rust functions (no C caller left). The hooked fds' socket map is Rust's too: fd to a lent
  `SocketImpl *`, no reference count; hooked `close` removes it only after
  `Socket::close()` succeeds. The hooks make and delete those sockets in Rust and call the Rust members
  directly, and those pick the ring for a `UringSocket`. A C++ exception or a
  pthread cancellation in one of those calls, or in a libc call a hook falls back
  to, aborts at the Rust frame.
- `swoole_coroutine_flock` is Rust, exported under that name: outside a
  coroutine or with `LOCK_NB` it is plain flock; `LOCK_UN` unlocks;
  `LOCK_EX`/`LOCK_SH` retry `flock(op | LOCK_NB)` only on -1/EWOULDBLOCK,
  sleeping through `System::sleep` with the `wait_for` backoff, and fail when that
  sleep is cancelled; anything else fails with EINVAL as errno and the last error.
- `wait_for` uses a copied caller-local delay/phase/result plan selecting the
  next callback, sleep, success or canceled return. Backoff: capped doubling with
  reset.

## Runtime hook transitions

`crates/phpruntime` compares requested with installed flags per family.
Capability filtering runs after the before-hook callback and before
stream/PDO/function/MongoDB/after-hook/publication. Requested config flags and
installed flags stay distinct. The curl conflict warning runs before normalized
flags are published, so a PHP warning handler can bail out safely. MongoDB
aliases are never uninstalled.

## Socket IO

- recv/send/read/write/readv/writev/recvmsg/sendmsg share a retry cursor. Order:
  IO -> negative-result classifier -> timer start -> wait -> IO. EOF/success
  never classifies or waits. Send/write wait callbacks may replace the buffer
  pointer; the next IO uses the current native value. msghdr/iovec/ancillary
  ownership and the non-global-buffer requirement are unchanged.
- sendto/recvfrom use an interrupted-result cursor: negative EINTR retries
  immediately, before classifier/timer/wait. Stream/message EINTR keeps the
  classifier path. errno enters only as a copied interrupted flag after trace.
- Transfer-all snapshots hold total bytes and a bool. Native classifies negative
  results first; Rust updates unsigned progress and decides barrier continuation.
  Partial error returns completed bytes; error without progress returns -1.
  Zero-length do/while still executes once. Vectored transfer accumulates only
  positive progress and picks the next vector after native updates it. Signed
  accumulation is defined only within `ssize_t`.
- Header policy uses copied event masks/binding facts to select the read or write
  coroutine field (read wins). CID queries and fatal bound diagnostics run
  first. Availability observes native closed state first, then
  selects EBADF / close-wait / success. Return-value policy selects
  clear / errno-publication / retain without reading errno eagerly.
- Timeouts: timeval conversion keeps the original two float divisions. The
  timeout controller selects no-clock / initial-clock / elapsed-clock phase, then
  remaining-time arithmetic with strict minimum comparison. NaN, non-positive,
  infinite timeouts and zero startup keep pinned behavior.
- Lifecycle: shutdown admission and syscall mask (including success with
  ENOTCONN), bound-before-event cancel precedence, close precheck/bound
  decisions, connect EINTR/EINPROGRESS/failure and `SO_ERROR`, liveness
  errno/reset fallback. Close selectors are stateless over the native fields:
  binding is observed after native shutdown, and each cancel re-observes bindings
  after the previous resume. Native detaches the fd and clears the copied dtor
  before invoking it. Terminal resume/dtor may delete the owner; nothing touches
  it afterward. Publication order is read then write; `connected` is cleared only
  when both are set. Failed `getsockopt` keeps the stale errCode as upstream.
- Packet framing: header parsing, negative/zero/oversize length
  classification, partial-read result, bounded EOF window/growth and
  length-before-EOF-before-raw dispatch. Native EOF scanning still precedes
  max-length rejection; equality at the limit and the minimum-EOF early-continue
  are unchanged. Buffers, protocol callbacks and syscalls stay native.
- Socket wait: `Socket::wait_event` is one Rust call over the socket's state,
  which holds `want_event` and the transfer barriers. Close-wait comes first, then
  the kernel no-buffers sleep (its flag is read only when not close-waiting),
  then the TLS-wanted or own event added to the reactor, the write buffer's copy,
  the bind/yield/unbind, and the event's removal unless another coroutine still
  waits under TLS. The readable/writable callbacks ask Rust which coroutine to
  resume: the other direction's under a TLS wait, none while an installed barrier
  keeps waiting (unless the peer hung up), else the bound one.
- Buffered reads: `recv_with_buffer` and `recv_line` run in Rust over the read
  buffer's fields, kept current across the one `recv` that may yield. `recv_line`
  checks its limit before reading the next byte.

## Poll, events and timers

- `System::socket_poll` and the hooked `poll` are one Rust call, `hook_socket_poll`
  / `hook_poll`: a zero timeout is one `poll(2)`; otherwise Rust owns the task
  (rows, sockets, timer, wait/success), registers each fd through native `add`
  (refused fds are released and skipped; none registered fails without parking),
  arms the timer for a positive timeout and parks. The first readiness
  (`hook_poll_ready`) coalesces revents (error becomes the requested read/write
  unless error was requested), deletes the timer and defers one completion; the
  completion or the timer removes every socket without closing its fd and resumes.
  The hooked `poll` counts a repeated fd once, through its first entry. The reactor
  socket, timer, deferred completion and coroutine operations are Rust too
  (`poll::NATIVE`); `stream_select`'s rows merge by fd in the order libstdc++'s
  `unordered_map<int, _>` iterated upstream's map (`socket_poll_rows`).
- `System::wait_event` and `System::init_reactor` are Rust under their upstream
  symbols (`wait_event.rs`, `reactor_handlers.rs`). `wait_event` masks to read/write
  (none is EINVAL), answers a zero timeout with one `poll(2)` (POLLNVAL is EBADF,
  nothing ready is ETIMEDOUT), and otherwise parks as an `EventWaiter` on a
  `SW_FD_CO_EVENT` socket, yielding through `Coroutine::yield_ex`; it sets the last
  error and errno. `init_reactor` registers the `SW_FD_CO_SOCKET`, `SW_FD_CO_POLL`,
  `SW_FD_CO_EVENT`, `SW_FD_AIO` (and `SW_FD_IOURING`) handlers; the event waiter's
  defers the coroutine's resume on the thread's deferred queue on the first event
  only. Error readiness becomes the requested read/write bits.
- `System::read_file`/`write_file` are Rust (`file_io.rs`): open, flock, size, the
  whole read or write (`File::read_all`, `write_all`, `read_content` semantics),
  fsync and unlock on the async pool, into a `Vec`, logging each warning on the pool
  thread as upstream named it (`System::read_file`, `File::read_all`, ...).
  `write_file` keeps its upstream symbol; `read_file` is a plain Rust function.
- `System::wait_signal` is Rust (`wait_signal.rs`): the listener table, the
  EBUSY/EINVAL checks, registering through the signal table (deferred to the loop's
  thread while thread contexts are active), the reactor exit condition and
  `signal_async_listener_num`, the park, the wake-up handler `hook_signal_wake`
  (clears the slot, records the signal, resumes) and the unregistering after it.
  It is the plain Rust `system_wait_signal`.
- The hooked stdio, directory and file-system functions (`fopen`, `fdopen`,
  `freopen`, `fread`, `fwrite`, `fgets`, `fputs`, `feof`, `fflush`, `fclose`,
  `opendir`, `readdir`, `closedir`, `open`, `fstat`, `stat`, `lstat`, `unlink`,
  `mkdir`, `rmdir`, `rename`, `fsync`, `fdatasync`, `ftruncate`, `lseek`,
  `readlink`, `statvfs`, `access`), `sleep`, `usleep`, `getaddrinfo` and
  `gethostbyname` are Rust, exported under upstream's `swoole_coroutine_<name>`.
  The coroutine test, `async_job`, `System::sleep` and the last error are Rust
  (`stdio::NATIVE`). `swoole_init` installs their host (`swr_v1_hook_install`): in an
  io_uring build, the engine's host (`iouring::HOST`), the `HAVE_IOURING_*` opcodes and whether
  sockets are `UringSocket`s.
  Inside a coroutine an io_uring build sends the call to the ring where upstream did;
  outside a coroutine the libc call runs in place; inside one it runs through native
  `async`, which parks the coroutine and restores the worker's errno; a refused
  dispatch returns the function's failure value. The resolver always offloads.
- `TimerController` selects activation/addition/negative-timeout sentinel and
  enabled-only destruction. Disabled destructors and zero-timeout starts do not
  dereference `timer_pp`. Positive allocation failure leaves enabled = true with
  a null pointer; existing non-null timers are not re-armed.
- `System::sleep` is Rust (`crates/hooks/src/sleep.rs`, upstream symbol): the
  minimum keeps NaN/Inf/non-positive comparisons and the get-current/`yield_ex`/
  is-canceled order; canceled is `yield_ex` failing with `SW_ERROR_CO_CANCELED`.

## Async tasks, DNS and io_uring

- `coroutine::async` and `async_job` are Rust (`offload.rs`): they fill the event
  (Rust mirrors `AsyncEvent`'s layout; swoole_async.h asserts it), dispatch it to
  the pool, park and resume the coroutine, and publish the result. Upstream's
  `async(std::function)` had no caller left and is gone. A refused dispatch returns
  false before parking; a timeout or cancel marks the pool's copy canceled, so its
  completion never touches the parked task.
- DNS async-timeout error mapping is scalar policy in both host/address lookup
  wrappers; requests, parsing and c-ares/libc stay native. The DNS cache
  (LRU, expiry, family key, hit/miss counters) is Rust, and so are `System`'s
  four DNS cache settings, under their upstream symbols.
- io_uring: CQ harvesting, SQ admission and task counter decrement stay native
  and precede resume. Rust maps negative CQ results (ECANCELED -> ETIMEDOUT).
  Submit keeps EAGAIN defer, EBUSY sleep, EINTR retry and the native fatal
  diagnostic.
