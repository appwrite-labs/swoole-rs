# Wrapper boundary

Feature bit 2048 (`wrappers`). Header: `swoole_rs_wrappers.h`. Its one export
plans `swoole_event_init` (`native/reactor/reactor.cc`). Supported targets: macOS
and Linux.

`swr_v1_wrappers_event_init(flags, wait_exit_mask, ready)` returns 20 bytes:
`u32` accepted, `u32` wait_exit and three `u32` initializer IDs. Rejected
readiness returns all zero; otherwise the wait flag is normalized and the
initializer order is Socket, System, Client. C++ allocates the Reactor and
reads `ready()` before the call, runs each initializer after, then publishes the
reactor to `SwooleTG` last.
