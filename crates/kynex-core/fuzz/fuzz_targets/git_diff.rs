#![no_main]

libfuzzer_sys::fuzz_target!(|text: &str| kynex_core::fuzz::git_diff(text));
