//! A document's text in canonical form, as the core's writer writes it, with nothing
//! rebuilt: what `fmt` writes of a file whose features it can rebuild none of.

/// `text` in canonical form, as the core's writer writes it, with nothing rebuilt.
pub fn canonical(text: &str) -> String {
    silico_core::document::write(&silico_core::document::read(text.as_bytes()).unwrap()).unwrap()
}
