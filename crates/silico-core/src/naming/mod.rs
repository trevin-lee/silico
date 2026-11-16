//! Topological naming (M2.7, docs/ports/naming.md, docs/design/m2.7-naming.md): every face,
//! edge, and vertex of a feature's solid gets a name that says how it came about, derived
//! from the kernel's history as FreeCAD's `makeShapeWithElementMap` derives it, so a
//! reference to an element can follow it through upstream edits.
//!
//! A [`Name`] is structured, not a string; it's written in Silico's compact form
//! (`m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)`), and [`freecad`]
//! renders FreeCAD's form for comparison with FreeCAD's names. An [`ElementMap`] holds a solid's names, built
//! by [`ElementMap::primitive`], [`ElementMap::operation`], or kept through a move.

mod build;
pub mod freecad;
mod parse;

#[cfg(test)]
pub(crate) use parse::makers_read;
pub use parse::{Maker, NameError};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;

use crate::document::{ElementId, FeatureId};
use crate::kernel::{BooleanOp, ElementCounts, ElementKind, ElementRef};

/// The naming scheme: what names a map gives its elements (the grammar, the map's builder,
/// a primitive's numbers). Its number changes with any change to them, and
/// tests/naming_scheme.rs, which pins with it the names a set of documents and histories
/// get, fails until it does: a fillet's step is keyed by its base's naming key, which holds
/// this, and its result is kept on disk, so a result another scheme made, from the edges
/// its names found then, is never taken for this one's (the review found a warm cache
/// re-pointing a fillet after the cone's numbers changed).
pub const NAMING_SCHEME: &[u8] = b"silico naming 6\n";

/// The naming scheme's number, the one [`NAMING_SCHEME`] holds: what a document's header
/// records (`naming`, M4.0) as the scheme its names were written under, and what every
/// save writes. Bumped with it.
pub const NAMING_SCHEME_NUMBER: u32 = 6;

/// The schemes whose names this Silico reads, as they are: its own, and 5, of which it's a
/// strict superset (N10, docs/design/m2.7-naming.md: scheme 6 adds the copy form and three
/// type words, and every scheme-5 name is a scheme-6 name, written alike, naming the same
/// element). A document of either is read with no migration; any other is refused, naming
/// its `naming`.
pub const NAMING_SCHEMES_READ: [u32; 2] = [5, NAMING_SCHEME_NUMBER];

/// The scheme a document that records none is read as: 5, the scheme Silico named by when
/// the header gained `naming` (M4.0). Only documents saved before then lack the field, and
/// their names read safely as scheme 5's: those saved under scheme 4 are the same names,
/// and scheme 2's are refused or find nothing, never re-pointed (docs/ports/naming.md).
/// This stays 5 when [`NAMING_SCHEME_NUMBER`] moves on, and such a document is then read as
/// scheme 5's (as [`NAMING_SCHEMES_READ`] reads 5's names), never as the new scheme's.
pub const UNRECORDED_NAMING_SCHEME: u32 = 5;

/// The operation that derived an element: written in Silico's form as the document's type
/// (`cut`), as a primitive's element writes its type (N8, 2026-10-18), so a boolean whose
/// type is changed under its ID names none of its old elements; and FreeCAD's op code
/// (`CUT`), part of FreeCAD's names and of the order in which FreeCAD picks a name's
/// sources. A pad, pocket, revolve, or groove names every element of its solid as its own
/// (docs/design/m3.6-sketch-features.md, S4), and a sketch its elements by their IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OpCode {
    Fuse,
    Cut,
    Common,
    Fillet,
    Chamfer,
    Pad,
    Pocket,
    Revolve,
    Groove,
    /// A sketch's faces, made of its profile: named by its elements' IDs, its only region
    /// `Face1` (S4).
    Sketch,
    /// A pattern's or mirror's steps, each fusing copies of a tool or of its base solid into
    /// its base, or cutting them from it (N10, scheme 6): their copies' elements are named
    /// [`Name::Copy`], and every input is bitten at every step.
    LinearPattern,
    PolarPattern,
    Mirror,
}

/// Which of a feature's steps names its elements (design S4): the sweep of a pad's,
/// pocket's, revolve's, or groove's profile, or a feature's last step, its only one for any
/// other feature, and a sweep feature's fuse or cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Sweep,
    Last,
}

/// What of an input an operation keeps, as its names write it (N9, and M3.6's third value):
/// what lies outside its other operand (bitten: kept names collapse to their source's), what
/// lies inside it (a piece, the feature's own, `I`), or neither side (a sweep's profile,
/// which is in no solid: the feature's own, one `M`, never collapsed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Outside,
    Inside,
    Neither,
}

impl From<crate::document::SweepFeature> for OpCode {
    fn from(kind: crate::document::SweepFeature) -> Self {
        match kind {
            crate::document::SweepFeature::Pad => OpCode::Pad,
            crate::document::SweepFeature::Pocket => OpCode::Pocket,
            crate::document::SweepFeature::Revolve => OpCode::Revolve,
            crate::document::SweepFeature::Groove => OpCode::Groove,
        }
    }
}

impl From<BooleanOp> for OpCode {
    fn from(op: BooleanOp) -> Self {
        match op {
            BooleanOp::Fuse => OpCode::Fuse,
            BooleanOp::Cut => OpCode::Cut,
            BooleanOp::Common => OpCode::Common,
        }
    }
}

impl OpCode {
    pub const ALL: [OpCode; 13] = [
        OpCode::Fuse,
        OpCode::Cut,
        OpCode::Common,
        OpCode::Fillet,
        OpCode::Chamfer,
        OpCode::Pad,
        OpCode::Pocket,
        OpCode::Revolve,
        OpCode::Groove,
        OpCode::Sketch,
        OpCode::LinearPattern,
        OpCode::PolarPattern,
        OpCode::Mirror,
    ];

    /// The word a name writes, which is the type the document writes.
    pub fn word(self) -> &'static str {
        match self {
            OpCode::Fuse => "fuse",
            OpCode::Cut => "cut",
            OpCode::Common => "common",
            OpCode::Fillet => "fillet",
            OpCode::Chamfer => "chamfer",
            OpCode::Pad => "pad",
            OpCode::Pocket => "pocket",
            OpCode::Revolve => "revolve",
            OpCode::Groove => "groove",
            OpCode::Sketch => "sketch",
            OpCode::LinearPattern => "linear_pattern",
            OpCode::PolarPattern => "polar_pattern",
            OpCode::Mirror => "mirror",
        }
    }

    /// The most directions a copy's instance may step along, for an operation that copies
    /// (a linear pattern's two, a polar pattern's or a mirror's one: a mirror is a pattern of
    /// two instances, N10); none for one that copies nothing, whose names never hold a copy.
    pub fn directions(self) -> Option<usize> {
        match self {
            OpCode::LinearPattern => Some(2),
            OpCode::PolarPattern | OpCode::Mirror => Some(1),
            OpCode::Fuse
            | OpCode::Cut
            | OpCode::Common
            | OpCode::Fillet
            | OpCode::Chamfer
            | OpCode::Pad
            | OpCode::Pocket
            | OpCode::Revolve
            | OpCode::Groove
            | OpCode::Sketch => None,
        }
    }

    /// Whether it's a sweep feature's (a pad's, pocket's, revolve's, or groove's), whose
    /// sweep step is the only tool a pattern copies (N10: a copy marked `C` is of one).
    pub fn sweeps(self) -> bool {
        matches!(
            self,
            OpCode::Pad | OpCode::Pocket | OpCode::Revolve | OpCode::Groove
        )
    }

    /// Whether the operation keeps what of input `input` lies outside its other operand
    /// (its element, less a bite: a cut's base, a fuse's operands, a fillet's or chamfer's
    /// input), rather than what lies inside it (a piece: a cut's tool, a common's operands).
    /// The one place a name's side is decided, which its letter writes (N9): what's bitten
    /// keeps its names, kept whole or modified one from one (N7), and is written `M` where
    /// it's derived (a split's `M1/2`); every piece, kept whole or trimmed, is the feature's
    /// own, written `I` (`c.cut.Face(I y.cylinder.Face1)`), so a cut made a common, or its
    /// base and tool swapped, takes no reference to a piece for another. By the booleans'
    /// definitions, a fixed table. Of a feature's last step: [`OpCode::side`] has every step.
    pub fn bites(self, input: usize) -> bool {
        self.side(Stage::Last, input) == Side::Outside
    }

    /// What step `stage` of the operation keeps of input `input` (N9; M3.6's S4, the table
    /// in docs/design/m3.6-sketch-features.md): per type word, step, and input, a fixed
    /// table. A cut keeps its base's outside and its tool's inside, a common both insides,
    /// a fuse both outsides, a fillet or chamfer its input's outside. A pad's or revolve's
    /// fuse, and a pocket's or groove's cut, bite both their base and their own sweep: its
    /// elements already say the feature and its type, and nothing else operates on it. A
    /// sweep keeps neither side of its profile, which is in no solid. A pattern's or mirror's
    /// steps bite every input, base and copies, as a sweep feature's boolean bites its own
    /// sweep (N10): a copy's side is fixed by its letter and its tool's type word, never by
    /// an `I`. A sketch's faces are named by their IDs, never through this; outside, were
    /// they asked.
    pub fn side(self, stage: Stage, input: usize) -> Side {
        match (self, stage) {
            (op, Stage::Sweep) if op.sweeps() => Side::Neither,
            (OpCode::Cut, _) if input == 0 => Side::Outside,
            (OpCode::Cut | OpCode::Common, _) => Side::Inside,
            (
                OpCode::Fuse
                | OpCode::Fillet
                | OpCode::Chamfer
                | OpCode::Pad
                | OpCode::Pocket
                | OpCode::Revolve
                | OpCode::Groove
                | OpCode::Sketch
                | OpCode::LinearPattern
                | OpCode::PolarPattern
                | OpCode::Mirror,
                _,
            ) => Side::Outside,
        }
    }

    /// FreeCAD's op code (`TopoShapeOpCode.h`). A sweep feature writes its sweep's
    /// (`XTR`, `RVL`) for every element it names, its fuse's or cut's too, where FreeCAD
    /// writes `FUS` or `CUT` for those: one word names every step of the feature (S4), so
    /// FreeCAD's form, held for comparison and FreeCAD's orders, says the feature's.
    ///
    /// A pattern's or mirror's is `FUS`, though FreeCAD 1.1's `Transformed` cuts a
    /// subtractive tool's copies (`FeatureTransformed.cpp:392`, `:399` at the 1.1 branch
    /// point, `af2d42a4aa`), so its form of a cut
    /// step's elements writes `CUT`: no step names a copy yet, and M6.3 settles the cut
    /// step's form, pinned by a test, before any does (docs/design/m2.7-naming.md, N10).
    pub fn code(self) -> &'static str {
        match self {
            OpCode::Fuse => "FUS",
            OpCode::Cut => "CUT",
            OpCode::Common => "CMN",
            OpCode::Fillet => "FLT",
            OpCode::Chamfer => "CHF",
            OpCode::Pad | OpCode::Pocket => "XTR",
            OpCode::Revolve | OpCode::Groove => "RVL",
            OpCode::Sketch => "SKT",
            OpCode::LinearPattern | OpCode::PolarPattern | OpCode::Mirror => "FUS",
        }
    }
}

/// What a copy's element is a copy of (N10): an element of a feature's tool, marked `C`
/// (`t.linear_pattern.Face(C2 q.pocket.Face(…))`), the tool always a sweep step, so its
/// side is its type word's (a pad's or revolve's fused, a pocket's or groove's cut); or an
/// element of the base solid, marked `S` (`m.mirror.Face(S1 p.pad.Face(…))`), always fused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Copied {
    Tool,
    Solid,
}

impl Copied {
    /// The letter a copy's name writes.
    pub fn letter(self) -> char {
        match self {
            Copied::Tool => 'C',
            Copied::Solid => 'S',
        }
    }
}

/// Where a copy was placed (N10): its steps from the original along each of its feature's
/// directions, direction 1 first, trailing zeros left off, so its last offset is 1 or more
/// and the original, every offset 0, is no instance (its elements are unmarked). One
/// spelling an instance: (2) and (2, 0) are one, written `2`, so a second direction added
/// renames no copy.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instance(Box<[u32]>);

impl Instance {
    /// The instance `offsets` steps from the original along each direction, trailing zeros
    /// left off; none for the original itself (every offset 0, or none).
    pub fn new(offsets: &[u32]) -> Option<Instance> {
        let last = offsets.iter().rposition(|&offset| offset > 0)?;
        offsets.get(..=last).map(|kept| Instance(kept.into()))
    }

    /// Its offsets, direction 1's first, the last 1 or more.
    pub fn offsets(&self) -> &[u32] {
        &self.0
    }
}

impl fmt::Display for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, offset) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "{offset}")?;
        }
        Ok(())
    }
}

/// The kind of primitive that made an element, as its name says it (`box`, ...): the type
/// the document writes for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrimitiveKind {
    Box,
    Cylinder,
    Sphere,
    Cone,
    Torus,
}

impl PrimitiveKind {
    pub const ALL: [PrimitiveKind; 5] = [
        PrimitiveKind::Box,
        PrimitiveKind::Cylinder,
        PrimitiveKind::Sphere,
        PrimitiveKind::Cone,
        PrimitiveKind::Torus,
    ];

    pub fn of(primitive: &crate::kernel::Primitive) -> PrimitiveKind {
        match primitive {
            crate::kernel::Primitive::Box { .. } => PrimitiveKind::Box,
            crate::kernel::Primitive::Cylinder { .. } => PrimitiveKind::Cylinder,
            crate::kernel::Primitive::Sphere { .. } => PrimitiveKind::Sphere,
            crate::kernel::Primitive::Cone { .. } => PrimitiveKind::Cone,
            crate::kernel::Primitive::Torus { .. } => PrimitiveKind::Torus,
        }
    }

    /// The word a name writes, which is the type the document writes.
    pub fn word(self) -> &'static str {
        match self {
            PrimitiveKind::Box => "box",
            PrimitiveKind::Cylinder => "cylinder",
            PrimitiveKind::Sphere => "sphere",
            PrimitiveKind::Cone => "cone",
            PrimitiveKind::Torus => "torus",
        }
    }
}

/// The type of the feature that made an element, as its name writes it after the feature
/// (`box`, `cut`): the type the document writes. Read from a name's text, never from what
/// the feature is now, so a feature whose type changed under its ID names none of its old
/// elements (a primitive's, Trevin's, 2026-10-17; an operation's, N8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TypeWord {
    Primitive(PrimitiveKind),
    Operation(OpCode),
}

impl TypeWord {
    /// Every type word, primitives first.
    pub fn all() -> impl Iterator<Item = TypeWord> {
        PrimitiveKind::ALL
            .into_iter()
            .map(TypeWord::Primitive)
            .chain(OpCode::ALL.into_iter().map(TypeWord::Operation))
    }

    /// The word.
    pub fn word(self) -> &'static str {
        match self {
            TypeWord::Primitive(kind) => kind.word(),
            TypeWord::Operation(op) => op.word(),
        }
    }
}

impl fmt::Display for TypeWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// How a source element became the element named: OCCT's `Modified` or `Generated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    Modified,
    Generated,
    /// What of the source lies inside the other operand (a cut's tool, a common's
    /// operands), kept whole or trimmed: the feature's own piece of it, `I`.
    Inside,
}

/// One of the elements an element came from, and how; and, if other elements came about
/// alike, which of them this one is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Source {
    pub relation: Relation,
    /// Where it is among the elements that came about alike (Trevin's decision, 2026-10-16):
    /// its rank among them in this source's OCCT list, and how many they are (`1/2`).
    /// Written only for such a group, so a name doesn't change when OCCT reorders its lists
    /// for reasons of its own (it reordered a hole's rims once the plate had another hole).
    pub place: Option<Place>,
    /// Where OCCT's list for the source had it, from 1, counting entries the result doesn't
    /// have: FreeCAD writes it (`;:G2`), Silico only where `place` is. None for an element
    /// kept whole inside the other operand, which no list holds. Not part of the name.
    pub position: Raw<Option<NonZeroU32>>,
    pub name: Arc<Name>,
}

/// A member's place in a group of elements that came about alike: its `rank` among them in
/// one source's OCCT list, of `size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Place {
    pub rank: NonZeroU32,
    pub size: NonZeroU32,
}

/// OCCT's position for a source, for FreeCAD's form of a name and nothing else: two names are
/// the same name whatever positions OCCT gave, as their Silico form says.
#[derive(Debug, Clone, Copy)]
pub struct Raw<T = NonZeroU32>(pub T);

impl<T> PartialEq for Raw<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<T> Eq for Raw<T> {}

impl<T> PartialOrd for Raw<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Raw<T> {
    fn cmp(&self, _: &Self) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }
}

impl<T> std::hash::Hash for Raw<T> {
    fn hash<H: std::hash::Hasher>(&self, _: &mut H) {}
}

/// An element's name: how it came about, down to the elements features made of nothing.
/// Every count in it is from 1, and every list has a first, so no two names are written
/// alike. Two names are equal when they're written alike: a modified element that's its
/// source's only result is equal to its source's name (`Modified`).
#[derive(Debug, Clone)]
pub enum Name {
    /// The element numbered so in what `feature` made, with the type `feature` is: a
    /// primitive's own `Face3` (`k7f2qx3m.box.Face3`: Trevin's, 2026-10-17, so a primitive
    /// whose type is changed under its ID names none of its old elements). Also, as
    /// FreeCAD's unmapped names are, the last resort for an element nothing else names
    /// (`m3tq8v0z.cut.Edge5`), unstable.
    Element {
        feature: FeatureId,
        made: TypeWord,
        element: ElementRef,
    },
    /// An element `feature`'s operation made from its inputs' elements, as OCCT reported
    /// it: the first source (by its kind, then input, then name, as FreeCAD orders them),
    /// then up to four more.
    Derived {
        feature: FeatureId,
        operation: OpCode,
        kind: ElementKind,
        first: Source,
        more: Vec<Source>,
    },
    /// An element modified from one element, `name`, as its only result (OCCT's first,
    /// none other modified or generated of its kind), by one feature or by a run of them,
    /// each keeping what of it lies outside its other operand ([`OpCode::bites`]: Trevin's
    /// decision, 2026-10-16, as N8 and N9 narrowed it; what lies inside is written `I`): it is its source's own name, written and compared
    /// as that name, so a plate's top face is `pppppppp.box.Face6` after any number of
    /// holes, and a hole's rim named from it doesn't change when an earlier hole is added,
    /// removed, or moved. Unique: such a step makes one element of one, and an element and what it
    /// became are never both in a solid (were they, the second would be `~1`, unstable).
    /// The operation is kept for FreeCAD's form (`Face6;:M;CUT`) and nothing else.
    Modified { name: Arc<Name>, last: LastOp },
    /// An element nothing reported, named by every named element above it (an edge by the
    /// faces it bounds, a vertex by the edges it ends), `upper` first by FreeCAD's order,
    /// then up to four `more` (Trevin's, 2026-10-17); and, where elements above are alike
    /// for two or more of them, its `place` among those, by FreeCAD's count in `upper`.
    /// `nth` is FreeCAD's own: the element's count among `upper`'s unnamed elements of its
    /// kind (`;:U2`), kept for FreeCAD's form, not the name's identity, since it moves when
    /// an edit changes what else `upper` bounds (FreeCAD 1.1.3 re-points a reference there).
    Upper {
        feature: FeatureId,
        operation: OpCode,
        kind: ElementKind,
        upper: Arc<Name>,
        more: Vec<Arc<Name>>,
        place: Option<Place>,
        nth: Raw,
    },
    /// An element named from the elements below it (an edge from its vertices, a face from
    /// its outer wire's edges): the first in FreeCAD's order, then up to four more
    /// (FreeCAD's `;:L`).
    Lower {
        feature: FeatureId,
        operation: OpCode,
        kind: ElementKind,
        first: Arc<Name>,
        more: Vec<Arc<Name>>,
    },
    /// The `ordinal`th of the elements that came about as `name` did, after the first:
    /// rare, and never stable, so a reference to one is refused (N3).
    Duplicate {
        name: Arc<Name>,
        ordinal: NonZeroU32,
    },
    /// A sketch's curve's edge or point's vertex in the faces made of its profile, by the
    /// element's ID, which never moves when the geometry is dragged or the sketch's lists
    /// reordered (`s0000001.sketch.Edge.6t7v8w9x`; design S4). Never a whole reference: a
    /// sketch's elements are in no solid, and appear only as sources in what a pad, pocket,
    /// revolve, or groove made of them.
    Sketch {
        feature: FeatureId,
        kind: ElementKind,
        id: ElementId,
    },
    /// A copy of the element `of` names, placed at `instance` by `feature`'s `operation`, a
    /// pattern or mirror (N10, scheme 6): `of` an element of a feature's tool (`C`) or of
    /// the base solid (`S`), as `copied` says. The copy's element is the tool's, wrapped once,
    /// so its parts are `of`'s: a copy of an unstable name is unstable, and the depth bound
    /// counts a level for it. The original is never a copy: its elements are the base's.
    Copy {
        feature: FeatureId,
        operation: OpCode,
        kind: ElementKind,
        copied: Copied,
        instance: Instance,
        of: Arc<Name>,
    },
}

/// The operation that last modified a [`Name::Modified`] element, for its FreeCAD form
/// (`Face6;:M;CUT`) and nothing else: two such names are the same name whatever modified
/// them, as their Silico form says.
#[derive(Debug, Clone, Copy)]
pub struct LastOp(pub OpCode);

impl PartialEq for LastOp {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for LastOp {}

impl PartialOrd for LastOp {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LastOp {
    fn cmp(&self, _: &Self) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }
}

impl std::hash::Hash for LastOp {
    fn hash<H: std::hash::Hasher>(&self, _: &mut H) {}
}

impl Name {
    /// The kind of element it names.
    pub fn kind(&self) -> ElementKind {
        match self {
            Name::Element { element, .. } => element.kind,
            Name::Derived { kind, .. }
            | Name::Upper { kind, .. }
            | Name::Lower { kind, .. }
            | Name::Sketch { kind, .. }
            | Name::Copy { kind, .. } => *kind,
            Name::Duplicate { name, .. } | Name::Modified { name, .. } => name.kind(),
        }
    }

    /// Every feature named in it, its makers, in the order it writes them, each once: a
    /// feature whose elements a reference to it stores, so an edit that would leave one of
    /// them missing or after the feature storing it is refused (the M5.0 design's chain). A
    /// modified element's are its source's, as it's written as that name.
    pub fn features(&self) -> Vec<&FeatureId> {
        let mut found = Vec::new();
        self.collect_features(&mut found, &mut BTreeSet::new());
        found
    }

    /// Into `found`, in written order, each feature `seen` doesn't hold yet: a search of a
    /// sorted set, not of the list, however many a name writes out.
    fn collect_features<'a>(
        &'a self,
        found: &mut Vec<&'a FeatureId>,
        seen: &mut BTreeSet<&'a FeatureId>,
    ) {
        fn add<'a>(
            id: &'a FeatureId,
            found: &mut Vec<&'a FeatureId>,
            seen: &mut BTreeSet<&'a FeatureId>,
        ) {
            if seen.insert(id) {
                found.push(id);
            }
        }
        match self {
            Name::Element { feature, .. } | Name::Sketch { feature, .. } => {
                add(feature, found, seen)
            }
            Name::Derived {
                feature,
                first,
                more,
                ..
            } => {
                add(feature, found, seen);
                for source in std::iter::once(first).chain(more) {
                    source.name.collect_features(found, seen);
                }
            }
            Name::Upper {
                feature,
                upper,
                more,
                ..
            } => {
                add(feature, found, seen);
                for name in std::iter::once(upper).chain(more) {
                    name.collect_features(found, seen);
                }
            }
            Name::Lower {
                feature,
                first,
                more,
                ..
            } => {
                add(feature, found, seen);
                for name in std::iter::once(first).chain(more) {
                    name.collect_features(found, seen);
                }
            }
            Name::Copy { feature, of, .. } => {
                add(feature, found, seen);
                of.collect_features(found, seen);
            }
            Name::Duplicate { name, .. } | Name::Modified { name, .. } => {
                name.collect_features(found, seen);
            }
        }
    }

    /// The feature named in it, first: for a modified element, its source's (it's written
    /// as that name), not the feature that modified it; for a duplicate, its name's.
    pub fn feature(&self) -> &FeatureId {
        match self {
            Name::Element { feature, .. }
            | Name::Derived { feature, .. }
            | Name::Upper { feature, .. }
            | Name::Lower { feature, .. }
            | Name::Sketch { feature, .. }
            | Name::Copy { feature, .. } => feature,
            Name::Duplicate { name, .. } | Name::Modified { name, .. } => name.feature(),
        }
    }

    /// The type its text writes after its first feature (N8): a name by number its
    /// feature's, a derived one or a copy its operation's, a sketch's `sketch`; a duplicate's
    /// or a modified element's, its name's.
    pub fn type_word(&self) -> TypeWord {
        match self {
            Name::Element { made, .. } => *made,
            Name::Derived { operation, .. }
            | Name::Upper { operation, .. }
            | Name::Lower { operation, .. }
            | Name::Copy { operation, .. } => TypeWord::Operation(*operation),
            Name::Sketch { .. } => TypeWord::Operation(OpCode::Sketch),
            Name::Duplicate { name, .. } | Name::Modified { name, .. } => name.type_word(),
        }
    }

    /// The name it's equal to: a modified element that's its source's only result is its
    /// source's name.
    fn identity(&self) -> &Name {
        let mut name = self;
        while let Name::Modified { name: source, .. } = name {
            name = source;
        }
        name
    }

    /// The names it's made of, one level down.
    pub(crate) fn parts(&self) -> Vec<&Arc<Name>> {
        match self {
            Name::Element { .. } | Name::Sketch { .. } => Vec::new(),
            Name::Derived { first, more, .. } => std::iter::once(first)
                .chain(more)
                .map(|source| &source.name)
                .collect(),
            Name::Upper { upper, more, .. } => std::iter::once(upper).chain(more).collect(),
            Name::Lower { first, more, .. } => std::iter::once(first).chain(more).collect(),
            Name::Duplicate { name, .. }
            | Name::Modified { name, .. }
            | Name::Copy { of: name, .. } => vec![name],
        }
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let (a, b) = (self.identity(), other.identity());
        // Names share their parts: one part is equal to itself without a walk.
        if std::ptr::eq(a, b) {
            return std::cmp::Ordering::Equal;
        }
        let variant = |n: &Name| match n {
            Name::Element { .. } => 0u8,
            Name::Derived { .. } => 1,
            Name::Upper { .. } => 2,
            Name::Lower { .. } => 3,
            Name::Duplicate { .. } => 4,
            Name::Modified { .. } => 5,
            Name::Sketch { .. } => 6,
            Name::Copy { .. } => 7,
        };
        variant(a).cmp(&variant(b)).then_with(|| match (a, b) {
            (
                Name::Element {
                    feature,
                    made,
                    element,
                },
                Name::Element {
                    feature: f,
                    made: m,
                    element: e,
                },
            ) => feature.cmp(f).then(made.cmp(m)).then(element.cmp(e)),
            (
                Name::Derived {
                    feature,
                    operation,
                    kind,
                    first,
                    more,
                },
                Name::Derived {
                    feature: f,
                    operation: o,
                    kind: k,
                    first: fi,
                    more: m,
                },
            ) => feature
                .cmp(f)
                .then(operation.cmp(o))
                .then(kind.cmp(k))
                .then_with(|| first.cmp(fi))
                .then_with(|| more.cmp(m)),
            (
                Name::Upper {
                    feature,
                    operation,
                    kind,
                    upper,
                    more,
                    place,
                    ..
                },
                Name::Upper {
                    feature: f,
                    operation: o,
                    kind: k,
                    upper: u,
                    more: m,
                    place: p,
                    ..
                },
            ) => feature
                .cmp(f)
                .then(operation.cmp(o))
                .then(kind.cmp(k))
                .then_with(|| upper.cmp(u))
                .then_with(|| more.cmp(m))
                .then(place.cmp(p)),
            (
                Name::Lower {
                    feature,
                    operation,
                    kind,
                    first,
                    more,
                },
                Name::Lower {
                    feature: f,
                    operation: o,
                    kind: k,
                    first: fi,
                    more: m,
                },
            ) => feature
                .cmp(f)
                .then(operation.cmp(o))
                .then(kind.cmp(k))
                .then_with(|| first.cmp(fi))
                .then_with(|| more.cmp(m)),
            (
                Name::Duplicate { name, ordinal },
                Name::Duplicate {
                    name: n,
                    ordinal: o,
                },
            ) => name.cmp(n).then(ordinal.cmp(o)),
            (
                Name::Sketch { feature, kind, id },
                Name::Sketch {
                    feature: f,
                    kind: k,
                    id: i,
                },
            ) => feature.cmp(f).then(kind.cmp(k)).then_with(|| id.cmp(i)),
            (
                Name::Copy {
                    feature,
                    operation,
                    kind,
                    copied,
                    instance,
                    of,
                },
                Name::Copy {
                    feature: f,
                    operation: o,
                    kind: k,
                    copied: c,
                    instance: i,
                    of: n,
                },
            ) => feature
                .cmp(f)
                .then(operation.cmp(o))
                .then(kind.cmp(k))
                .then(copied.cmp(c))
                .then_with(|| instance.cmp(i))
                .then_with(|| of.cmp(n)),
            // Different variants are ordered above; a `Modified` is never an identity.
            _ => std::cmp::Ordering::Equal,
        })
    }
}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for Name {}

impl std::hash::Hash for Name {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // As equality: by the name it's equal to, its parts by theirs.
        let name = self.identity();
        std::mem::discriminant(name).hash(state);
        match name {
            Name::Element {
                feature,
                made,
                element,
            } => {
                feature.hash(state);
                made.hash(state);
                element.hash(state);
            }
            Name::Derived {
                feature,
                operation,
                kind,
                first,
                more,
            } => {
                feature.hash(state);
                operation.hash(state);
                kind.hash(state);
                first.hash(state);
                more.hash(state);
            }
            Name::Upper {
                feature,
                operation,
                kind,
                upper,
                more,
                place,
                ..
            } => {
                feature.hash(state);
                operation.hash(state);
                kind.hash(state);
                upper.hash(state);
                more.hash(state);
                place.hash(state);
            }
            Name::Lower {
                feature,
                operation,
                kind,
                first,
                more,
            } => {
                feature.hash(state);
                operation.hash(state);
                kind.hash(state);
                first.hash(state);
                more.hash(state);
            }
            Name::Duplicate { name, ordinal } => {
                name.hash(state);
                ordinal.hash(state);
            }
            Name::Sketch { feature, kind, id } => {
                feature.hash(state);
                kind.hash(state);
                id.hash(state);
            }
            Name::Copy {
                feature,
                operation,
                kind,
                copied,
                instance,
                of,
            } => {
                feature.hash(state);
                operation.hash(state);
                kind.hash(state);
                copied.hash(state);
                instance.hash(state);
                of.hash(state);
            }
            Name::Modified { .. } => {}
        }
    }
}

fn kind_word(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Vertex => "Vertex",
        ElementKind::Edge => "Edge",
        ElementKind::Face => "Face",
        ElementKind::Solid => "Solid",
    }
}

/// A count as Silico's form and FreeCAD's write it: nothing for 1.
fn count_text(n: NonZeroU32) -> String {
    if n.get() > 1 {
        n.to_string()
    } else {
        String::new()
    }
}

fn relation_letter(relation: Relation) -> &'static str {
    match relation {
        Relation::Modified => "M",
        Relation::Generated => "G",
        Relation::Inside => "I",
    }
}

/// A relation's letter and position, as FreeCAD writes them: `M`, `G2`.
fn relation_text(relation: Relation, position: NonZeroU32) -> String {
    format!("{}{}", relation_letter(relation), count_text(position))
}

/// Silico's form (N1): the feature that made the element, its type, the element's kind,
/// and how it came about.
impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Name::Element {
                feature,
                made,
                element,
            } => write!(f, "{feature}.{made}.{element}"),
            Name::Derived {
                feature,
                operation,
                kind,
                first,
                more,
            } => {
                write!(f, "{feature}.{}.{}(", operation.word(), kind_word(*kind))?;
                for (i, source) in std::iter::once(first).chain(more).enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    let letter = relation_letter(source.relation);
                    match source.place {
                        Some(place) => {
                            write!(f, "{letter}{}/{} {}", place.rank, place.size, source.name)?
                        }
                        None => write!(f, "{letter} {}", source.name)?,
                    }
                }
                f.write_str(")")
            }
            Name::Upper {
                feature,
                operation,
                kind,
                upper,
                more,
                place,
                ..
            } => {
                write!(f, "{feature}.{}.{}(U", operation.word(), kind_word(*kind))?;
                if let Some(place) = place {
                    write!(f, "{}/{}", place.rank, place.size)?;
                }
                write!(f, " {upper}")?;
                for name in more {
                    write!(f, ", {name}")?;
                }
                f.write_str(")")
            }
            Name::Lower {
                feature,
                operation,
                kind,
                first,
                more,
            } => {
                write!(f, "{feature}.{}.{}(L ", operation.word(), kind_word(*kind))?;
                for (i, lower) in std::iter::once(first).chain(more).enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{lower}")?;
                }
                f.write_str(")")
            }
            Name::Duplicate { name, ordinal } => write!(f, "{name}~{ordinal}"),
            Name::Modified { name, .. } => write!(f, "{name}"),
            Name::Sketch { feature, kind, id } => write!(
                f,
                "{feature}.{}.{}.{id}",
                OpCode::Sketch.word(),
                kind_word(*kind)
            ),
            Name::Copy {
                feature,
                operation,
                kind,
                copied,
                instance,
                of,
            } => write!(
                f,
                "{feature}.{}.{}({}{instance} {of})",
                operation.word(),
                kind_word(*kind),
                copied.letter()
            ),
        }
    }
}

/// A solid's element names: each element's names (the first its own, any others other
/// derivations that also reach it), and each name's element, so a name is looked up either
/// way. Every name names one element; every face, edge, and vertex has a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementMap {
    /// The feature that made the solid, and its type; any map names an element nothing
    /// else names by its number, `feature.cut.Face7`.
    feature: FeatureId,
    made: TypeWord,
    names: BTreeMap<ElementRef, Vec<Arc<Name>>>,
    elements: BTreeMap<Arc<Name>, ElementRef>,
    /// Its names that are unstable ([`ElementMap::is_unstable`]), decided when it was made:
    /// only its own, so nothing is carried from the solids it was made from that it no
    /// longer names (the review found a set carried forward growing with the square of the
    /// timeline).
    unstable: BTreeSet<Arc<Name>>,
}

impl ElementMap {
    /// A primitive's map: each of its faces, edges, and vertices named by its type and its
    /// number (`k7f2qx3m.box.Face3`, so a name says what made it), as in the primitive's fullest form (`kernel::primitive_numbers`:
    /// a pointed cone's base cap is its Face3, as a cone's always is), so a number never
    /// names another element after an edit of the primitive's values.
    pub fn primitive(feature: FeatureId, primitive: &crate::kernel::Primitive) -> Self {
        let mut map = ElementMap {
            feature,
            made: TypeWord::Primitive(PrimitiveKind::of(primitive)),
            names: BTreeMap::new(),
            elements: BTreeMap::new(),
            unstable: BTreeSet::new(),
        };
        for kind in [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex] {
            for (index, number) in (1..).zip(crate::kernel::primitive_numbers(primitive, kind)) {
                let element = ElementRef { kind, index };
                let name = Arc::new(Name::Element {
                    feature: map.feature.clone(),
                    made: map.made,
                    element: ElementRef {
                        kind,
                        index: number,
                    },
                });
                map.elements.insert(Arc::clone(&name), element);
                map.names.insert(element, vec![name]);
            }
        }
        map
    }

    /// A map of `counts` faces, edges, and vertices, each named by OCCT's number for it
    /// (`k7f2qx3m.cut.Face3`), as `operation` names an element no history names: a solid
    /// made up for a test, of any counts. A primitive's map is [`ElementMap::primitive`],
    /// numbered as in its fullest form.
    pub fn by_number(feature: FeatureId, operation: OpCode, counts: ElementCounts) -> Self {
        let mut map = ElementMap {
            feature,
            made: TypeWord::Operation(operation),
            names: BTreeMap::new(),
            elements: BTreeMap::new(),
            unstable: BTreeSet::new(),
        };
        for kind in [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex] {
            for index in 1..=counts.of(kind) {
                let element = ElementRef { kind, index };
                let name = Arc::new(map.element_name(element));
                map.elements.insert(Arc::clone(&name), element);
                map.names.insert(element, vec![name]);
            }
        }
        map
    }

    /// The feature that made the solid.
    pub fn feature(&self) -> &FeatureId {
        &self.feature
    }

    /// The type of the feature that made the solid, as its names write it.
    pub fn made(&self) -> TypeWord {
        self.made
    }

    /// The element's own name (its first).
    pub fn name(&self, element: ElementRef) -> Option<Arc<Name>> {
        self.names
            .get(&element)
            .and_then(|names| names.first().cloned())
    }

    /// Every name of the element, its own first.
    pub fn names(&self, element: ElementRef) -> Vec<Arc<Name>> {
        self.names.get(&element).cloned().unwrap_or_default()
    }

    /// The element `name` names, if any.
    pub fn element(&self, name: &Name) -> Option<ElementRef> {
        self.elements.get(name).copied()
    }

    /// Whether `name` is unstable, so a reference to it would be refused (N3): it came about
    /// for more than one element (here, or in a solid this one was made from: a kept
    /// element keeps it), it's one of those elements' `~n`, it's an operation's element by
    /// its number (nothing else named it), or it was derived from such a name. Which
    /// element it names depends on how elements are numbered, which an upstream edit can
    /// change.
    pub fn is_unstable(&self, name: &Name) -> bool {
        self.unstable.contains(name)
            || matches!(name, Name::Duplicate { .. })
            || name.parts().into_iter().any(|part| self.is_unstable(part))
    }

    /// Its unstable names, in order.
    pub fn unstable(&self) -> impl Iterator<Item = &Arc<Name>> {
        self.unstable.iter()
    }

    /// How many elements of `kind` it names.
    pub fn count(&self, kind: ElementKind) -> usize {
        self.names.keys().filter(|e| e.kind == kind).count()
    }

    /// Every element and its names, in element order.
    pub fn iter(&self) -> impl Iterator<Item = (ElementRef, &[Arc<Name>])> {
        self.names
            .iter()
            .map(|(element, names)| (*element, names.as_slice()))
    }

    fn element_name(&self, element: ElementRef) -> Name {
        Name::Element {
            feature: self.feature.clone(),
            made: self.made,
            element,
        }
    }
}
