//! Topological naming (M2.7, docs/ports/naming.md, docs/design/m2.7-naming.md): every face,
//! edge, and vertex of a feature's solid gets a name that says how it came about, derived
//! from the kernel's history as FreeCAD's `makeShapeWithElementMap` derives it, so a
//! reference to an element can follow it through upstream edits.
//!
//! A [`Name`] is structured, not a string; it's written in Silico's compact form
//! (`m3tq8v0z.Edge(G2 k7f2qx3m.Face6, G2 a91cw0de.Face1)`), and [`freecad`] renders FreeCAD's
//! form for comparison with FreeCAD's names. An [`ElementMap`] holds a solid's names, built
//! by [`ElementMap::primitive`], [`ElementMap::operation`], or kept through a move.

mod build;
pub mod freecad;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;

use crate::document::FeatureId;
use crate::kernel::{BooleanOp, ElementCounts, ElementKind, ElementRef};

/// The operation that derived an element, as FreeCAD's op code names it (`FUS`, `CUT`, ...).
/// It isn't written in Silico's form, where the feature that made the element says it, but
/// it's part of FreeCAD's names, and of the order in which FreeCAD picks a name's sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OpCode {
    Fuse,
    Cut,
    Common,
    Fillet,
    Chamfer,
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
    /// FreeCAD's op code (`TopoShapeOpCode.h`).
    pub fn code(self) -> &'static str {
        match self {
            OpCode::Fuse => "FUS",
            OpCode::Cut => "CUT",
            OpCode::Common => "CMN",
            OpCode::Fillet => "FLT",
            OpCode::Chamfer => "CHF",
        }
    }
}

/// How a source element became the element named: OCCT's `Modified` or `Generated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    Modified,
    Generated,
}

/// One of the elements an element came from, how, and where in OCCT's list for it the
/// element was (from 1, counting entries the result doesn't have).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Source {
    pub relation: Relation,
    pub position: NonZeroU32,
    pub name: Arc<Name>,
}

/// An element's name: how it came about, down to the elements features made of nothing.
/// Every count in it is from 1, and every list has a first, so no two names are written
/// alike.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Name {
    /// The element numbered so in what `feature` made: a primitive's own `Face3`. Also, as
    /// FreeCAD's unmapped names are, the last resort for an element nothing else names.
    Element {
        feature: FeatureId,
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
    /// An element modified from one element, `name`, as OCCT's first (and only) result of
    /// it, by one feature or by a run of them, each the same way (Trevin's decision,
    /// 2026-10-16): written `pppppppp.Face6*`, with no feature in between, so a plate's top
    /// face is the same name after any number of holes cut through it, and a hole's rim
    /// named from it doesn't change when an earlier hole is added, removed, or moved.
    /// Unique: such a step makes one element of one, so two elements of a result never
    /// both are. FreeCAD writes a level per feature (`Face6;:M;CUT;:H..;:M;CUT;:H..`),
    /// hashed.
    Modified { name: Arc<Name>, last: LastOp },
    /// An element nothing reported, named as the `nth` unnamed element of its kind in the
    /// element above it named `upper` (FreeCAD's `;:U`).
    Upper {
        feature: FeatureId,
        operation: OpCode,
        kind: ElementKind,
        upper: Arc<Name>,
        nth: NonZeroU32,
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
            Name::Derived { kind, .. } | Name::Upper { kind, .. } | Name::Lower { kind, .. } => {
                *kind
            }
            Name::Duplicate { name, .. } | Name::Modified { name, .. } => name.kind(),
        }
    }

    /// The names it's made of, one level down.
    pub(crate) fn parts(&self) -> Vec<&Arc<Name>> {
        match self {
            Name::Element { .. } => Vec::new(),
            Name::Derived { first, more, .. } => std::iter::once(first)
                .chain(more)
                .map(|source| &source.name)
                .collect(),
            Name::Upper { upper, .. } => vec![upper],
            Name::Lower { first, more, .. } => std::iter::once(first).chain(more).collect(),
            Name::Duplicate { name, .. } | Name::Modified { name, .. } => vec![name],
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

/// A relation's letter and position, as Silico's form and FreeCAD's write them: `M`, `G2`.
fn relation_text(relation: Relation, position: NonZeroU32) -> String {
    let letter = match relation {
        Relation::Modified => "M",
        Relation::Generated => "G",
    };
    format!("{letter}{}", count_text(position))
}

/// Silico's form (N1): the feature that made the element, its kind, and how it came about.
impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Name::Element { feature, element } => write!(f, "{feature}.{element}"),
            Name::Derived {
                feature,
                kind,
                first,
                more,
                ..
            } => {
                write!(f, "{feature}.{}(", kind_word(*kind))?;
                for (i, source) in std::iter::once(first).chain(more).enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    let relation = relation_text(source.relation, source.position);
                    write!(f, "{relation} {}", source.name)?;
                }
                f.write_str(")")
            }
            Name::Upper {
                feature,
                kind,
                upper,
                nth,
                ..
            } => write!(
                f,
                "{feature}.{}(U{} {upper})",
                kind_word(*kind),
                count_text(*nth)
            ),
            Name::Lower {
                feature,
                kind,
                first,
                more,
                ..
            } => {
                write!(f, "{feature}.{}(L ", kind_word(*kind))?;
                for (i, lower) in std::iter::once(first).chain(more).enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{lower}")?;
                }
                f.write_str(")")
            }
            Name::Duplicate { name, ordinal } => write!(f, "{name}~{ordinal}"),
            Name::Modified { name, .. } => write!(f, "{name}*"),
        }
    }
}

/// A solid's element names: each element's names (the first its own, any others other
/// derivations that also reach it), and each name's element, so a name is looked up either
/// way. Every name names one element; every face, edge, and vertex has a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementMap {
    /// The feature that made the solid; any map names an element nothing else names by
    /// its number, `feature.Face7`.
    feature: FeatureId,
    names: BTreeMap<ElementRef, Vec<Arc<Name>>>,
    elements: BTreeMap<Arc<Name>, ElementRef>,
    /// Its names that are unstable ([`ElementMap::is_unstable`]), decided when it was made:
    /// only its own, so nothing is carried from the solids it was made from that it no
    /// longer names (the review found a set carried forward growing with the square of the
    /// timeline).
    unstable: BTreeSet<Arc<Name>>,
}

impl ElementMap {
    /// A primitive's map: each of its `counts` faces, edges, and vertices named by its
    /// number (`k7f2qx3m.Face3`), as the kernel made it, which is OCCT's order (and
    /// FreeCAD's `Face3`).
    pub fn primitive(feature: FeatureId, counts: ElementCounts) -> Self {
        let mut map = ElementMap {
            feature,
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

    /// Every element and its names, in element order.
    pub fn iter(&self) -> impl Iterator<Item = (ElementRef, &[Arc<Name>])> {
        self.names
            .iter()
            .map(|(element, names)| (*element, names.as_slice()))
    }

    fn element_name(&self, element: ElementRef) -> Name {
        Name::Element {
            feature: self.feature.clone(),
            element,
        }
    }
}
