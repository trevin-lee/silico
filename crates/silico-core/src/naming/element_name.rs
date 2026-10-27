//! An element's name as a command's answer gives it or a client writes it back: its own type,
//! so a name is never confused with the other text a result holds (a feature's ID, a path, a
//! message), and written on the wire as the text itself, unchanged.

use std::fmt;

use super::Name;

/// An element's name in a command's answer or request, as text: its own name in Silico's form
/// as `elements` lists it (`angle`'s vertex, an `elements` page's cursor), or, for a solid
/// with no element map, as FreeCAD numbers it, `Face3` (an export's faces left out). The wire
/// writes it as the string it holds. A cursor's comes from the client and is read as a name
/// only where the listing goes on from it, which refuses one that isn't.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementName(String);

impl ElementName {
    /// The name's text, as the wire writes it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&Name> for ElementName {
    /// The name as Silico writes it.
    fn from(name: &Name) -> Self {
        ElementName(name.to_string())
    }
}

impl From<String> for ElementName {
    fn from(text: String) -> Self {
        ElementName(text)
    }
}

impl From<&str> for ElementName {
    fn from(text: &str) -> Self {
        ElementName(text.to_owned())
    }
}

/// Compared with text as a `String` is: a name equals the text it writes.
impl PartialEq<str> for ElementName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ElementName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for ElementName {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl fmt::Display for ElementName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_text_is_the_text_it_was_made_of_written_as_is() {
        let text = "aaaaaaaa.cut.Face1";
        let name = ElementName::from(text);
        assert_eq!(name.as_str(), text);
        assert_eq!(name.to_string(), text);
        assert_eq!(ElementName::from(text.to_owned()), name);
        assert_ne!(ElementName::from("Face3"), name);
        assert_eq!(name, text);
        assert_eq!(name, *text);
        assert_eq!(name, text.to_owned());
        assert_ne!(name, "Face3");
    }
}
