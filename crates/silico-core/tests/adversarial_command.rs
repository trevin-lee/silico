//! break-it for M2.9's core commands (`command::check`, `format`, `outline`): hostile
//! bytes, names and notes that try to hide in or break the outline, documents at the size
//! limit, and a kernel that refuses. Expected values come from the commands' own contract
//! (one line per feature, escaped names, idempotent formatting), Unicode's own lists of
//! bidirectional and format characters (UAX #9, General_Category=Cf), and hand counts.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;

use example::EXAMPLE;
use proptest::prelude::*;
use silico_core::command;
use silico_core::document::{self, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Op, Primitive};
use silico_core::recompute::Recompute;
use silico_core::{Error, ErrorCode};

/// `command::check` of a document's bytes: the gate's problems, or the rebuild's.
fn check_bytes<K: silico_core::kernel::Kernel + 'static>(
    engine: &silico_core::recompute::Recompute<K>,
    bytes: &[u8],
) -> Result<silico_core::command::Problems, silico_core::Error> {
    match silico_core::command::gate(bytes) {
        Ok(checked) => silico_core::command::check(engine, checked),
        Err(problems) => Ok(problems),
    }
}

/// `command::format` of a document's bytes: the gate's problems, or the formatted text.
fn format_bytes<K: silico_core::kernel::Kernel + 'static>(
    engine: &silico_core::recompute::Recompute<K>,
    bytes: &[u8],
) -> Result<
    Result<silico_core::command::Formatted, silico_core::command::Problems>,
    silico_core::Error,
> {
    match silico_core::command::gate(bytes) {
        Ok(checked) => silico_core::command::format(engine, checked),
        Err(problems) => Ok(Err(problems)),
    }
}

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap())
}

/// The example with the block's name replaced by `name` (written as JSON).
fn named(name: &str) -> String {
    EXAMPLE.replace(
        r#""name": "Block""#,
        &format!(r#""name": {}"#, serde_json::to_string(name).unwrap()),
    )
}

fn outline_of(text: &str) -> String {
    command::outline(&read(text.as_bytes()).unwrap())
}

// Unicode's bidirectional formatting characters (UAX #9, table 4, and the Trojan Source
// paper's list): each moves the text around it when shown. The outline promises a name
// can't hide or move text in its line.
const BIDI_CONTROLS: &[char] = &[
    '\u{061c}', // ARABIC LETTER MARK
    '\u{200e}', '\u{200f}', // LRM, RLM
    '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', // LRE RLE PDF LRO RLO
    '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}', // LRI RLI FSI PDI
];

#[test]
fn an_outline_escapes_every_bidi_control_in_a_name() {
    // BREAKS: `invisible` lists U+202A..202E and U+200E/F but not the isolates U+2066..2069
    // (LRI, RLI, FSI, PDI) or U+061C (ARABIC LETTER MARK), so `silico show` prints them raw:
    // a name of "\u{2067}" + "tool a91cw0de" reorders how the line reads in a terminal or a
    // `git diff`, the Trojan Source attack the escaping exists to stop.
    let mut raw = Vec::new();
    for &c in BIDI_CONTROLS {
        let outline = outline_of(&named(&format!("x{c}y")));
        let line = outline.lines().next().unwrap();
        if line.contains(c) {
            raw.push(format!("U+{:04X}", c as u32));
        } else {
            assert!(
                line.contains(&format!("\\u{{{:x}}}", c as u32)),
                "U+{:04X} neither raw nor escaped: {line}",
                c as u32
            );
        }
    }
    assert!(raw.is_empty(), "printed raw in the outline: {raw:?}");
}

#[test]
fn an_outline_escapes_every_format_character_in_a_note() {
    // BREAKS: format characters (General_Category=Cf) print as nothing, so a note holding
    // them looks like a different note; `invisible` escapes only some. Raw here: the soft
    // hyphen, the Mongolian vowel separator, word joiner's neighbours U+2065..206F, the
    // interlinear annotation marks, and the tag characters (U+E0001, U+E0020..E007F), which
    // can spell hidden ASCII text inside a note.
    let format_chars: Vec<char> = [
        0xad, 0x600, 0x605, 0x61c, 0x6dd, 0x70f, 0x180e, 0x200b, 0x200f, 0x202a, 0x202e, 0x2060,
        0x2064, 0x2066, 0x206f, 0xfeff, 0xfff9, 0xfffb, 0x110bd, 0x1d173, 0xe0001, 0xe0041,
        0xe007f,
    ]
    .into_iter()
    .filter_map(char::from_u32)
    .collect();
    let mut raw = Vec::new();
    for c in format_chars {
        let text = EXAMPLE.replace(
            r#""note": "Clearance for an M5 bolt""#,
            &format!(
                r#""note": {}"#,
                serde_json::to_string(&format!("a{c}b")).unwrap()
            ),
        );
        let outline = outline_of(&text);
        if outline.contains(c) {
            raw.push(format!("U+{:04X}", c as u32));
        }
    }
    assert!(raw.is_empty(), "printed raw in the outline: {raw:?}");
}

#[test]
fn an_outline_escapes_c0_c1_and_del_and_line_separators() {
    // Every C0 and C1 control, DEL, U+2028 and U+2029: each escaped, one line per feature.
    let mut chars: Vec<char> = (0u32..0x20)
        .chain(0x7f..0xa0)
        .filter_map(char::from_u32)
        .collect();
    chars.extend(['\u{2028}', '\u{2029}', '\u{85}']);
    let name: String = chars.iter().collect();
    let outline = outline_of(&named(&name));
    let first = outline.split('\n').next().unwrap();
    for c in &chars {
        assert!(!first.contains(*c), "U+{:04X} raw: {first:?}", *c as u32);
    }
    assert_eq!(outline.lines().count(), 5, "{outline:?}");
    assert!(
        outline.starts_with("k7f2qx3m \"\\u{0}\\u{1}"),
        "{outline:?}"
    );
    assert!(outline.contains("\\u{1b}"), "ESC escaped: {outline:?}");
}

#[test]
fn an_outline_of_a_name_that_looks_like_the_line_after_it_stays_in_quotes() {
    // A name spelled as the rest of an outline line: still one quoted token, its quotes
    // escaped, so nothing after the closing quote is the name's.
    let fake = r#"Block" box length 1, width 1, height 1, at (0, 0, 0)"#;
    let outline = outline_of(&named(fake));
    assert_eq!(
        outline.lines().next().unwrap(),
        r#"k7f2qx3m "Block\" box length 1, width 1, height 1, at (0, 0, 0)" box length 40, width 20, height 10, at (0, 0, 0)"#
    );
}

#[test]
fn an_outline_of_a_ten_megabyte_name_is_that_name_once() {
    let name = "n".repeat(10 << 20);
    let outline = outline_of(&named(&name));
    assert_eq!(outline.lines().count(), 5);
    assert_eq!(outline.len(), OUTLINE_LEN - "Block".len() + name.len());
}

/// The example's outline length (tests/command.rs spells it out).
const OUTLINE_LEN: usize = r#"k7f2qx3m "Block" box length 40, width 20, height 10, at (0, 0, 0)
a91cw0de "Pin" cylinder radius 2.5, height 12, at (20, 10, -1)
m3tq8v0z "Hole" cut base k7f2qx3m, tool a91cw0de
  note "Clearance for an M5 bolt"
bodies m3tq8v0z
"#
.len();

#[test]
fn an_outline_of_a_document_the_gate_refuses_still_says_each_feature() {
    // `show` doesn't gate: a dangling tool and a cut that uses itself are shown as written,
    // and the bodies are what no feature consumes (the cut consumes itself: none).
    let text = EXAMPLE.replace(r#""tool": "a91cw0de""#, r#""tool": "m3tq8v0z""#);
    let outline = outline_of(&text);
    assert!(
        outline.contains("cut base k7f2qx3m, tool m3tq8v0z"),
        "{outline}"
    );
    assert!(outline.ends_with("bodies a91cw0de\n"), "{outline}");
}

#[test]
fn an_outline_of_no_features_says_no_bodies() {
    let text = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "features": []
}
"#;
    assert_eq!(outline_of(text), "bodies none\n");
}

#[test]
fn an_outline_writes_lengths_exactly_at_the_documents_extremes() {
    // The smallest step the resolution allows, and a large negative position: written as
    // the file writes them, no float rounding, no exponent.
    let text = EXAMPLE
        .replace(r#""length": 40"#, r#""length": 0.000001"#)
        .replace(
            r#""position": [0, 0, 0]"#,
            r#""position": [-999999.999999, 1e3, 0]"#,
        );
    let line = outline_of(&text).lines().next().unwrap().to_owned();
    assert_eq!(
        line,
        r#"k7f2qx3m "Block" box length 0.000001, width 20, height 10, at (-999999.999999, 1000, 0)"#
    );
}

#[test]
fn checking_a_document_just_under_the_size_limit_then_formatting_it_says_why_it_cant_be_saved() {
    // Two boxes whose notes take the document to within a few bytes of the limit: it reads
    // (and checks clean, with nothing stored to drift from), but the fingerprints a save
    // adds take it past the limit. `format` must say so as a problem, not write it or fail.
    let limit = document::json::MAX_DOCUMENT_LEN;
    let base = read(EXAMPLE.as_bytes()).unwrap();
    let text = document::write(&base).unwrap();
    let pad = limit - text.len() - 64;
    let padded = text.replacen(
        r#""Clearance for an M5 bolt""#,
        &format!("\"{}\"", "x".repeat(pad)),
        1,
    );
    assert!(padded.len() <= limit && padded.len() > limit - 128);
    let problems = check_bytes(&engine(), padded.as_bytes()).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    let problems = format_bytes(&engine(), padded.as_bytes())
        .unwrap()
        .unwrap_err();
    assert_eq!(problems.total, 1);
    let error = &problems.problems[0];
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(error.to_string().contains(&limit.to_string()), "{error}");
}

#[test]
fn a_refusal_doesnt_leave_the_engine_broken_for_the_next_command() {
    // After a refused rebuild and a document that doesn't read, the same engine checks and
    // formats another sound document cleanly: nothing stale from the earlier runs. (The
    // refused box's failure is kept for its key, by recompute's design, so the document
    // after is a different box.)
    let e = engine();
    let refusing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = std::sync::Arc::clone(&refusing);
    e.kernel().refuse(move |op| {
        (flag.load(std::sync::atomic::Ordering::SeqCst)
            && matches!(op, Op::MakePrimitive(Primitive::Box { .. })))
        .then(|| Error::new(ErrorCode::KernelFailed, "MakeBox failed"))
    });
    assert_eq!(check_bytes(&e, EXAMPLE.as_bytes()).unwrap().total, 1);
    assert_eq!(check_bytes(&e, b"\xff").unwrap().total, 1);
    refusing.store(false, std::sync::atomic::Ordering::SeqCst);
    let other = EXAMPLE.replace(r#""length": 40"#, r#""length": 41"#);
    let out = format_bytes(&e, other.as_bytes()).unwrap().unwrap();
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert_eq!(out.refingerprinted.len(), 3);
    assert!(check_bytes(&e, out.text.as_bytes()).unwrap().is_empty());
}

#[test]
fn more_problems_than_the_gate_lists_report_their_total() {
    // 150 boxes each 0 long: the gate lists 100 and counts 150.
    let mut features = Vec::new();
    for i in 0..150 {
        features.push(format!(
            r#"{{"id": "b{i:07}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 0, "width": 1, "height": 1}}"#
        ));
    }
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
        features.join(",")
    );
    let problems = check_bytes(&engine(), text.as_bytes()).unwrap();
    assert_eq!(problems.total, 150);
    assert_eq!(problems.problems.len(), 100);
    assert!(!problems.is_empty());
    let problems = format_bytes(&engine(), text.as_bytes())
        .unwrap()
        .unwrap_err();
    assert_eq!(problems.total, 150);
}

// Hostile bytes: the example cut, spliced, and flipped at random. Every outcome must be a
// list of problems (or a clean check), never an `Err` (the engine is sound) and never a
// panic; and the same bytes always give the same answer.
fn mutated() -> impl Strategy<Value = Vec<u8>> {
    let base = EXAMPLE.as_bytes().to_vec();
    let len = base.len();
    prop_oneof![
        (0..=len).prop_map({
            let base = base.clone();
            move |cut| base[..cut].to_vec()
        }),
        (0..len, any::<u8>()).prop_map({
            let base = base.clone();
            move |(at, b)| {
                let mut v = base.clone();
                v[at] = b;
                v
            }
        }),
        (
            0..len,
            prop::collection::vec(
                prop_oneof![
                    Just(b'"'),
                    Just(b'\\'),
                    Just(b'{'),
                    Just(b'}'),
                    Just(b'['),
                    Just(b','),
                    Just(b'0'),
                    Just(b'-'),
                    Just(b'e'),
                    Just(0xff),
                    Just(0xc0),
                    Just(0x00)
                ],
                1..6,
            )
        )
            .prop_map({
                let base = base.clone();
                move |(at, ins)| {
                    let mut v = base.clone();
                    v.splice(at..at, ins);
                    v
                }
            }),
        prop::collection::vec(any::<u8>(), 0..64),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn hostile_bytes_are_problems_never_a_panic_and_the_same_each_time(bytes in mutated()) {
        let e = engine();
        let once = check_bytes(&e, &bytes);
        prop_assert!(once.is_ok(), "{once:?}");
        let twice = check_bytes(&e, &bytes);
        prop_assert_eq!(&once, &twice);
        let problems = once.unwrap();
        prop_assert!(problems.total >= problems.problems.len() as u64);
        match format_bytes(&e, &bytes).unwrap() {
            Ok(formatted) => {
                // It read: the canonical text reads back, formats to itself, and checks
                // clean.
                let back = read(formatted.text.as_bytes());
                prop_assert!(back.is_ok(), "{back:?}");
                let again = format_bytes(&e, formatted.text.as_bytes()).unwrap().unwrap();
                prop_assert_eq!(&again.text, &formatted.text);
                prop_assert!(again.refingerprinted.is_empty());
                prop_assert!(check_bytes(&e, formatted.text.as_bytes()).unwrap().is_empty());
                // And show says each feature on a line, each note on one more.
                let doc = back.unwrap();
                let notes = doc.features.iter().filter(|f| f.note.is_some()).count();
                prop_assert_eq!(command::outline(&doc).lines().count(), doc.features.len() + notes + 1);
            }
            Err(p) => prop_assert!(!p.is_empty()),
        }
    }

    #[test]
    fn any_name_or_note_is_one_quoted_line_of_the_outline(
        name in prop_oneof![
            "\\PC{1,12}",
            any::<String>().prop_filter("not empty", |s| !s.is_empty()),
            prop::collection::vec(
                prop_oneof![
                    Just('\n'), Just('\r'), Just('"'), Just('\\'), Just('\u{1b}'),
                    Just('\u{2028}'), Just('\u{85}'), Just('\u{0}'), Just('a'),
                ],
                1..10,
            ).prop_map(|v| v.into_iter().collect::<String>()),
        ],
        note in any::<String>(),
    ) {
        let text = named(&name).replace(
            r#""note": "Clearance for an M5 bolt""#,
            &format!(r#""note": {}"#, serde_json::to_string(&note).unwrap()),
        );
        let outline = outline_of(&text);
        prop_assert_eq!(outline.lines().count(), 5, "{:?}", outline);
        // `lines` splits only at \n; nothing else that ends a line in a terminal or an
        // editor may appear raw.
        for c in ['\r', '\u{2028}', '\u{2029}', '\u{85}', '\u{b}', '\u{c}'] {
            prop_assert!(!outline.contains(c), "{:?}", outline);
        }
        // The name sits between the ID and the type, quoted: unescaping what's between the
        // first `"` and the closing one gives it back.
        let first = outline.lines().next().unwrap();
        let quoted = first.strip_prefix("k7f2qx3m ").unwrap();
        let end = quoted.rfind("\" box length 40").unwrap();
        prop_assert_eq!(unescape(&quoted[1..end]), name);
    }
}

/// The outline's escapes undone: `\"`, `\\`, and `\u{hex}`.
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                assert_eq!(chars.next(), Some('{'));
                let hex: String = chars.by_ref().take_while(|&c| c != '}').collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
            }
            Some(other) => out.push(other),
            None => panic!("a lone backslash"),
        }
    }
    out
}
