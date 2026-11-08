# Suspended HTTP response ownership (open problem)

M15's synchronous non-TLS PROCESS-mode early-FIN fix does not cover responses
held by a suspended coroutine; that case still fails. No design is chosen yet.
Constraints:

- Pipelined requests and coroutine concurrency must keep working. Serializing to
  one request per connection can deadlock an app whose first callback waits for
  the second request, so a single-lease/pause design is not correct. Counted,
  tokened outstanding responses with deferred clean EOF is an untested option.
- The PHP HTTP Context, the core framing Request and M17's wrapper Context have
  different lifetimes and cannot share one handle.
- `onAfterResponse` is concurrency bookkeeping that runs before `end()`,
  `sendfile()` and destructor auto-end. Neither a callback return, a write nor
  entering `end()` means the response was committed.
- Retryable chunk/header failures keep ownership. Terminal failure, close,
  reset/force and worker shutdown cancel it without reviving a closed session.
  Detached or recreated contexts need explicit transfer, and stale completions
  must be rejected by session plus request identity.
- Port parsing can return on removal before consuming a dispatched prefix and
  always calls `recv()` before parsing buffered data, so pause/resume can
  duplicate a request or strand a buffered tail.
- Worker death: the manager respawns after checking exit status, and only
  nonzero exits run the error callback. `Server::abort_worker`
  (`crates/server/src/worker_methods.rs`) clears BASE-mode session state, not PROCESS-mode response ownership. RSHUTDOWN does
  not run on SIGKILL/`_exit`. Response accounting must cancel a dead worker
  generation's tokens and keep healthy idle sessions.

The sendfile/FIN loss and the `EINTR` chunk-loss fixes are separate bugs.
