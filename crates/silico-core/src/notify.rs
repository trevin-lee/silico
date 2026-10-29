//! A waker: what the core tells, with no lock held, when something a client draws changes
//! (a drag's frame, a disk comparison's finding), so the client needn't wait or poll.

use std::sync::Arc;

/// Told of a change, with no lock held: the caller's waker.
pub type Notify = Arc<dyn Fn() + Send + Sync>;
