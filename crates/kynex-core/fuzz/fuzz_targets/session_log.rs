#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| kynex_core::fuzz::session_log(bytes));
