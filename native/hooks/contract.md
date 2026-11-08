# Coroutine I/O policy contract

Feature bit 65536 (`hooks`). ABI: `docs/hooks-abi-v1.md` and
`swoole_rs_hooks.h`. Patched upstream files: `ext-src/swoole_runtime.cc` (deleted; the
coroutine streams reach the socket through `system.cc`'s
`swr_v1_runtime_socket`, the C++ services through `native/phphelpers/module.cc`, and
`thirdparty/php/streams/plain_wrapper.c` builds alone),
`include/swoole_coroutine_socket.h`, `src/coroutine/file_lock.cc` (deleted),
`src/coroutine/hook.cc` (deleted),
`src/coroutine/iouring.cc`, `src/coroutine/socket.cc` (deleted; the coroutine
Socket's native side is in `system.cc`), `src/coroutine/system.cc`,
`src/coroutine/uring_socket.cc` (deleted with `include/swoole_uring_socket.h`; a
`UringSocket` is a `coroutine::Socket` its `swoole_socket_impl.h` constructors mark, and
the Rust members take the ring for it).

All exports are allocation-free, callback-free decisions over copied scalars.
They never read or write errno/last_error. Native code owns objects,
IO, callbacks, queues, timers, context switches, PHP function/transport
registries and error publication. No user-buffer borrow or Rust frame survives
a native operation, and native exceptions unwind only native frames. Public
declarations and layouts are unchanged.

## Hooks and file locks

- hook.cc's socket functions (`swoole_coroutine_socket`, `send`, `recv`,
  `sendmsg`, `recvmsg`, `connect`, `accept`, `poll`, `poll_fake`, `close`, `read`,
  `write`, `socket_create`, `socket_unwrap`, `socket_exists`, the timeouts and
  `socket_wait_event`, and `swoole_coroutine_get_socket_object[_ex]` under their C++
  symbols) are Rust. The hooked fds' socket map is Rust's too: fd to a lent
  `SocketImpl *`, no reference count; hooked `close` removes it only after
  `Socket::close()` succeeds. The host's `socket` table is what stays C++ of
  `SocketImpl`: its constructors and `delete`. The hooks call the Rust members
  directly, and those pick the ring for a `UringSocket`. A C++ exception or a
  pthread cancellation in one of those calls, or in a libc call a hook falls back
  to, aborts at the Rust frame.
- `swoole_coroutine_flock` is Rust, exported under that name: outside a
  coroutine or with `LOCK_NB` it is plain flock; `LOCK_UN` unlocks;
  `LOCK_EX`/`LOCK_SH` retry `flock(op | LOCK_NB)` only on -1/EWOULDBLOCK,
  sleeping through the host's `System::sleep` with the `wait_for` backoff, and fail
  when that sleep is cancelled; anything else fails with EINVAL as errno and, through
  the host's `swoole_set_last_error`, the last error.
- `hook_file_result` rejects open fd 0 and does not reset close's fd.
- `wait_for` uses a copied caller-local delay/phase/result plan selecting the
  next callback, sleep, success or canceled return. Backoff: capped doubling with
  reset.

## Runtime hook transitions

Requested flags are compared with installed flags per family (21 families,
including curl and MongoDB's install-only alias path), with no Rust registry.
Capability filtering runs after the before-hook callback and before
stream/PDO/function/MongoDB/after-hook/publication. Requested config flags and
installed flags stay distinct. Curl conflict normalization is a copied
by-value plan: the native warning runs before normalized flags are published and
before Rust reads installed flags again, so PHP warning handlers can re-enter or
throw safely. MongoDB class lookup/alias registration stay native, and aliases
are still never uninstalled.

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
  coroutine field (read wins). CID queries and fatal bound diagnostics stay
  native and run first. Availability observes native closed state first, then
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
  The hooked `poll` counts a repeated fd once, through its first entry.
- `System::wait_event` is one Rust call, `hook_wait_event`: it masks to read/write
  (none is EINVAL), answers a zero timeout with one `poll(2)` (POLLNVAL is EBADF,
  nothing ready is ETIMEDOUT), and otherwise parks as an `EventWaiter` whose state
  (coroutine, fired events) is Rust's: native adds and removes the `SW_FD_CO_EVENT`
  socket and yields. The `SW_FD_CO_SOCKET`, `SW_FD_CO_POLL` and `SW_FD_CO_EVENT`
  reactor handlers are Rust, registered by `hook_init_reactor` from `System::init_reactor` through a
  native `set_handler`; the event waiter's defers native `wake` (the coroutine's
  resume) on the first event only. Error readiness
  becomes the requested read/write bits. Native sets the last error and errno it
  returns.
- `System::read_file`/`write_file` are one Rust call each, `hook_read_file` and
  `hook_write_file`: open, flock, size, the whole read or write (`File::read_all`,
  `write_all`, `read_content` semantics), fsync and unlock on the async pool.
  Native creates and grows the `String` and logs each warning on the pool thread,
  named as upstream (`System::read_file`, `File::read_all`, ...).
- `System::wait_signal` is one Rust call, `hook_wait_signal`: the listener table,
  the EBUSY/EINVAL checks, registering through native `listen`, the park, the wake-up handler
  `hook_signal_wake` (clears the slot, records the signal, resumes) and the
  unregistering after it. Native passes the coroutine operations, signal
  registration (deferred under `SW_USE_THREAD_CONTEXT`), the reactor exit condition
  and `signal_async_listener_num`.
- The hooked stdio, directory and file-system functions (`fopen`, `fdopen`,
  `freopen`, `fread`, `fwrite`, `fgets`, `fputs`, `feof`, `fflush`, `fclose`,
  `opendir`, `readdir`, `closedir`, `open`, `fstat`, `stat`, `lstat`, `unlink`,
  `mkdir`, `rmdir`, `rename`, `fsync`, `fdatasync`, `ftruncate`, `lseek`,
  `readlink`, `statvfs`, `access`), `sleep`, `usleep`, `getaddrinfo` and
  `gethostbyname` are Rust, exported under upstream's `swoole_coroutine_<name>`.
  system.cc registers a host table while the library loads (`swr_v1_hook_set_host`): its
  coroutine test, `async`, `System::sleep`, the poll table, the socket table and, in an
  io_uring build, `Iouring::host`.
  Inside a coroutine an io_uring build sends the call to the ring where upstream did;
  outside a coroutine the libc call runs in place; inside one it runs through native
  `async`, which parks the coroutine and restores the worker's errno; a refused
  dispatch returns the function's failure value. The resolver always offloads.
- `TimerController` selects activation/addition/negative-timeout sentinel and
  enabled-only destruction. Disabled destructors and zero-timeout starts do not
  dereference `timer_pp`. Positive allocation failure leaves enabled = true with
  a null pointer; existing non-null timers are not re-armed.
- `System::sleep` minimum keeps NaN/Inf/non-positive comparisons and
  get-current/`yield_ex`/is-canceled order.

## Async tasks, DNS and io_uring

- `coroutine::async` (both forms) is Rust, `hook_async` and `hook_async_job`: it
  fills the event (Rust mirrors `AsyncEvent`'s layout; system.cc asserts it), parks
  and resumes the coroutine, and publishes the result. Native passes
  `async::dispatch` and the coroutine operations; `async(std::function)` is a
  trampoline into `hook_async_job`. A refused dispatch returns false before
  parking; a timeout or cancel marks the pool's copy canceled, so its completion
  never touches the parked task.
- DNS async-timeout error mapping is scalar policy in both host/address lookup
  wrappers; requests, parsing and c-ares/libc stay native. The DNS cache
  (`hook_dns_cache_*`: LRU, expiry, family key, hit/miss counters) is Rust.
- io_uring: CQ harvesting, SQ admission and task counter decrement stay native
  and precede resume. Rust maps negative CQ results (ECANCELED -> ETIMEDOUT).
  Submit keeps EAGAIN defer, EBUSY sleep, EINTR retry and the native fatal
  diagnostic. Behavior on io_uring-enabled kernels is untested.

Known native races (cancellation fields, unhandled worker exceptions) are
unchanged.
