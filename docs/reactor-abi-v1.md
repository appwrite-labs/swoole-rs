# Reactor boundary (M05)

Feature value 16384. Upstream targets: `src/reactor/`,
`src/network/{socket,stream}.cc`, `src/server/{base,reactor_thread}.cc`,
`ext-src/swoole_admin_server.cc`, `include/swoole_{reactor,socket}.h`. Follows
the [policy/effect split](abi-v1.md#policyeffect-split).

Native Reactor and Socket objects keep their public API and lifetime. Rust may
own private policy and descriptor-list storage behind an opaque handle and
receives scalar/span snapshots and opaque tokens. fd/socket/session custody stays
native and singular; a poll snapshot rebuilt from the native registry is not a
second map.

- Kernel waits, syscalls, callbacks, warnings, exceptions and bailout run outside
  Rust. Rust returns events; native runs one callback, then supplies fresh
  removal/interest/running state before the next decision.
- Rust never frees sockets or treats a numeric fd as a lifetime. Deferred free
  keeps epoll/kqueue event tokens valid for the whole batch; old tokens never
  resolve to a new fd occupant.
- Backend behaviors to keep: AUTO selection, read-before-write, error-only
  dispatch, per-step removal checks, ONCE/early-continue, HUP flags, signal
  records, deferred-work timeouts, EINTR/uring hooks, non-EINTR return status,
  and poll's snapshot-fd lookup in the current registry.
- Registration failure keeps each original sequence: epoll `EBADF`/`ENOENT`
  cleanup on delete, poll capacity/existence checks, kqueue partial filter
  updates. OOM returns a typed failure; native translates it.
- Order: before-wait, begin, end callbacks; timers (priority 0) before defers,
  idle, signals and exit checks. Destruction marks destroyed and runs native
  destroy callbacks before deleting the backend, so timer disarm still sees it.
  Child fork cleanup keeps the native reset order.
- Writable DATA/SENDFILE/CLOSE execution and TLS stay native. M15's
  pause/drain/close behavior is preserved.

## Rust-owned private storage

- End-callback and exit-condition ordered maps: Rust owns ordered metadata and
  opaque payload tokens; `std::function` payloads are allocated, copied, called
  and destroyed natively. Keys are traversed ascending with fresh queries after
  each callback. Changes the selected Reactor class layout.
- Handler resolution (`set`/`isset`/`get`/`get_error`): Rust policy over the
  existing public typed handler arrays. `set` uses bit priority; `get` selects
  the exact event with error fallback through defaults/read/write.
- Socket output: `handle_send`, `sendfile_async`, `handle_sendfile`, blocking
  `send`/`writev`/`sendfile`, `wait_for`, `what_event_want` and `IOVector`
  progress policy in `socket.cc`. iovec arrays, SSL, syscalls and public layout
  stay native. The optional M15 patch is applied first and unchanged.
- `--reactor-{iov,defer,sockets}-owned` select further Rust-owned storage.

The `sockets_` registry stays native: `get_sockets()` returns a const reference
to the native `unordered_map`, and changing that is a public API change.
