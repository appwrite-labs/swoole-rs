# Wrapper boundary

Feature bit 2048 (`wrappers`). Header: `swoole_rs_wrappers.h`. Both patched upstream
files are deleted: what is left of `src/wrapper/event.cc` is `native/reactor/reactor.cc`,
and `src/wrapper/http.cc` (the `http_server::listen` SDK, used only by four GoogleTests,
now Rust tests listed in `tests/modules/http/README.md`) is gone. Supported targets:
macOS and Linux. No panic, C++ exception or Zend bailout may cross the ABI.

## Event wrappers

Event add-or-update direction and wait admission are Rust decisions. The native
facade keeps `if_exit` short-circuiting, invalid-mask assertions, callback
installation, `SwooleTG` ownership, syscalls and teardown. Other event functions
forward to the reactor. Timer wrappers belong to the timer module.

`swr_v1_wrappers_event_init(flags, wait_exit_mask, ready)` returns 20 bytes:
`u32` accepted, `u32` wait_exit and three `u32` initializer IDs. Rejected
readiness returns all zero; otherwise the wait flag is normalized and the
initializer order is Socket, System, Client. Native allocates the Reactor and
calls `ready()` before Rust, runs each initializer after, then publishes the
same owner to `SwooleTG` last.
