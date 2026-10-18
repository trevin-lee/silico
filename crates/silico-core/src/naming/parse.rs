//! Silico's form of a name read back (M2.8): exactly the grammar [`Name`]'s `Display` writes,
//! nothing looser, so a name a document stores is either one a map can hold or refused,
//! saying where. The form leaves out what isn't a name's identity (OCCT's raw positions,
//! an element's last operation) and the operation a feature names its elements by, which is
//! the feature's own: `operation_of` says what each feature is, so the name read is the
//! name a map holds, found by identity. A modified element's name is its source's (N7), so
//! a name read is never [`Name::Modified`].
//!
//! Bounded as the builder bounds names: at most 256 levels deep, a `~n` a level, and at
//! most 4,096 names written out.
//!
//! ```text
//! name     = base ( "~" count )*
//! base     = feature "." [ primitive "." ] kind index        an element by its number:
//!                                                            a primitive's says its kind
//!          | feature "." kind "(" source ( ", " source )* ")"    derived (1 to 5 sources)
//!          | feature "." kind "(U" [ place ] " " name ( ", " name )* ")"
//!                                                  from the elements above (1 to 5)
//!          | feature "." kind "(L " name ( ", " name )* ")"     from those below (1 to 5)
//! source   = ( "M" | "G" ) [ place ] " " name
//! place    = count "/" count                                 rank/size, size >= 2
//! primitive = "box" | "cylinder" | "sphere" | "cone" | "torus"   a primitive feature's
//! kind     = "Vertex" | "Edge" | "Face" | "Solid"
//! count    = a number from 1, no leading zero, at most 2^32 - 1
//! ```

use std::num::NonZeroU32;
use std::sync::Arc;

use super::build::{MOST_DEPTH, MOST_SOURCES, MOST_WEIGHT};
use super::{Name, OpCode, Place, PrimitiveKind, Raw, Relation, Source};
use crate::document::{FeatureId, Made, Operation};
use crate::kernel::{ElementKind, ElementRef};

/// Why a name's text isn't a name: what's wrong, and where (the column, from 1, in
/// characters: the text is ASCII up to where it goes wrong).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameError {
    pub column: usize,
    pub why: String,
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, at column {}", self.why, self.column)
    }
}

impl Name {
    /// The name `text` writes, if it's one: every feature it names derives elements as
    /// `operation_of` says (`None` for a feature that makes its elements, a primitive,
    /// and for a feature there's no such thing as, whose names are refused).
    pub fn parse(
        text: &str,
        operation_of: &dyn Fn(&FeatureId) -> Option<Maker>,
    ) -> Result<Name, NameError> {
        let mut reader = Reader {
            text: text.as_bytes(),
            at: 0,
            weight: 0,
            height: 0,
            operation_of,
        };
        let name = reader.name(0)?;
        if reader.at != reader.text.len() {
            return Err(reader.error("more after the name"));
        }
        Ok(name)
    }
}

/// How a feature makes the elements a name says it made: by its number (a primitive, of
/// this kind), or derived from its inputs' by an operation; or not at all (a sketch, which
/// makes no solid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Maker {
    Itself(PrimitiveKind),
    By(OpCode),
    Nothing,
}

impl Maker {
    /// How a document's feature makes its elements.
    pub fn of(operation: &Operation) -> Maker {
        match operation.made() {
            Made::Primitive(primitive) => Maker::Itself(PrimitiveKind::of(&primitive)),
            Made::Boolean(op) => Maker::By(op.into()),
            Made::Fillet { .. } => Maker::By(OpCode::Fillet),
            Made::Chamfer { .. } => Maker::By(OpCode::Chamfer),
            Made::Sketch => Maker::Nothing,
        }
    }
}

struct Reader<'a> {
    text: &'a [u8],
    at: usize,
    /// The names read so far, written out.
    weight: u64,
    /// How deep the name last read is, itself and the longest chain of names inside it,
    /// each `~n` a level, as the builder counts a map's names.
    height: u32,
    operation_of: &'a dyn Fn(&FeatureId) -> Option<Maker>,
}

impl Reader<'_> {
    fn error(&self, why: &str) -> NameError {
        NameError {
            column: self.at + 1,
            why: why.to_owned(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn eat(&mut self, expected: &str) -> bool {
        let found = self
            .text
            .get(self.at..)
            .is_some_and(|rest| rest.starts_with(expected.as_bytes()));
        if found {
            self.at += expected.len();
        }
        found
    }

    fn expect(&mut self, expected: &str) -> Result<(), NameError> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("expected {expected:?}")))
        }
    }

    /// A name, `depth` names inside another.
    fn name(&mut self, depth: u32) -> Result<Name, NameError> {
        if depth >= MOST_DEPTH {
            return Err(self.too_deep());
        }
        let mut name = self.base(depth)?;
        // Each `~n` a level on top of the name's own, as the builder counts it, and the
        // whole name's depth bounded, not one level's chain (the review found chains on
        // top of chains reading as a name 4,096 deep): no deeper than a map's names, so
        // nothing that walks a name read overflows a stack.
        // (The height comes back in a field, not with the name: a larger return value on
        // every level made a 256-deep name's frames overflow a 2 MB stack.)
        let mut height = self.height;
        while self.eat("~") {
            height += 1;
            if height > MOST_DEPTH {
                return Err(self.too_deep());
            }
            self.weight += 1;
            if self.weight > MOST_WEIGHT {
                return Err(self.error(&format!(
                    "a name of more than {MOST_WEIGHT} names written out, which no map holds"
                )));
            }
            let ordinal = self.count()?;
            name = Name::Duplicate {
                name: Arc::new(name),
                ordinal,
            };
        }
        self.height = height;
        Ok(name)
    }

    /// A name deeper than [`MOST_DEPTH`], counting its `~n`s: deeper than a reference can
    /// be, though a map can hold one a level deeper, a duplicate (`~n`) of a name at the
    /// bound, which no reference may name.
    fn too_deep(&self) -> NameError {
        self.error(&format!(
            "a name nested more than {MOST_DEPTH} deep, deeper than a reference can be"
        ))
    }

    fn base(&mut self, depth: u32) -> Result<Name, NameError> {
        let start = self.at;
        // Every name written out counts, as the builder's bound counts them.
        self.weight += 1;
        if self.weight > MOST_WEIGHT {
            return Err(self.error(&format!(
                "a name of more than {MOST_WEIGHT} names written out, which no map holds"
            )));
        }
        let feature = self.feature()?;
        let Some(maker) = (self.operation_of)(&feature) else {
            self.at = start;
            return Err(self.error(&format!("no feature {feature} before this one")));
        };
        if maker == Maker::Nothing {
            self.at = start;
            return Err(self.error(&format!(
                "{feature} makes no solid (a sketch): it has no faces, edges, or vertices"
            )));
        }
        self.expect(".")?;
        // A primitive's element says the kind of primitive (`box.`), and only a primitive's.
        let primitive = self.primitive_kind();
        match (maker, primitive) {
            (Maker::Itself(made), None) => {
                let word = made.word();
                return Err(self.error(&format!(
                    "{feature} is a primitive: its elements say which (`{feature}.{word}.Edge1`)"
                )));
            }
            (Maker::By(_), Some(_)) => {
                self.at = start;
                return Err(self.error(&format!(
                    "{feature} isn't a primitive: its elements don't say a primitive's kind"
                )));
            }
            _ => {}
        }
        let kind = self.kind()?;
        if !self.eat("(") {
            let index = self.count()?;
            self.height = 1;
            return Ok(Name::Element {
                feature,
                primitive,
                element: ElementRef {
                    kind,
                    index: index.get(),
                },
            });
        }
        let operation = match maker {
            Maker::By(operation) => operation,
            Maker::Itself(_) | Maker::Nothing => {
                self.at = start;
                return Err(self.error(&format!(
                    "{feature} makes its elements itself: it names them by their numbers"
                )));
            }
        };
        let mut tallest = 0;
        let name = if self.eat("U") {
            let place = self.place()?;
            self.expect(" ")?;
            let upper = Arc::new(self.name(depth + 1)?);
            tallest = tallest.max(self.height);
            let mut more = Vec::new();
            while self.eat(", ") {
                if more.len() + 1 >= MOST_SOURCES {
                    return Err(self.error(&format!("more than {MOST_SOURCES} elements above")));
                }
                more.push(Arc::new(self.name(depth + 1)?));
                tallest = tallest.max(self.height);
            }
            Name::Upper {
                feature,
                operation,
                kind,
                upper,
                more,
                place,
                // Not a name's identity: FreeCAD's count, which the text doesn't hold.
                nth: Raw(NonZeroU32::MIN),
            }
        } else if self.eat("L ") {
            let first = Arc::new(self.name(depth + 1)?);
            tallest = tallest.max(self.height);
            let mut more = Vec::new();
            while self.eat(", ") {
                if more.len() + 1 >= MOST_SOURCES {
                    return Err(self.error(&format!("more than {MOST_SOURCES} elements below")));
                }
                more.push(Arc::new(self.name(depth + 1)?));
                tallest = tallest.max(self.height);
            }
            Name::Lower {
                feature,
                operation,
                kind,
                first,
                more,
            }
        } else {
            let first = self.source(depth)?;
            tallest = tallest.max(self.height);
            let mut more = Vec::new();
            while self.eat(", ") {
                if more.len() + 1 >= MOST_SOURCES {
                    return Err(self.error(&format!("more than {MOST_SOURCES} sources")));
                }
                more.push(self.source(depth)?);
                tallest = tallest.max(self.height);
            }
            Name::Derived {
                feature,
                operation,
                kind,
                first,
                more,
            }
        };
        self.expect(")")?;
        // Every level counted, not only a `~n`'s (the review found 255 levels on top of a
        // chain read as a name 511 deep).
        self.height = tallest + 1;
        if self.height > MOST_DEPTH {
            return Err(self.too_deep());
        }
        Ok(name)
    }

    /// A source, `depth` names inside another; [`Reader::height`] its name's.
    fn source(&mut self, depth: u32) -> Result<Source, NameError> {
        let relation = if self.eat("M") {
            Relation::Modified
        } else if self.eat("G") {
            Relation::Generated
        } else {
            return Err(self.error("expected a source: `M` or `G`"));
        };
        let place = self.place()?;
        self.expect(" ")?;
        let name = self.name(depth + 1)?;
        Ok(Source {
            relation,
            place,
            // Not a name's identity: what OCCT's list said, which the text doesn't hold.
            position: Raw(NonZeroU32::MIN),
            name: Arc::new(name),
        })
    }

    /// A place, `rank/size`, if the text says one here.
    fn place(&mut self) -> Result<Option<Place>, NameError> {
        if !self.peek().is_some_and(|b| b.is_ascii_digit()) {
            return Ok(None);
        }
        let rank = self.count()?;
        self.expect("/")?;
        let size = self.count()?;
        if size.get() < 2 {
            return Err(self.error("a place is among 2 or more"));
        }
        if rank > size {
            return Err(self.error("a place's rank is at most its group's size"));
        }
        Ok(Some(Place { rank, size }))
    }

    fn feature(&mut self) -> Result<FeatureId, NameError> {
        let end = (self.at + crate::document::ID_LENGTH).min(self.text.len());
        let id = self
            .text
            .get(self.at..end)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(FeatureId::new)
            .ok_or_else(|| self.error("expected a feature's ID"))?;
        self.at = end;
        Ok(id)
    }

    /// A primitive's kind and its `.`, if the text says one here.
    fn primitive_kind(&mut self) -> Option<PrimitiveKind> {
        PrimitiveKind::ALL
            .into_iter()
            .find(|kind| self.eat(&format!("{}.", kind.word())))
    }

    fn kind(&mut self) -> Result<ElementKind, NameError> {
        for (word, kind) in [
            ("Vertex", ElementKind::Vertex),
            ("Edge", ElementKind::Edge),
            ("Face", ElementKind::Face),
            ("Solid", ElementKind::Solid),
        ] {
            if self.eat(word) {
                return Ok(kind);
            }
        }
        Err(self.error("expected `Vertex`, `Edge`, `Face`, or `Solid`"))
    }

    /// A number from 1, written without a leading zero, that fits in 32 bits.
    fn count(&mut self) -> Result<NonZeroU32, NameError> {
        let start = self.at;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.at += 1;
        }
        let digits = self.text.get(start..self.at).unwrap_or_default();
        let at_start = |reader: &mut Self, why: &str| {
            reader.at = start;
            reader.error(why)
        };
        match digits {
            [] => Err(at_start(self, "expected a number")),
            [b'0', ..] => Err(at_start(self, "a number from 1, without a leading zero")),
            _ => std::str::from_utf8(digits)
                .ok()
                .and_then(|d| d.parse::<NonZeroU32>().ok())
                .ok_or_else(|| at_start(self, "a number past 4294967295")),
        }
    }
}
