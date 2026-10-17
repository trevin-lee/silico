//! FreeCAD's form of a name, and FreeCAD's order of names, for two uses: comparing Silico's
//! names with FreeCAD's in tests, and choosing, where FreeCAD chooses by name, the same
//! element FreeCAD would (which source comes first, which element above or below an element
//! names it).
//!
//! The form is FreeCAD's without its tags (`;:H<tag>`, `:<length>`): FreeCAD's tags are
//! session object IDs, and its own test matchers strip them too. Silico's names keep an
//! element's name through features that keep it (N2), where FreeCAD's add a tag per
//! feature, so without tags the two forms agree.

use std::cmp::Ordering;

use super::{Name, Relation, count_text, relation_text};

/// FreeCAD's form of `name`, without tags: `Face6;:G2(Face1;K-2);CUT`.
pub fn text(name: &Name) -> String {
    let mut out = String::new();
    write(name, &mut out);
    out
}

fn write(name: &Name, out: &mut String) {
    match name {
        Name::Element { element, .. } => out.push_str(&element.to_string()),
        Name::Derived {
            operation,
            first,
            more,
            ..
        } => {
            write(&first.name, out);
            // Modified, generated, or (sources disagreeing) both, with the first's position.
            out.push_str(";:");
            if more.iter().any(|o| o.relation != first.relation) {
                out.push_str("MG");
                out.push_str(&count_text(first.position));
            } else {
                out.push_str(&relation_text(first.relation, first.position));
            }
            if !more.is_empty() {
                out.push('(');
                for (i, other) in more.iter().enumerate() {
                    if i > 0 {
                        out.push('|');
                    }
                    write(&other.name, out);
                    // FreeCAD's signed position: generated negative; left out for the first
                    // modified.
                    let position = i64::from(other.position.get());
                    let signed = match other.relation {
                        Relation::Modified => position,
                        Relation::Generated => -position,
                    };
                    if signed != 1 {
                        out.push_str(";K");
                        out.push_str(&signed.to_string());
                    }
                }
                out.push(')');
            }
            out.push(';');
            out.push_str(operation.code());
        }
        Name::Upper {
            operation,
            upper,
            nth,
            ..
        } => {
            write(upper, out);
            out.push_str(";:U");
            out.push_str(&count_text(*nth));
            out.push(';');
            out.push_str(operation.code());
        }
        Name::Lower {
            operation,
            first,
            more,
            ..
        } => {
            write(first, out);
            out.push_str(";:L");
            if !more.is_empty() {
                out.push('(');
                for (i, lower) in more.iter().enumerate() {
                    if i > 0 {
                        out.push('|');
                    }
                    write(lower, out);
                }
                out.push(')');
            }
            out.push(';');
            out.push_str(operation.code());
        }
        Name::Duplicate { name, ordinal } => {
            write(name, out);
            out.push_str(&format!(";D{:x}", ordinal.get()));
        }
    }
}

/// FreeCAD's `ElementNameComparator` (`MappedElement.cpp`, as rewritten in `140c9febc4`),
/// for names with no string-table IDs (Silico's never have them): the empty name first;
/// then character by character, a digit before any other character, and at the first
/// place both have digits, the shorter number first, then the rest as text (FreeCAD's
/// `compareNumbers`, whose loop compares from there as text); a name that's the start of
/// another first.
pub fn compare(left: &str, right: &str) -> Ordering {
    let (l, r) = (left.as_bytes(), right.as_bytes());
    if l.is_empty() || r.is_empty() {
        return l.len().cmp(&r.len());
    }
    for (i, (&lc, &rc)) in l.iter().zip(r).enumerate() {
        match (lc.is_ascii_digit(), rc.is_ascii_digit()) {
            (false, false) => {
                if lc != rc {
                    return lc.cmp(&rc);
                }
            }
            (true, true) => return compare_numbers(l, r, i),
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
        }
    }
    l.len().cmp(&r.len())
}

/// FreeCAD's `compareNumbers`: the shorter run of digits from `start` first; otherwise the
/// text from `start` on (its loop runs to the run's length as an index, then hands over to
/// `compareText`, which together compare the rest as text).
fn compare_numbers(l: &[u8], r: &[u8], start: usize) -> Ordering {
    let digits = |s: &[u8]| {
        s.get(start..).map_or(0, |rest| {
            rest.iter().take_while(|c| c.is_ascii_digit()).count()
        })
    };
    match digits(l).cmp(&digits(r)) {
        Ordering::Equal => compare_text(l, r, start),
        unequal => unequal,
    }
}

/// FreeCAD's `compareText`: from `start`, byte by byte, then the shorter first.
fn compare_text(l: &[u8], r: &[u8], start: usize) -> Ordering {
    let (l_rest, r_rest) = (l.get(start..).unwrap_or(&[]), r.get(start..).unwrap_or(&[]));
    for (lc, rc) in l_rest.iter().zip(r_rest) {
        if lc != rc {
            return lc.cmp(rc);
        }
    }
    l.len().cmp(&r.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freecads_comparator_orders_its_own_examples() {
        // tests/src/App/MappedElement.cpp's comparator cases without string-table IDs
        // (`#`), which Silico's names never have.
        let less = |a: &str, b: &str| compare(a, b) == Ordering::Less;
        assert!(!less("", ""));
        assert!(less("A", "B"));
        assert!(!less("Edge123456;", "Edge321;"));
        assert!(!less("Edge123456", "Edge321"));
        // Its map-order case, the identifiers among them, in its expected order.
        let mut names = [
            "g4v2;SKT;:H1234,F;:H5678:2,E;:G0(g1;SKT;:H9012,E);XTR;:H3456:2,F",
            "Edge999;;:L#1dae4;FUS;:H:d,E",
            "Edge123;:U;FUS;:Hce4:7,E",
        ];
        names.sort_by(|a, b| compare(a, b));
        assert_eq!(
            names,
            [
                "Edge123;:U;FUS;:Hce4:7,E",
                "Edge999;;:L#1dae4;FUS;:H:d,E",
                "g4v2;SKT;:H1234,F;:H5678:2,E;:G0(g1;SKT;:H9012,E);XTR;:H3456:2,F",
            ]
        );
    }

    #[test]
    fn digits_sort_before_letters_and_the_shorter_number_first() {
        // Worked from the comparator's rules.
        let less = |a: &str, b: &str| compare(a, b) == Ordering::Less;
        assert!(less("", "Edge1"));
        assert!(less("Edge2", "Edge10"));
        assert!(less("Edge1", "Face1"));
        assert!(less("Edge1;:U", "Edge1;:U2"));
        assert!(less("Edge1", "Edge1;:M"));
        assert!(less("Edge1a", "Edgea"));
        assert!(less("Face1;:M;FLT", "Face1;:MG;FLT"));
        assert_eq!(compare("Face12;:M", "Face12;:M"), Ordering::Equal);
    }
}
