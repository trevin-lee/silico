//! `cache_clear`'s words (the design's section 7, decision 25): the cache's directory as the
//! session's path names it, and what a clear couldn't do, as the errors it's refused with.

use std::path::Path;

use super::Problems;
use crate::cache::{ClearFailure, ClearStep, PartlyCleared};
use crate::file::{self, Workspace, shown_path};
use crate::{Error, ErrorCode};

/// The rebuild cache's directory, `cache_dir` (where it is, through links), as the session's
/// path, `path` (as `open` was given it, relative to the `workspace`'s root if the sessions
/// have one), names it, as every result names a path: beside that path (`.silico-cache`
/// beside `bracket.silico`, `parts/.silico-cache` beside `parts/bracket.silico`) when the
/// directory it names is the one the cache is in; otherwise (a path that's a link from
/// another directory to the document, or one whose directory leads out of the workspace
/// now) where it is, through links: relative to the workspace's root if there's one, so a
/// result never quotes where the workspace is (docs/design/m4.9-mcp.md, the bracket).
pub(super) fn shown_cache_directory(
    workspace: Option<&Workspace>,
    path: &str,
    cache_dir: &Path,
) -> String {
    let given = Path::new(path);
    let beside = file::resolve(workspace, file::directory(given))
        .is_ok_and(|named| Some(named.as_path()) == cache_dir.parent());
    match cache_dir.file_name() {
        Some(name) if beside => given.with_file_name(name).to_string_lossy().into_owned(),
        _ => workspace
            .and_then(|workspace| cache_dir.strip_prefix(workspace.root()).ok())
            .unwrap_or(cache_dir)
            .to_string_lossy()
            .into_owned(),
    }
}

/// What a clear of the cache in `cache_dir`, shown as `directory`, couldn't do: an
/// `io_failed` for each file it couldn't remove and each directory it couldn't read, named
/// as `directory` names the cache, the first hundred listed and all counted, each saying how
/// many entries it removed besides.
pub(super) fn partly_cleared_problems(
    directory: &str,
    cache_dir: &Path,
    partly: PartlyCleared,
) -> Problems {
    let PartlyCleared {
        removed,
        failures,
        total,
    } = partly;
    let listed = failures.len() as u64;
    let shown = |path: &Path| {
        let named = match path.strip_prefix(cache_dir) {
            Ok(inside) if inside.as_os_str().is_empty() => Path::new(directory).to_owned(),
            Ok(inside) => Path::new(directory).join(inside),
            Err(_) => path.to_owned(),
        };
        shown_path(&named)
    };
    let entries = |n: u64| if n == 1 { "entry" } else { "entries" };
    let errors: Vec<Error> = failures
        .into_iter()
        .map(|ClearFailure { path, step, why }| {
            let message = match step {
                ClearStep::Remove => {
                    format!("couldn't remove the cache's entry {}: {why}", shown(&path))
                }
                ClearStep::Read => format!(
                    "couldn't read the cache's directory {}: {why}",
                    shown(&path)
                ),
            };
            Error::new(ErrorCode::IoFailed, message)
                .with_cause(format!(
                    "the clear removed {removed} {} and left what it couldn't remove or read",
                    entries(removed)
                ))
                .with_suggestion(
                    "check that this user may read and write the cache's directory and its \
                     subdirectories, then clear it again: an entry left is still sound, and \
                     one removed is only a miss",
                )
        })
        .collect();
    Problems::with_unlisted(errors, total.saturating_sub(listed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_is_named_beside_the_path_as_given() {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        let parts = real.join("parts");
        std::fs::create_dir(&parts).unwrap();
        let cache = parts.join(".silico-cache");
        // An absolute path in the cache's directory: the cache beside it.
        let given = parts.join("bracket.silico");
        assert_eq!(
            shown_cache_directory(None, given.to_str().unwrap(), &cache),
            cache.to_str().unwrap()
        );
        // A path through a link to that directory names it through the link.
        let linked = real.join("linked");
        std::os::unix::fs::symlink(&parts, &linked).unwrap();
        assert_eq!(
            shown_cache_directory(
                None,
                linked.join("bracket.silico").to_str().unwrap(),
                &cache
            ),
            linked.join(".silico-cache").to_str().unwrap()
        );
        // A path in another directory (a link from there to the document): where the cache
        // is.
        assert_eq!(
            shown_cache_directory(None, real.join("bracket.silico").to_str().unwrap(), &cache),
            cache.to_str().unwrap()
        );
    }

    #[test]
    fn in_a_workspace_the_directory_is_named_from_its_root_never_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        let parts = real.join("parts");
        std::fs::create_dir(&parts).unwrap();
        let cache = parts.join(".silico-cache");
        let workspace = Workspace::new(&real).unwrap();
        let workspace = Some(&workspace);
        // A path relative to the root, as `open` takes it: beside it, whatever the process's
        // working directory (never this one).
        assert_eq!(
            shown_cache_directory(workspace, "parts/bracket.silico", &cache),
            "parts/.silico-cache"
        );
        assert_eq!(
            shown_cache_directory(workspace, "parts/../parts/bracket.silico", &cache),
            "parts/../parts/.silico-cache"
        );
        // A path in another directory (a link from there to the document): where the cache
        // is, from the root.
        assert_eq!(
            shown_cache_directory(workspace, "bracket.silico", &cache),
            "parts/.silico-cache"
        );
        // Its directory swapped for a link out of the workspace: still from the root.
        let outside = tempfile::tempdir().unwrap();
        std::fs::remove_dir(&parts).unwrap();
        std::os::unix::fs::symlink(outside.path(), &parts).unwrap();
        assert_eq!(
            shown_cache_directory(workspace, "parts/bracket.silico", &cache),
            "parts/.silico-cache"
        );
    }

    #[test]
    fn what_a_clear_couldnt_do_is_an_io_failed_each_the_first_hundred_listed() {
        let cache = Path::new("/work/.silico-cache");
        let denied = || std::io::Error::from_raw_os_error(libc::EACCES);
        let failures: Vec<_> = (0..100)
            .map(|n| ClearFailure {
                path: cache.join(format!("{n:02x}")),
                step: ClearStep::Read,
                why: denied(),
            })
            .collect();
        let problems = partly_cleared_problems(
            ".silico-cache",
            cache,
            PartlyCleared {
                removed: 1,
                failures,
                total: 103,
            },
        );
        assert_eq!(problems.total(), 103);
        assert_eq!(problems.errors().len(), 100);
        let first = &problems.errors()[0];
        assert_eq!(first.code(), ErrorCode::IoFailed);
        assert_eq!(
            first.message(),
            format!(
                "couldn't read the cache's directory \".silico-cache/00\": {}",
                denied()
            )
        );
        assert_eq!(
            first.cause(),
            Some("the clear removed 1 entry and left what it couldn't remove or read")
        );
        assert_eq!(first.parameter(), None);
        let removed = partly_cleared_problems(
            "parts/.silico-cache",
            cache,
            PartlyCleared {
                removed: 2,
                failures: vec![ClearFailure {
                    path: cache.join("ab").join("cd"),
                    step: ClearStep::Remove,
                    why: denied(),
                }],
                total: 1,
            },
        );
        assert_eq!(
            removed.errors()[0].message(),
            format!(
                "couldn't remove the cache's entry \"parts/.silico-cache/ab/cd\": {}",
                denied()
            )
        );
        assert_eq!(
            removed.errors()[0].cause(),
            Some("the clear removed 2 entries and left what it couldn't remove or read")
        );
    }
}
