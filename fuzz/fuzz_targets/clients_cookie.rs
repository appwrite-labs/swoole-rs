#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| swoole_rs_fuzz::clients_cookie(bytes));
