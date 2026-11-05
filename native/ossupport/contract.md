# OS support policy v1

Opt-in feature bit 131072 (`ossupport`). ABI: `docs/os-support-abi-v1.md` and
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

Event tokens are native `AsyncEvent` pointers that Rust never dereferences. Native
hooks (`swr_v1_os_pool_hooks`) run on worker threads: `execute` runs the handler
(missing handler wins over canceled) and writes the event to its pipe; `retire`
sends the idle worker's key to the default pipe, whose callback joins it with
`swr_v1_os_pool_join`. `swr_v1_os_pool_complete` reads up to 128 tokens from the
reactor's end of the pipe and hands each to native's completion callback. Defaults, scheduling and the whole-second idle wait match
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

`msg_queue.rs`: `swr_v1_os_msg_open` defaults permissions outside (0, 01000) to
0666 and calls `msgget(key, IPC_CREAT | perms)`; `swr_v1_os_msg_flags` changes
only `IPC_NOWAIT`; push retries `EINTR` and stays quiet on `EAGAIN`; pop does not
retry and is quiet on `ENOMSG`/`EINTR`. `swr_v1_os_msg_remove` is `IPC_RMID`. Stat and capacity stay native (`msqid_ds`).
Module tests use a private queue and remove it.

## Process and thread identity

`system.rs` (upstream `src/os/base.cc`): root check, `swoole_set_isolation`
(reporting each failed step through a native callback so warnings keep their
order and errno), parent-death signal, process CPU affinity (`getpid()`, as
upstream), native thread id and thread names.

`swr_v1_os_daemon_detach` runs Darwin `swoole_daemon`'s `chdir`/`/dev/null`
steps; `swr_v1_os_file_open`/`close` and `swr_v1_os_tmpfile` back `File::open`,
`File::close` and `swoole_tmpfile`.

Still native: the fork and setsid in `swoole_daemon`, the resolver request types
and handlers, `swoole_thread_id_to_str`, File storage and UnixSocket construction.

`QueueRecord` is the canonical inline `MsgQueue` record, emitted by cbindgen.
Initialization and blocking-flag/removal updates run in Rust without adding an
allocation or changing native value-copy behavior. Native code retains the
platform-specific `msqid_ds` view and original warning/last-error ordering.
