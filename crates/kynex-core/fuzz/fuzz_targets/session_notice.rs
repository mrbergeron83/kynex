#![no_main]

libfuzzer_sys::fuzz_target!(|text: &str| kynex_core::fuzz::session_notice(text));
