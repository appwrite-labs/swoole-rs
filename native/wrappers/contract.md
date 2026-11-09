# Wrapper boundary

Feature bit 2048 (`wrappers`). The module is its patches: upstream's
`src/wrapper/event.cc` and `src/wrapper/http.cc` leave the build. Their
functions are Rust: `swoole_event_*` in crates/reactor (`event_api.rs`,
`callbacks.rs`, and `reactor_state.rs` for `swoole_event_init` and
`swoole_event_free`), and the HTTP server's SDK callbacks in their own crates.
There is no wrapper crate and no C ABI. Supported targets: macOS and Linux.
