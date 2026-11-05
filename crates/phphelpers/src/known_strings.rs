//! Immutable known-string bytes in native enum order (`SW_ZEND_KNOWN_STRINGS`,
//! the ones C++ still reads). Zend's interned pointer table is allocated and
//! published by Rust.

pub(crate) const STRINGS: [&[u8]; 6] = [
    b"setting\0",
    b"fd\0",
    b"sock\0",
    b"pipe\0",
    b"in_coroutine\0",
    b"socket\0",
];
