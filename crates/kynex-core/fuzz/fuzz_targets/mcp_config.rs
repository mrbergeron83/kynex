#![no_main]

libfuzzer_sys::fuzz_target!(|text: &str| kynex_core::fuzz::mcp_config(text));
