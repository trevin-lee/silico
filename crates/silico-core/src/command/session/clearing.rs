//! `cache_clear`'s words (the design's section 7, decision 25): the cache's directory as the
//! session's path names it, and what a clear couldn't do, as the errors it's refused with.

use std::path::Path;

use super::Problems;
use crate::cache::{Unclear, Uncleared};
use crate::{Error, ErrorCode};

/// The rebuild cache's directory, `cache_dir` (where it is, through links), as the session's
/// path, `path` (as `open` was given it), names it, as every result names a path: beside
/// that path (`.silico-cache` beside `bracket.silico`, `parts/.silico-cache` beside
/// `parts/bracket.silico`) when the directory it names is the one the cache is in; otherwise
/// (a path that's a link from another directory to the document) where it is, through links.
pub(super) fn shown_cache_directory(path: &str, cache_dir: &Path) -> String {
    let given = Path::new(path).parent().unwrap_or(Path::new(""));
    let named = if given.as_os_str().is_empty() {
        Path::new(".")
    } else {
        given
    };
    let beside =
        std::fs::canonicalize(named).is_ok_and(|named| Some(named.as_path()) == cache_dir.parent());
    match cache_dir.file_name() {
        Some(name) if beside => given.join(name).to_string_lossy().into_owned(),
        _ => cache_dir.to_string_lossy().into_owned(),
    }
}

/// What a clear of the cache in `cache_dir`, shown as `directory`, couldn't do: an
/// `io_failed` for each file it couldn't remove and each directory it couldn't read, named
/// as `directory` names the cache, the first hundred listed and all counted, each saying how
/// many entries it removed besides.
pub(super) fn uncleared_problems(
    directory: &str,
    cache_dir: &Path,
    uncleared: Uncleared,
) -> Problems {
    let Uncleared {
        removed,
        failures,
        total,
    } = uncleared;
    let listed = failures.len() as u64;
    let shown = |path: &Path| {
        let named = match path.strip_prefix(cache_dir) {
            Ok(inside) if inside.as_os_str().is_empty() => Path::new(directory).to_owned(),
            Ok(inside) => Path::new(directory).join(inside),
            Err(_) => path.to_owned(),
        };
        crate::command::quoted(&named.to_string_lossy())
    };
    let entries = |n: u64| if n == 1 { "entry" } else { "entries" };
    let errors: Vec<Error> = failures
        .into_iter()
        .map(|(path, what, why)| {
            let message = match what {
                Unclear::Remove => {
                    format!("couldn't remove the cache's entry {}: {why}", shown(&path))
                }
                Unclear::Read => format!(
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
            shown_cache_directory(given.to_str().unwrap(), &cache),
            cache.to_str().unwrap()
        );
        // A path through a link to that directory names it through the link.
        let linked = real.join("linked");
        std::os::unix::fs::symlink(&parts, &linked).unwrap();
        assert_eq!(
            shown_cache_directory(linked.join("bracket.silico").to_str().unwrap(), &cache),
            linked.join(".silico-cache").to_str().unwrap()
        );
        // A path in another directory (a link from there to the document): where the cache
        // is.
        assert_eq!(
            shown_cache_directory(real.join("bracket.silico").to_str().unwrap(), &cache),
            cache.to_str().unwrap()
        );
    }

    #[test]
    fn what_a_clear_couldnt_do_is_an_io_failed_each_the_first_hundred_listed() {
        let cache = Path::new("/work/.silico-cache");
        let denied = || std::io::Error::from_raw_os_error(libc::EACCES);
        let failures: Vec<_> = (0..100)
            .map(|n| (cache.join(format!("{n:02x}")), Unclear::Read, denied()))
            .collect();
        let problems = uncleared_problems(
            ".silico-cache",
            cache,
            Uncleared {
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
        let removed = uncleared_problems(
            "parts/.silico-cache",
            cache,
            Uncleared {
                removed: 2,
                failures: vec![(cache.join("ab").join("cd"), Unclear::Remove, denied())],
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
