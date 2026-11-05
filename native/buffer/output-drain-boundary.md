# Buffer output drain: behavior a Rust runtime must keep

The output-drain loop is still native. Its callers in pinned upstream:

| Source | Responsibility |
| --- | --- |
| `src/network/socket.cc` `handle_sendfile` | cork/span/chunk policy; sendfile result, error, progress, completion |
| `src/network/socket.cc` `handle_send` | DATA view, partial-send offset, completion/pop, wait/close |
| `src/network/socket.cc` SendfileRequest | native destructor and queued token creation |
| `src/reactor/base.cc` writable callback | queue-kind dispatch, wait/close, timer cancel, write-interest removal |
| `src/server/reactor_thread.cc` writable loop | same, plus queued bytes, watermarks, close notification |
| `src/server/base.cc`, `master.cc` | queued CLOSE creation, graceful EOF / read-interest |

A Rust runtime must own the whole drain/interest/progress decision rather than
add a predicate next to a duplicated C++ loop. No Rust frame may enclose Zend
allocation, a user callback, an exception or a bailout.

## Accounting and disposal

- A DATA chunk counts toward queued length until it is popped. Advancing its public
  offset does not reduce the Rust queue's accounting. Native token chunks do not
  count DATA bytes.
- Queued DATA has no destroy callback. Native tokens are detached from Rust
  first, then their disposer runs once, outside Rust.
- No borrowed view or Rust guard survives an external effect.

## Branches to preserve

- **DATA**: zero remaining bytes pops without sending. A full send pops. A partial
  positive send advances the offset and waits. A send returning 0 keeps the
  chunk/offset and waits. Close/wait errors keep the front. Ordinary `SW_ERROR` logs
  a warning, keeps the DATA and returns `SW_OK`.
- **SENDFILE**: ordinary `SW_ERROR` pops the token and returns `SW_OK` (unlike
  DATA). Close/wait errors keep the task. Native sendfile updates `begin`, and a
  platform may report partial progress together with an error, so
  begin/result/errno must be read together.
- **Cork**: attempted once based on span and threshold, with a tri-state result.
  A successful completion uncorks before the pop.
- **Reactor/server**: the generic reactor cancels its send timer and removes write
  interest after draining. The server also updates queued bytes, high/low
  watermarks, callbacks and close errno. An empty write queue does not allow
  resuming reads or skipping TLS close_notify.
