//! `cache_clear`'s wire types (docs/design/m4.1-command-api.md, section 7, and decision 25):
//! its request, the [`SessionRequest`](super::transaction::SessionRequest) it takes, read
//! strictly; its result, written field for field ([`ToWire`]: [`CacheCleared`], what
//! [`Sessions::cache_clear`](crate::command::Sessions::cache_clear) gives); and the mirrors
//! its snapshot, `commands/cache_clear.json`, is generated from. What it removes, and what it
//! leaves, the session and the cache say.

use schemars::JsonSchema;

use super::{Json, ToWire, wire_shape};
use crate::command::CacheCleared;
use crate::command::session::SESSION_ID_PATTERN;

impl ToWire for CacheCleared {
    /// The cache's directory, as the session's path names it, and how many entries were
    /// removed.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("directory", Json::str(&self.directory)),
            ("removed", Json::whole(self.removed)),
        ])
    }
}

/// `cache_clear`'s request: remove every entry of the rebuild cache beside the session's
/// document. The cache is the directory's, shared by every document in it, and an entry
/// removed is only a miss: the next rebuild of each step makes it again.
#[derive(JsonSchema)]
#[schemars(rename = "CacheClearRequest", transform = wire_shape)]
pub struct CacheClearRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
}

/// `cache_clear`'s result. Only files the cache made as entries are removed: never another
/// file in the directory, and nothing through a link. A cache that leads out of the
/// workspace through a link isn't cleared: the command is refused, saying so.
#[derive(JsonSchema)]
#[schemars(rename = "CacheClearResult", transform = wire_shape)]
pub struct CacheClearResultSchema {
    /// The cache's directory (`.silico-cache` beside the document), named as the session's
    /// path names the document's.
    pub directory: String,
    /// How many entries were removed.
    pub removed: u64,
}
