//! Immutable known-string bytes, indexed by `swoole_rs_phphost.h`'s `SW_ZEND_STR_*`
//! (the ones C++ still reads). Zend's interned pointer table is a Rust static,
//! published at MINIT.

pub(crate) const STRINGS: [&[u8]; 1] = [b"in_coroutine\0"];

#[cfg(test)]
mod tests {
    #[test]
    fn indexes_are_the_native_ones() {
        let header = include_str!("../../../../native/phphelpers/swoole_rs_phphost.h");
        assert_eq!(super::STRINGS[0], b"in_coroutine\0");
        assert!(header.contains("SW_ZEND_STR_IN_COROUTINE = 0,"));
    }
}
