# Worker, task and reload transitions v1

Feature bit 524288 (`process`). ABI: `docs/process-abi-v1.md` and
`swoole_rs_process.h`. Callers are in `src/server/process.cc` (the event worker, reactor process,
manager and factory ops), `src/server/task_worker.cc` and `src/os/process_pool.cc`.

Rust decides worker-exit observation and timeout plans, discard triage,
task-kind/finish diagnostic/result-flag precedence, ordinary manager reload
selection and stale-PID/reload-kill result actions. All APIs take copied
bool/int facts and return POD/scalars (CPU pinning and task files also take
native pointers). No allocator, callback, borrow,
registry or native handle enters Rust. Server/Worker/ProcessPool/ReloadTask
state, counters, maps, PID snapshot and kill queue stay native, as do logging,
signals, syscalls, timers, reactor, callbacks, fork, atomics and coroutines,
which run after Rust returns.

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
  `worker_id % count`-th CPU and pin the thread (thread mode) or process. Native
  sets errno from the outcome and logs upstream's warnings.
- A task's temporary file (`Server::task_unpack`) is read in Rust: open, pread
  `length` bytes retrying EINTR/EAGAIN, close, unlink unless `SW_TASK_PEEK`.
  Native sizes the buffer first (upstream did so after a successful open), then
  logs the open/pread warnings and builds the packet.
- The signal table (`src/os/signal.cc`) is Rust's `swr_v1_signals`, laid out as
  `swoole::Signal` so the native kqueue path (and kqueue's udata) still uses it. Rust registers handlers through sigaction, runs the deferred and
  immediate async handlers (atomic stores; the immediate one calls
  `swoole_signal_callback` in the signal frame as upstream), dispatches marked
  signals and blocks/unblocks every signal. Native keeps the warnings and calls
  the looked-up handler. Dispatch's callback may run PHP; no Rust value with a
  destructor is live across it. Upstream's `pthread_sigmask(...) < 0` check never
  held, so blocking all signals has no warning.
- The signalfd path (Linux) is Rust too: the routed-signal mask (atomics, no
  lock), the descriptor and its creating PID, registration, create/clear/close and
  reading one `signalfd_siginfo` into a call/warn/ignore decision. Native keeps
  the reactor `Socket` around the descriptor, its handler, exit condition and
  destroy callback, `SwooleG.signal_fd`, the warnings and the handler call.
- Coroutine waitpid (`src/os/wait.cc`, without io_uring) is Rust: the children
  reaped before anyone waited, the any-child FIFO and the per-PID waiters, the
  last WNOHANG try after SIGCHLD registration, the yield, and the SIGCHLD reaper
  that hands each child out and resumes its coroutine. Native passes the
  coroutine operations (current, yield, resume, timed out), registers SIGCHLD and
  the reactor callbacks, and sets errno/last error on a timeout. The waiting
  coroutine's Rust frame stays on its stack across the yield; no value with a
  destructor is live there, and no RefCell borrow spans a yield or resume.

`cargo test -p swoole-rs-process` checks each worker, task, reload, exit-budget
and result-flag decision against the upstream expression it replaces, and the
C layout of the exit records.

Not converted: the rest of startup/fork/worker/task/reload/exit/IPC/signal/wait
lifecycle.
