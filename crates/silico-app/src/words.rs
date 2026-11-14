//! Words the app's messages and labels share.

/// `names` as a list in words, joined by `conjunction` ("and" or "or"): "A", "A and B",
/// "A, B, and C", with the serial comma, as the core's and the kernel's lists have it.
pub(crate) fn listed<S: AsRef<str>>(names: &[S], conjunction: &str) -> String {
    match names {
        [] => String::new(),
        [one] => one.as_ref().to_owned(),
        [one, two] => format!("{} {conjunction} {}", one.as_ref(), two.as_ref()),
        [rest @ .., last] => {
            let rest: Vec<&str> = rest.iter().map(AsRef::as_ref).collect();
            format!("{}, {conjunction} {}", rest.join(", "), last.as_ref())
        }
    }
}

/// [`listed`], naming at most `most` of `names` and counting the rest: "A, B, and 3 others".
pub(crate) fn listed_at_most<S: AsRef<str>>(names: &[S], conjunction: &str, most: usize) -> String {
    match names.len().checked_sub(most) {
        Some(rest) if rest > 0 => {
            let mut shown: Vec<String> = names
                .iter()
                .take(most)
                .map(|name| name.as_ref().to_owned())
                .collect();
            shown.push(format!("{rest} other{}", if rest == 1 { "" } else { "s" }));
            listed(&shown, conjunction)
        }
        _ => listed(names, conjunction),
    }
}

/// Words as the ViewCube labels a view, the first a capital: `top front right` is "Top front
/// right".
pub(crate) fn capitalised(words: &str) -> String {
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A named view's words, as the ViewCube labels it and the view label names a face:
/// `top_front_right` is "Top front right", `front` "Front".
pub(crate) fn view_words(view: silico_core::views::NamedView) -> String {
    capitalised(&view.name().replace('_', " "))
}

/// A view's projection in words, as the view label, the ViewCube's menu, and Display
/// settings say it: "Perspective", or "Orthographic".
pub(crate) fn projection_words(perspective: bool) -> &'static str {
    if perspective {
        "Perspective"
    } else {
        "Orthographic"
    }
}

#[cfg(test)]
mod tests {
    use super::{listed, listed_at_most};

    #[test]
    fn a_list_in_words_has_its_conjunction_and_the_serial_comma() {
        let none: [&str; 0] = [];
        assert_eq!(listed(&none, "and"), "");
        assert_eq!(listed(&["A"], "and"), "A");
        assert_eq!(listed(&["A", "B"], "and"), "A and B");
        assert_eq!(listed(&["A", "B", "C"], "and"), "A, B, and C");
        assert_eq!(listed(&["A", "B", "C", "D"], "or"), "A, B, C, or D");
        assert_eq!(listed(&["A", "B"], "or"), "A or B");
    }

    #[test]
    fn a_list_past_its_most_names_that_many_and_counts_the_rest() {
        let seven = ["A", "B", "C", "D", "E", "F", "G"];
        assert_eq!(
            listed_at_most(&seven, "or", 5),
            "A, B, C, D, E, or 2 others"
        );
        assert_eq!(
            listed_at_most(&seven[..6], "and", 5),
            "A, B, C, D, E, and 1 other"
        );
        assert_eq!(listed_at_most(&seven[..5], "and", 5), "A, B, C, D, and E");
        assert_eq!(listed_at_most(&seven[..2], "and", 1), "A and 1 other");
    }
}
