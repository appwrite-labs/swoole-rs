# Socket I/O

`src/network/socket.cc` is deleted. Its members that need C++ (exceptions,
`std::string`, `std::vector`) are inline in `swoole_rs_socket_methods.h`, and
`free`, whose deferral is a `std::function`, is in `src/reactor/base.cc`; the rest are Rust (`crates/reactor/src/socket_upstream.rs`,
`crates/tls/src/upstream.rs`). The IOVector storage and progress are below and
the output paths in [output.md](output.md).

## IOVector

Rust scans `iovec` lengths lazily (never payloads) and returns
remaining/index/offset and pointer progress; Native writes the public fields.
Full completion publishes counters and returns before touching the iterator.
The upstream `offset_bytes` quirk at an exact segment boundary after partial
progress is kept.

Storage: the constructor makes one Rust allocation holding release `Layout` metadata followed
by two contiguous `iovec` spans (original and iterator), copied from the caller's
descriptors. Payload bytes are not copied or owned. Native publishes the
returned pointers/counts; allocation refusal returns before Native throws
`bad_array_new_length` or `bad_alloc`. Release uses the allocation-local Layout,
not the public count, and never drops payloads; NULL release is a no-op. Inline
`length` reads the original span each call with C `size_t` wrapping. Public class
layout and getters are unchanged.

Incompatible: replacing the public pointers with Native `new[]`
arrays, or calling `delete[]` on Rust spans. Implicit C++ shallow copy is
unchanged.
