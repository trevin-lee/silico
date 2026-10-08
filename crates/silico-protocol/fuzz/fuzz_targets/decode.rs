//! Any byte stream, read as requests and as responses, gives messages or a clean error:
//! never a panic, a hang, or an allocation beyond what arrived.
#![no_main]

use libfuzzer_sys::fuzz_target;
use silico_protocol::{FrameReader, Request, Response};

fuzz_target!(|data: &[u8]| {
    let mut requests = FrameReader::with_max_len(data, 1 << 20);
    while let Ok(Some(_)) = requests.read_message::<Request>() {}
    let mut responses = FrameReader::with_max_len(data, 1 << 20);
    while let Ok(Some(_)) = responses.read_message::<Response>() {}
});
