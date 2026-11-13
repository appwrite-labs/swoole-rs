# Worker, task and reload transitions v1

Feature bit 524288 (`process`). crates/process runs the event worker, task worker,
reactor process, manager and factory operations, and crates/server builds each
operation's context and answers its ops. Decisions it keeps from upstream:

- WorkerExit observes `if_exit` before callback presence, publishes its local
  `called` flag before the native callback, then observes reactor/settings/clock
  again. The clock is not sampled before the callback or when the reactor can
  exit. Remaining seconds keep native `int` narrowing; positive `remaining*1000`
  outside `int` range is undefined input. Native timeout is written only when the
  original comparison required it. No Rust frame spans the callback.
- Task shutdown keeps its early return without idle publication; only ordinary
  tasks increment request accounting. Finish: `PIPE_MESSAGE` precedes `NOREPLY`.
  Task packing stays native before callback/coroutine flag selection, and
  `CALLBACK` wins. The original `latest_task == nullptr` invalid context remains
  a native gap.
- Worker discard keeps null / closed / disable-notify precedence; the warning is
  native.
- Manager reload selection runs only in the ordinary wait loop, after the
  original message/timer/signal dispatch. `Server::reload` and signal frames stay
  native. All-worker reload precedes task-only reload.
- Reload kill actions consume a fresh native kill result/errno. ESRCH/ECHILD
  erase-and-continue only for `kill_one`; `kill_all` keeps its map. EPERM/other
  failures warn then stop that attempt. Worker iteration, kill queue, timer
  kill-before-clear and callback order are unchanged.
- Worker CPU pinning (`Worker_set_cpu_affinity`, Linux) is one Rust operation:
  read the process's affinity, keep `cpu_affinity_available`, pick the
  `worker_id % count`-th CPU and pin the thread (thread mode) or process.
  crates/server logs upstream's warnings with the outcome's errno.
- A task's temporary file (`Server::task_unpack`) is read in Rust: open, pread
  `length` bytes retrying EINTR/EAGAIN, close, unlink unless `SW_TASK_PEEK`.
  Native sizes the buffer first (upstream did so after a successful open), then
  logs the open/pread warnings and builds the packet.
- The signal table (upstream `src/os/signal.cc`; facades in
  `crates/reactor/src/signal_upstream.rs`) is `SIGNALS` (`crates/process/src/signal.rs`),
  which the kqueue path shares (a slot is the kqueue event's udata);
  `native/process/swoole_signal.h` keeps the signal functions C++ still calls. Rust registers handlers through sigaction, runs the deferred and
  immediate async handlers (atomic stores; the immediate one calls
  `swoole_signal_callback` in the signal frame as upstream), dispatches marked
  signals and blocks/unblocks every signal. Dispatch's callback may run PHP; no Rust value with a
  destructor is live across it. Upstream's `pthread_sigmask(...) < 0` check never
  held, so blocking all signals has no warning.
- The signalfd path (Linux) is Rust too: the routed-signal mask (atomics, no
  lock), the descriptor and its creating PID, registration, create/clear/close and
  reading one `signalfd_siginfo` into a call/warn/ignore decision. Native keeps
  the reactor `Socket` around the descriptor, its handler, exit condition and
  destroy callback, `SwooleG.signal_fd`, the warnings and the handler call.
- Coroutine waitpid (upstream `src/os/wait.cc`; facades in
  `crates/reactor/src/wait_upstream.rs`, registry in `crates/process/src/wait.rs`) is Rust: the children
  reaped before anyone waited, the any-child FIFO and the per-PID waiters, the
  last WNOHANG try after SIGCHLD registration, the yield, and the SIGCHLD reaper
  that hands each child out and resumes its coroutine. The caller passes the
  coroutine operations (current, yield, resume, timed out). The waiting
  coroutine's Rust frame stays on its stack across the yield; no value with a
  destructor is live there, and no RefCell borrow spans a yield or resume.

`cargo test -p swoole-rs-process` checks each worker, task, reload, exit-budget
and result-flag decision against the upstream expression it replaces, and the
C layout of the exit records.

