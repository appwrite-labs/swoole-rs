# OS support policy v1

Feature bit 131072 (`ossupport`). ABI: `docs/os-support-abi-v1.md` and
`swoole_rs_ossupport.h`. Patched upstream files: `src/os/async_thread.cc`,
`src/os/file.cc`, `src/os/msg_queue.cc`, `src/os/base.cc`, `src/os/sendfile.cc`.

Rust owns the async thread pool. File, SysV queue, sendfile and entropy code is
still policy only: Rust keeps no state there and every native effect runs after
Rust returns.

## Thread pool

`crates/ossupport/src/pool.rs` is upstream's `async::ThreadPool`: one process-wide
pool (created on first `swr_v1_os_pool_acquire` with that call's configuration,
restarted after a full shutdown), its queue, `std::thread` workers, growth
scheduling and idle retirement. Each `AsyncThreads` holds one reference; the last
release shuts the pool down and joins every worker, as upstream's `use_count() == 1`.

Event tokens are `AsyncEvent` pointers the pool never dereferences. Its hooks
(`PoolHooks`) are `crates/hooks/src/async_threads.rs`, which is also `AsyncThreads`
and `async::dispatch` (under its upstream symbol; `Co::stats()` reads the counts
through `swr_v1_async_threads_stats`): `execute` runs the handler
(missing handler wins over canceled) and writes the event to its pipe; `retire`
sends the idle worker's key to the default pipe, whose callback joins it with
`swr_v1_os_pool_join`. `swr_v1_os_pool_complete` reads up to 128 tokens from the
reactor's end of the pipe and hands each to the completion callback. Copying and
freeing an event (its request is a `std::shared_ptr`) is `async_event.cc`. Defaults, scheduling and the whole-second idle wait match
upstream v6.2.4. Workers get an 8 MiB stack. Known remaining issues: the
unsynchronized `canceled` field and the retirement leak in
[retirement-shutdown-gap.md](retirement-shutdown-gap.md).

## File, sendfile and entropy

`crates/ossupport/src/file.rs` runs the syscalls; native keeps the descriptors,
`String` allocation and logging. Each call returns right after a failing syscall
so the native `swoole_sys_warning` sees its errno.

- `swr_v1_os_file_write_all` / `read_all` (`File::write_all` / `read_all`):
  `write` in append mode, otherwise `pwrite`/`pread` at the progress; zero stops,
  `EINTR`/`EAGAIN` retry, other errors set `failed` for the upstream warning.
- `swr_v1_os_file_read_line`: reads `n - 1` bytes, keeps the first NUL/newline,
  NUL-terminates and seeks past the line; without one it seeks one byte past the
  data read, as upstream.
- `swr_v1_os_file_size`: `fstat` errno or `EISDIR` for non-regular files.
- `swr_v1_os_sendfile` (Darwin, FreeBSD): retries `EINTR`, advancing the offset by
  each call's progress; Darwin retries with the length the interrupted call
  reported. `swr_v1_os_sendfile_copy` (`!HAVE_SENDFILE`): one 64 KiB `pread` and
  one `write`, no retries. Linux keeps its direct `sendfile`.
- `swr_v1_os_random_bytes` (in corebase, so String's `append_random_bytes` reaches
  it): `getrandom` on Linux/Android (retrying `EINTR`/`EAGAIN`, a zero result
  repeats), `arc4random_buf` elsewhere.

- `file_api.rs` is the rest of `src/os/file.cc`, which the patch deletes:
  `swoole_tmpfile`, both `file_get_size` overloads, `File::stat`, `write_all` and
  `read_all` under their upstream symbols (File keeps its Rust record as its first
  member), and the `std::string` helpers (`file_get_contents`, `file_put_contents`,
  `file_get_size(filename)`, `file_exists`) plus `File::read_content`, which the
  header calls with `c_str()` and the String to fill. Logs name the function as
  upstream's `__SW_FUNC__` does.

## SysV message queue

`msg_queue.rs` is `swoole::MsgQueue` (upstream `src/os/msg_queue.cc` and
`include/swoole_msg_queue.h`, both deleted): Rust allocates and frees the queue,
and native code holds an opaque `swr_v1_os_msg_queue *`. The constructor defaults
permissions outside (0, 01000) to 0666 and calls `msgget(key, IPC_CREAT | perms)`;
`set_blocking` changes only `IPC_NOWAIT`; push retries `EINTR` and stays quiet on
`EAGAIN`; pop does not retry and is quiet on `ENOMSG`/`EINTR`; a private queue is
removed when freed. Warnings and the last error follow upstream's order. Upstream's
`core-tests/src/os/msg_queue.cpp` is the module test `msg_queue_rbac`.

## Process and thread identity

`system.rs` is upstream `src/os/base.cc`, which the patch deletes. Under their
upstream symbols: the root check, `swoole_daemon` (Darwin detaches, forks and
calls `setsid` itself; elsewhere `daemon(3)` and the after-fork hooks, through the
C++ `swoole_fork`, `swoole_isset_hook` and `swoole_call_hook`), the parent-death
signal, `swoole_get_cpu_affinity` (`getpid()`, as upstream), the native thread id
and thread names. `swoole_set_isolation` is an inline header call with each
`std::string`'s `c_str()`; Rust logs each failed step as it happens, errno intact.
`file.rs`'s `swoole_sendfile` is upstream `src/os/sendfile.cc`, also deleted.

`swr_v1_os_file_open`/`close` and `swr_v1_os_tmpfile` back `File::open`,
`File::close` and `swoole_tmpfile`. The resolver request constructors and the
`gethostbyname`/`getaddrinfo` async handlers are inline in the owned `swoole_async.h`
(upstream's is deleted): `AsyncEvent`, whose layout `crates/hooks/src/offload.rs`
mirrors, and the requests, views of the Rust resolver record.
Unused upstream helpers left with the file: `swoole_set_cpu_affinity` and
`swoole_thread_id_to_str`.

Still native: File storage and UnixSocket construction.
