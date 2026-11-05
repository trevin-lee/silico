//! What changed between two revisions a session holds, in words (DESIGN.md's 6c and 6d; the
//! prototype's `AGENT` lists in `js/interact.js`, drawn by `renderBanner` in `js/app.js`):
//! each of `diff`'s changes as a row of the banner's list (the feature's name, its field,
//! and what the field went from and to, each value as the file writes it), and as the text
//! of the feature's purple mark. Worked out once, when `diff` answers, never each frame.

use std::collections::BTreeMap;

use silico_core::command::Problems;
use silico_core::command::wire::{self, Json, Layout};
use silico_core::document::json::Plain;
use silico_core::document::{Change, FeatureId};

/// How many of a diff's changes the banner lists: past them, it says how many more there
/// are, and in how many features.
pub const LISTED: usize = 8;

/// How many characters of a name or a value a row shows: past them, it's cut short with an
/// ellipsis, so one long value doesn't fill the canvas.
pub const SHOWN_CHARS: usize = 60;

/// How many of a feature's changes its mark says: past them, how many more.
pub const MARKED: usize = 3;

/// How many values an array's or an object's text is written with at most: past them, it's
/// shown as `[…]` or `{…}`, never written out (a sketch's whole list, say).
const WRITTEN_VALUES: usize = 32;

/// One row of the list (`.canvas-banner .diff`): the feature's name (`.k`, purple), the
/// field (a `set_field` path, or the word for a change of the feature's place), and what it
/// went from and to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub feature: String,
    pub field: String,
    pub text: String,
}

/// A diff's changes, as the banner lists them: the first [`LISTED`] as rows, and, if
/// there are more, the line that says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub rows: Vec<Row>,
    pub more: Option<String>,
    /// How many changes, and in how many features.
    pub changes: usize,
    pub features: usize,
}

/// What a `diff` the tab asked answered: nothing yet, its changes in words and the marks
/// they give, or its refusal, said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Changes {
    #[default]
    Asking,
    Listed(Listed),
    Refused(String),
}

/// `text` cut short at [`SHOWN_CHARS`] characters, with an ellipsis.
fn shown(text: &str) -> String {
    match text.char_indices().nth(SHOWN_CHARS) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// How many values `value` holds, itself among them, counting no further than `limit`.
fn values(value: &Plain, limit: usize) -> usize {
    let mut count = 0;
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        count += 1;
        if count > limit {
            return count;
        }
        match value {
            Plain::Array(items) => stack.extend(items),
            Plain::Object(members) => stack.extend(members.iter().map(|(_, value)| value)),
            Plain::Null | Plain::Bool(_) | Plain::Number(_) | Plain::String(_) => {}
        }
    }
    count
}

/// `value` as the file writes it, on one line (a number as its text, a string quoted), cut
/// short at [`SHOWN_CHARS`]; an array or an object of more than [`WRITTEN_VALUES`] values
/// as `[…]` or `{…}`.
pub fn value(value: &Plain) -> String {
    if values(value, WRITTEN_VALUES) > WRITTEN_VALUES {
        return match value {
            Plain::Array(_) => "[…]".to_owned(),
            _ => "{…}".to_owned(),
        };
    }
    match wire::write(&Json::plain(value), Layout::OneLine) {
        Ok(text) => shown(&text),
        // Never: the value is a document's, far below a response's limit.
        Err(_) => "…".to_owned(),
    }
}

/// A feature's place in the timeline (`before` or `after` of a change of place: from 0), as
/// a person counts it, from 1.
fn place(value: &Plain) -> Option<String> {
    match value {
        Plain::Number(text) => text
            .parse::<u64>()
            .ok()
            .map(|n| n.saturating_add(1).to_string()),
        _ => None,
    }
}

/// `change` as the list's row: its feature's name, then its field and what it went from and
/// to; or, for a change of the feature's place (`field: null`), whether it was added (at
/// its place), removed (from its place), or moved (from one place to the other), places
/// counted from 1.
pub fn row(change: &Change) -> Row {
    let (field, text) = match &change.field {
        Some(field) => (shown(field), values_text(&change.before, &change.after)),
        None => match (place(&change.before), place(&change.after)) {
            (None, Some(after)) => ("added".to_owned(), format!("at place {after}")),
            (Some(before), None) => ("removed".to_owned(), format!("from place {before}")),
            (Some(before), Some(after)) => {
                ("moved".to_owned(), format!("place {before} → {after}"))
            }
            // Never: a change of place has a place on one side at least.
            (None, None) => ("moved".to_owned(), String::new()),
        },
    };
    Row {
        feature: shown(&change.name),
        field,
        text,
    }
}

/// A field's value before and after, as the file writes each: `5 → 3`; or, for a field
/// one side hasn't, `added: 3` or `removed: 5`.
fn values_text(before: &Plain, after: &Plain) -> String {
    match (before, after) {
        (Plain::Null, Plain::Null) => "null → null".to_owned(),
        (Plain::Null, after) => format!("added: {}", value(after)),
        (before, Plain::Null) => format!("removed: {}", value(before)),
        (before, after) => format!("{} → {}", value(before), value(after)),
    }
}

/// `changes`, as the banner lists them ([`LISTED`] at most, then how many more).
pub fn listed(changes: &[Change]) -> Listed {
    let features = changes
        .iter()
        .map(|change| &change.feature)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let rows: Vec<Row> = changes.iter().take(LISTED).map(row).collect();
    let more = (changes.len() > rows.len()).then(|| {
        let rest = changes.len() - rows.len();
        let plural = |n: usize, one: &str, many: &str| {
            if n == 1 {
                format!("{n} {one}")
            } else {
                format!("{n} {many}")
            }
        };
        let changes = if rest == 1 { "change" } else { "changes" };
        format!(
            "and {rest} more {changes}: {} changed in all",
            plural(features, "feature", "features")
        )
    });
    Listed {
        rows,
        more,
        changes: changes.len(),
        features,
    }
}

/// The text of each changed feature's purple mark (`d.outside`), by its ID: its field and
/// what it went from and to (`radius 5 → 3`), its first [`MARKED`] changes, then how many
/// more. A feature removed has no row to mark, so none.
pub fn marks(changes: &[Change]) -> BTreeMap<FeatureId, String> {
    let mut by_feature: BTreeMap<FeatureId, (Vec<String>, usize)> = BTreeMap::new();
    for change in changes {
        let removed = change.field.is_none() && matches!(change.after, Plain::Null);
        if removed {
            continue;
        }
        let (said, count) = by_feature.entry(change.feature.clone()).or_default();
        *count += 1;
        if said.len() < MARKED {
            let row = row(change);
            said.push(if row.text.is_empty() {
                row.field
            } else {
                format!("{} {}", row.field, row.text)
            });
        }
    }
    by_feature
        .into_iter()
        .map(|(feature, (said, count))| {
            let mut text = said.join(", ");
            if count > said.len() {
                text.push_str(&format!(", and {} more", count - said.len()));
            }
            (feature, text)
        })
        .collect()
}

/// A refusal, said: its first error's message, then its candidates, if it has any.
pub fn refused(problems: &Problems) -> String {
    let Some(error) = problems.errors().first() else {
        return "refused".to_owned();
    };
    let candidates = error.candidates();
    if candidates.is_empty() {
        error.message().to_owned()
    } else {
        format!(
            "{} (candidates: {})",
            error.message(),
            candidates.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn id(text: &str) -> FeatureId {
        FeatureId::new(text).unwrap()
    }

    fn number(text: &str) -> Plain {
        Plain::Number(text.to_owned())
    }

    fn change(
        feature: &str,
        name: &str,
        field: Option<&str>,
        before: Plain,
        after: Plain,
    ) -> Change {
        Change {
            feature: id(feature),
            name: Arc::from(name),
            field: field.map(str::to_owned),
            before,
            after,
        }
    }

    #[test]
    fn a_field_reads_as_its_values_as_the_file_writes_them() {
        // Fable's example: Inside corner's radius, 5 then 3.
        let radius = change(
            "f0000001",
            "Inside corner",
            Some("radius"),
            number("5"),
            number("3"),
        );
        assert_eq!(
            row(&radius),
            Row {
                feature: "Inside corner".to_owned(),
                field: "radius".to_owned(),
                text: "5 → 3".to_owned(),
            }
        );
        // A number as its text writes it, exponent and all; a string quoted.
        let tiny = change(
            "b0000001",
            "Block",
            Some("height"),
            number("2.5"),
            number("1e-3"),
        );
        assert_eq!(row(&tiny).text, "2.5 → 1e-3");
        let named = change(
            "b0000001",
            "Base",
            Some("name"),
            Plain::String("Block".to_owned()),
            Plain::String("Base \"A\"".to_owned()),
        );
        assert_eq!(row(&named).text, r#""Block" → "Base \"A\"""#);
        // A field only one side has.
        let added = change(
            "s0000001",
            "Sketch",
            Some("points.pa000007"),
            Plain::Null,
            Plain::Object(vec![
                ("id".to_owned(), Plain::String("pa000007".to_owned())),
                ("x".to_owned(), number("1")),
            ]),
        );
        assert_eq!(row(&added).text, r#"added: {"id": "pa000007", "x": 1}"#);
        let removed = change(
            "b0000001",
            "Block",
            Some("note"),
            Plain::String("n".to_owned()),
            Plain::Null,
        );
        assert_eq!(row(&removed).text, r#"removed: "n""#);
    }

    #[test]
    fn a_change_of_place_reads_as_added_removed_or_moved_counted_from_one() {
        // Fable's examples: Boss added at place 4 (from 0), Base holes removed from 3,
        // Upright holes moved from 2 to 5.
        let boss = change("p0000002", "Boss", None, Plain::Null, number("4"));
        assert_eq!(
            (row(&boss).field, row(&boss).text),
            ("added".to_owned(), "at place 5".to_owned())
        );
        let holes = change("h0000001", "Base holes", None, number("3"), Plain::Null);
        assert_eq!(
            (row(&holes).field, row(&holes).text),
            ("removed".to_owned(), "from place 4".to_owned())
        );
        let upright = change("s0000003", "Upright holes", None, number("2"), number("5"));
        assert_eq!(
            (row(&upright).field, row(&upright).text),
            ("moved".to_owned(), "place 3 → 6".to_owned())
        );
    }

    #[test]
    fn long_names_and_values_are_cut_short_and_big_lists_never_written() {
        let long = "x".repeat(SHOWN_CHARS + 5);
        let named = change(
            "b0000001",
            &long,
            Some("name"),
            Plain::Null,
            Plain::String(long.clone()),
        );
        let row = row(&named);
        assert_eq!(row.feature, format!("{}…", "x".repeat(SHOWN_CHARS)));
        // `added: "xxx…`: the quote counts.
        assert_eq!(
            row.text,
            format!("added: \"{}…", "x".repeat(SHOWN_CHARS - 1))
        );
        let big = Plain::Array(
            (0..=WRITTEN_VALUES)
                .map(|n| number(&n.to_string()))
                .collect(),
        );
        assert_eq!(value(&big), "[…]");
        let small = Plain::Array(vec![number("0"), number("0"), number("1")]);
        assert_eq!(value(&small), "[0, 0, 1]");
    }

    #[test]
    fn the_list_says_how_many_more_past_its_first_rows() {
        let changes: Vec<Change> = (0..LISTED + 3)
            .map(|n| {
                // Two features: the first two changes the Block's, the rest the Pin's.
                let (feature, name) = if n < 2 {
                    ("b0000001", "Block")
                } else {
                    ("p0000001", "Pin")
                };
                change(
                    feature,
                    name,
                    Some(&format!("f{n}")),
                    number("1"),
                    number("2"),
                )
            })
            .collect();
        let listed = listed(&changes);
        assert_eq!(listed.rows.len(), LISTED);
        assert_eq!((listed.changes, listed.features), (11, 2));
        assert_eq!(
            listed.more.as_deref(),
            Some("and 3 more changes: 2 features changed in all")
        );
        let one = listed_one();
        assert_eq!(one.more, None);
        assert_eq!((one.changes, one.features), (1, 1));
    }

    fn listed_one() -> Listed {
        listed(&[change(
            "b0000001",
            "Block",
            Some("height"),
            number("10"),
            number("11"),
        )])
    }

    #[test]
    fn each_changed_feature_is_marked_with_its_changes_but_a_removed_one_isnt() {
        let changes = vec![
            change("p0000001", "Pin", Some("radius"), number("5"), number("3")),
            change(
                "p0000001",
                "Pin",
                Some("height"),
                number("12"),
                number("14"),
            ),
            change("b0000001", "Block", None, Plain::Null, number("0")),
            change("h0000001", "Hole", None, number("2"), Plain::Null),
        ];
        let marked = marks(&changes);
        assert_eq!(marked.len(), 2);
        assert_eq!(marked[&id("p0000001")], "radius 5 → 3, height 12 → 14");
        assert_eq!(marked[&id("b0000001")], "added at place 1");
        // Past three, how many more.
        let many: Vec<Change> = (0..5)
            .map(|n| {
                change(
                    "p0000001",
                    "Pin",
                    Some(&format!("f{n}")),
                    number("1"),
                    number("2"),
                )
            })
            .collect();
        assert_eq!(
            marks(&many)[&id("p0000001")],
            "f0 1 → 2, f1 1 → 2, f2 1 → 2, and 2 more"
        );
    }
}
