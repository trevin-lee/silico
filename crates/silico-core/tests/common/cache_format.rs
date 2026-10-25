//! The rebuild cache's format, mirrored by hand from what `silico_core::cache` documents, for
//! the tests that read and forge entries: where an entry lives, its payload, and its seal.
//! Written out here rather than taken from the cache, so a change to the format breaks
//! these tests rather than passing through them.

use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use silico_core::kernel::{History, ShapeSummary};
use silico_protocol::{Description, Failure};

use crate::cache::secret_of;

/// The first bytes of every entry, naming its format.
pub const MAGIC: &[u8] = b"silico cache 8\n";

/// This platform's name, as the cache spells it.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Where the fake kernel's entry for step `key` lives in the cache in `dir`, derived by hand
/// from the format the module documents: HMAC-SHA-256, under the cache's secret, of the
/// magic, then the kernel, the platform, and the step, each after its length (8 bytes,
/// little-endian); the first two hex digits are the subdirectory.
pub fn entry_path(dir: &Path, step: &str) -> PathBuf {
    entry_path_in(dir, MAGIC, step)
}

/// Where the fake kernel's entry for step `key` would live in the cache in `dir`, in the
/// format whose magic is `magic`.
pub fn entry_path_in(dir: &Path, magic: &[u8], step: &str) -> PathBuf {
    let platform = platform();
    let secret = std::fs::read(secret_of(dir)).unwrap();
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&secret).unwrap();
    mac.update(magic);
    for part in ["silico fake kernel 1", &platform, step] {
        mac.update(&(part.len() as u64).to_le_bytes());
        mac.update(part.as_bytes());
    }
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    dir.join(&hex[..2]).join(&hex[2..])
}

/// An entry's payload, as the module documents it (postcard is structural).
#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub key: [u8; 32],
    pub brep: Vec<u8>,
    pub valid: bool,
    pub history: Option<History>,
    pub measure: Measure,
}

/// An entry's measure: none, its summary and what's known of its description, or the kernel
/// worker's refusal of its summary, as data, and the operation it was for (the protocol's
/// own `Failure` and `Description`: their layouts are the protocol's).
#[derive(Serialize, Deserialize)]
pub enum Measure {
    Unmeasured,
    Measured {
        summary: ShapeSummary,
        description: KeptDescription,
    },
    Refused {
        op: String,
        failure: Failure,
    },
}

/// What an entry keeps of its shape's description beside its summary: nothing (asked
/// again), the description, or the kernel worker's refusal of it, as data.
#[derive(Serialize, Deserialize)]
pub enum KeptDescription {
    Unknown,
    Described(Description),
    Refused { op: String, failure: Failure },
}

/// An entry's payload, from its bytes (past the magic and the seal).
pub fn decode(bytes: &[u8]) -> Entry {
    let payload = &bytes[MAGIC.len() + 32..];
    postcard::from_bytes(payload).unwrap()
}

/// An entry's bytes, as the module documents them: the magic, the payload's HMAC-SHA-256
/// under `secret`, and the payload.
pub fn encode_with(secret: &[u8], entry: &impl Serialize) -> Vec<u8> {
    let payload = postcard::to_allocvec(entry).unwrap();
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).unwrap();
    mac.update(&payload);
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&mac.finalize().into_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// An entry's bytes, sealed with the secret of the cache in `dir`, as its own writer would.
pub fn encode(dir: &Path, entry: &impl Serialize) -> Vec<u8> {
    encode_with(&std::fs::read(secret_of(dir)).unwrap(), entry)
}
