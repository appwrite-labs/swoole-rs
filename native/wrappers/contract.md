# Wrapper boundary

Feature bit 2048 (`wrappers`). Upstream's `src/wrapper/event.cc` and
`src/wrapper/http.cc` are not built. `event.cc`'s `swoole_event_*` are Rust in
crates/reactor (`event_api.rs`, `callbacks.rs`, and `reactor_state.rs` for
`swoole_event_init` and `swoole_event_free`). `http.cc`'s `http_server::listen` SDK
had no other user and was dropped (#1672; tests/modules/http/README.md).
There is no wrapper crate and no C ABI. Supported targets: macOS and Linux.
