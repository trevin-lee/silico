//! B-reps OCCT wrote (tests/fixtures/brep), read as `from_brep` reads Silico's own: with its
//! checksum appended.
#![allow(dead_code)]

/// The fixture `name`, with Silico's checksum.
pub fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/brep")
        .join(name);
    with_crc(std::fs::read(path).unwrap())
}

/// `body` with Silico's B-rep checksum appended (a CRC-32, little-endian).
pub fn with_crc(mut body: Vec<u8>) -> Vec<u8> {
    body.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
    body
}
