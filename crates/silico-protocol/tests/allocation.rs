//! A frame's length is never trusted: reading allocates only for bytes that actually
//! arrive. Measured with a counting allocator, in its own test binary so it counts nothing
//! else. (On macOS a large allocation is lazy, so time or memory alone can't show this.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::alloc::System;

use silico_protocol::{FrameError, FrameReader, MAX_FRAME_LEN};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// Bytes allocated (and grown by reallocation) while reading one frame from `stream`.
fn bytes_allocated_reading(stream: &[u8]) -> (Result<Option<Vec<u8>>, FrameError>, usize) {
    let region = Region::new(GLOBAL);
    let result = FrameReader::new(stream).read_frame();
    let change = region.change();
    let grown = usize::try_from(change.bytes_reallocated.max(0)).unwrap();
    (result, change.bytes_allocated + grown)
}

#[test]
fn a_lying_length_allocates_only_what_arrives() {
    // The header claims the maximum (512 MiB); 10 bytes follow.
    let mut stream = MAX_FRAME_LEN.to_le_bytes().to_vec();
    stream.extend([0u8; 10]);
    let (result, allocated) = bytes_allocated_reading(&stream);
    assert!(
        matches!(result, Err(FrameError::Truncated { .. })),
        "{result:?}"
    );
    assert!(
        allocated < 64 * 1024,
        "allocated {allocated} bytes for 10 that arrived"
    );
}

#[test]
fn an_oversized_length_allocates_nothing_for_the_payload() {
    let (result, allocated) = bytes_allocated_reading(&u32::MAX.to_le_bytes());
    assert!(
        matches!(result, Err(FrameError::TooLarge { .. })),
        "{result:?}"
    );
    assert!(allocated < 1024, "allocated {allocated} bytes");
}

#[test]
fn a_real_frame_allocates_about_its_size() {
    // The bound above isn't vacuous: reading a 1 MiB frame allocates at least 1 MiB.
    let len = 1024 * 1024u32;
    let mut stream = len.to_le_bytes().to_vec();
    stream.extend(vec![0u8; len as usize]);
    let (result, allocated) = bytes_allocated_reading(&stream);
    assert_eq!(result.unwrap().unwrap().len(), len as usize);
    assert!(allocated >= len as usize, "allocated {allocated} bytes");
}
