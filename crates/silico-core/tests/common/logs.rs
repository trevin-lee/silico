//! For tests of `read_logs` (`silico_core::command::logs`): the process's subscriber the
//! core's log layer alone, a fake kernel that refuses fillets, sessions on it, a document
//! of a box and a fillet, and reads of a session's logs.
#![allow(dead_code)]

use std::sync::Arc;

use silico_core::cache::Cache;
use silico_core::command::logs::{LogLevel, LogQuery, Logs};
use silico_core::command::{CheckModel, SessionId, Sessions};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::{Error, ErrorCode};
use tempfile::TempDir;
use tracing_subscriber::layer::SubscriberExt;

use crate::sessions::{answer, id};

/// A box and a fillet of one of its edges, `fillet` its ID.
pub fn rounded(fillet: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}}, {{"id": "{fillet}", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1"], "radius": 2}}]}}"#
    )
}

/// Sets the process's subscriber to the core's log layer alone: each test that calls it
/// needs a process of its own (nextest runs each in one).
pub fn keep_logs() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(silico_core::command::logs::layer()),
    )
    .unwrap();
}

/// A fake kernel that refuses every fillet with `message` (and `cause`, if given), as a
/// kernel worker refuses one OCCT can't build.
pub fn refusing_fillets_with(message: String, cause: Option<String>) -> Arc<FakeKernel> {
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    kernel.refuse(move |op| {
        matches!(op, Op::Fillet { .. }).then(|| {
            let error = Error::new(ErrorCode::KernelFailed, message.clone());
            match &cause {
                Some(cause) => error.with_cause(cause.clone()),
                None => error,
            }
        })
    });
    kernel
}

/// A fake kernel that refuses every fillet: `Fillet failed`, with no cause.
pub fn refusing_fillets() -> Arc<FakeKernel> {
    refusing_fillets_with("Fillet failed".to_owned(), None)
}

/// Sessions on `kernel` in `dir`, their cache sealed with a secret there.
pub fn sessions(kernel: Arc<FakeKernel>, dir: &TempDir) -> Sessions<Arc<FakeKernel>> {
    let secret = dir.path().join("secret");
    Sessions::with_cache_opener(kernel, move |cache| Cache::with_secret(cache, &secret))
}

/// The document `text` written as `name` in `dir` and opened.
pub fn opened(
    sessions: &Sessions<Arc<FakeKernel>>,
    dir: &TempDir,
    name: &str,
    text: &str,
) -> SessionId {
    let path = dir.path().join(name);
    std::fs::write(&path, text).unwrap();
    answer(sessions.open(&path, false)).session.unwrap().id
}

pub fn query(level: LogLevel, feature: Option<&str>, after: Option<u64>, limit: u32) -> LogQuery {
    LogQuery {
        level,
        feature: feature.map(id),
        after,
        limit,
    }
}

/// What `read_logs` answers, which must be events, of the session asked and no model.
pub fn read(sessions: &Sessions<Arc<FakeKernel>>, session: &SessionId, query: &LogQuery) -> Logs {
    let response = sessions.read_logs(session, query);
    assert_eq!(response.session.as_ref().map(|s| s.id), Some(*session));
    assert!(response.model.is_none());
    response.result.unwrap()
}

/// Every event `session` kept, from the oldest.
pub fn every(sessions: &Sessions<Arc<FakeKernel>>, session: &SessionId) -> Logs {
    read(sessions, session, &query(LogLevel::Trace, None, None, 1000))
}

/// `session` checked, which must answer.
pub fn checked(sessions: &Sessions<Arc<FakeKernel>>, session: &SessionId, fresh: bool) {
    let response = answer(sessions.check(session, CheckModel::Current { fresh }));
    assert!(response.result.is_ok(), "{response:?}");
}
