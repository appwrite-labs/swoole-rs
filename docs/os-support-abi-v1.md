# OS support and async worker boundary (M20)

Feature value 131072. Upstream targets:
`src/os/{async_thread,base,file,msg_queue,sendfile,unix_socket}.cc`.
`src/os/timer.cc` belongs to M03. Follows the
[policy/effect split](abi-v1.md#policyeffect-split).

Rust owns the async thread pool: the process-wide pool, its queue, worker
threads, scheduling and idle retirement (`crates/ossupport/src/pool.rs`), and runs
the file IO, sendfile, entropy, message queue and process/thread identity
syscalls (`file.rs`, `msg_queue.rs`, `system.rs`). Native code keeps descriptors,
allocation and logging.

## Async thread pool

- Scheduling counts the new task: it runs after the push, under the queue lock.
- Default worker count is `cpu * 8` even when an explicit core count is larger.
- `max_idle_time` is truncated to whole seconds.
- A missing handler takes precedence over the canceled status.
- A failed completion write deletes the event without counting a delivery.
- Idle retirement keeps its compensating task counter.
- The thread-local `AsyncThreads`, its pipe, `task_num` and `AsyncEvent` custody
  stay native; native hooks run, send and free events. Destruction releases the
  pool reference (the last one shuts down and joins) before closing the pipe.

Known gap: the native `uint8` `canceled` field is read across threads without
proven synchronization.

## Files, message queues, sendfile

- `file_put_contents` treats a partial positive write as success. Append mode
  uses `write`; non-append uses `pwrite` at offset 0.
- `read_line` includes the NUL/newline as upstream; a full chunk skips one byte
  at `offset + read_bytes + 1`.
- Close invalidates the fd before the syscall and never retries on `EINTR`.
- `File::exists` checks `R_OK`; `file_exists` checks `F_OK`.
- MsgQueue: one kernel id, automatic removal only for private queues, default
  mode `0666` for out-of-range permissions, `set_blocking` changes only flags,
  `EINTR` retry only on push. Tests use a private IPC namespace or a reserved key.
- Sendfile keeps separate Linux, Darwin/BSD and fallback offset/result/retry
  rules, including progress reported before `EINTR`.
- Daemon and resolver hooks run natively.
