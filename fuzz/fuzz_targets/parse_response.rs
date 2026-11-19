// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Fuzzing target for `client::parse_response`, the HTTP/1.1 response
// parser every `ztop connect` and every hub poll runs on bytes a remote
// server chose. It must turn any input into a status and body or an
// error, never a panic (audit M1 found a panic here).

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = ztop_core::client::parse_response(data);
});
