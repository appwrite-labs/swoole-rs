# Coroutine I/O and runtime hooks boundary (M07)

Feature value 65536. Upstream targets:
`src/coroutine/{hook,socket,system,file,file_lock,iouring,uring_socket}.cc`,
`ext-src/swoole_runtime.cc`, `include/swoole_coroutine_socket.h` (inline Socket
method bodies only; public layout unchanged). Follows the
[policy/effect split](abi-v1.md#policyeffect-split).

Rust owns routing, transfer/retry/error/timeout and hook-transition policy. Every
syscall, allocation, container change, timer, diagnostic, callback and context
switch runs natively after Rust returns. No user-buffer borrow survives a yield or
async completion. `socket_map`/`shared_ptr` custody, Socket fields and buffers,
PollSocket/tasks/EventWaiter, AsyncEvents, AsyncThreads queues, io_uring SQ/CQ
and waiting tasks, DNS caches and PHP hook tables stay native and singular.

errno, `h_errno`, Swoole last_error and Socket errors stay distinct in original
order. Where upstream returns immediately after terminal resume/destruction, so
does the adapter; an extra refresh there is a use-after-free.

- **Socket wait/close**: TLS WANT redirection, no-buffer fallback, interest
  errors, buffer copy before yield, timer/read/write publication order. Timer
  callbacks clear their field before resume. Error events wake the writer, then
  recheck identity before waking the reader. Close detaches the fd and clears the
  copied dtor before calling it. Stale fds never resolve to new occupants.
- **System poll**: borrows fd wrappers without closing caller fds; coalescing,
  deferred cleanup, one terminal wake. AsyncEvent cancellation and
  AsyncLambdaTask stack-closure completion differ and both are kept; worker errno
  is transferred on the original thread.
- **DNS/files**: native resolver/libc/c-ares selection, DNS cache key/expiry
  order, per-hook no-coroutine routing; flock nonblocking/unlock, invalid
  operation precedence, capped wait backoff.
- **io_uring**: advance CQ, admit queued SQ work, then resume; counters decrement
  before resume; expired stack events are never reread. Native
  `EAGAIN`/`EBUSY`/`EINTR`, `ECANCELED` mapping and timeout-SQE failure order are
  kept. Disabled io_uring is unsupported, not a fallback.
- **PHP hooks**: requested flags and installed flags are distinct; requested
  updates precede native thread guards; registration failure is not rolled back.
  curl's by-value normalization and callback/transport/stream/PDO/function
  order are kept.
- **Socket inline methods**: availability checks, read-before-write event
  selection, bound-coroutine checks, error publication and timeout conversion are
  in Rust; diagnostics (which may bail out), strings, timers and callbacks stay
  native.

Not validated: thread-context ownership and macOS ucontext. Original inventory:
227 primary PHPTs plus 2 io_uring.
