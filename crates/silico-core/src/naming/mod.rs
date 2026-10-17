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
use std::sync::Arc;

use crate::document::FeatureId;
use crate::kernel::{ElementKind, ElementRef};

/// The operation that derived an element, as FreeCAD's op code names it (`FUS`, `CUT`, ...).
/// It isn't written in Silico's form, where the feature that made the element says it, but
/// it's part of FreeCAD's names, and of the order in which FreeCAD picks a name's sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    Fuse,
    Cut,
    Common,
    Fillet,
    Chamfer,
}

impl Operation {
    /// FreeCAD's op code (`TopoShapeOpCode.h`).
    pub fn code(self) -> &'static str {
        match self {
            Operation::Fuse => "FUS",
            Operation::Cut => "CUT",
            Operation::Common => "CMN",
            Operation::Fillet => "FLT",
            Operation::Chamfer => "CHF",
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
    pub position: u32,
    pub name: Arc<Name>,
}

/// An element's name: how it came about, down to the elements features made of nothing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Name {
    /// The element numbered so in what `feature` made: a primitive's own `Face3`. Also, as
    /// FreeCAD's unmapped names are, the last resort for an element nothing else names.
    Element {
        feature: FeatureId,
        element: ElementRef,
    },
    /// An element `feature`'s operation made from its inputs' elements, as OCCT reported
    /// it: the first source first (by its kind, then input, then name, as FreeCAD orders
    /// them), then up to four more.
    Derived {
        feature: FeatureId,
        operation: Operation,
        kind: ElementKind,
        sources: Vec<Source>,
    },
    /// An element nothing reported, named as the `nth` unnamed element of its kind in the
    /// element above it named `upper` (FreeCAD's `;:U`).
    Upper {
        feature: FeatureId,
        operation: Operation,
        kind: ElementKind,
        upper: Arc<Name>,
        nth: u32,
    },
    /// An element named from the elements below it (an edge from its vertices, a face from
    /// its outer wire's edges): the first in FreeCAD's order, then up to four more
    /// (FreeCAD's `;:L`).
    Lower {
        feature: FeatureId,
        operation: Operation,
        kind: ElementKind,
        lowers: Vec<Arc<Name>>,
    },
    /// The `ordinal`th of the elements that came about as `name` did, after the first:
    /// rare, and never stable, so a reference to one is refused (N3).
    Duplicate { name: Arc<Name>, ordinal: u32 },
}

impl Name {
    /// The kind of element it names.
    pub fn kind(&self) -> ElementKind {
        match self {
            Name::Element { element, .. } => element.kind,
            Name::Derived { kind, .. } | Name::Upper { kind, .. } | Name::Lower { kind, .. } => {
                *kind
            }
            Name::Duplicate { name, .. } => name.kind(),
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

/// A relation's letter and position, as Silico's form and FreeCAD's write them: `M`, `G2`.
fn relation_text(relation: Relation, position: u32) -> String {
    let letter = match relation {
        Relation::Modified => "M",
        Relation::Generated => "G",
    };
    if position > 1 {
        format!("{letter}{position}")
    } else {
        letter.to_owned()
    }
}

/// Silico's form (N1): the feature that made the element, its kind, and how it came about.
impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Name::Element { feature, element } => {
                write!(f, "{feature}.{}{}", kind_word(element.kind), element.index)
            }
            Name::Derived {
                feature,
                kind,
                sources,
                ..
            } => {
                write!(f, "{feature}.{}(", kind_word(*kind))?;
                for (i, source) in sources.iter().enumerate() {
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
            } => {
                let nth = if *nth > 1 {
                    nth.to_string()
                } else {
                    String::new()
                };
                write!(f, "{feature}.{}(U{nth} {upper})", kind_word(*kind))
            }
            Name::Lower {
                feature,
                kind,
                lowers,
                ..
            } => {
                write!(f, "{feature}.{}(L ", kind_word(*kind))?;
                for (i, lower) in lowers.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{lower}")?;
                }
                f.write_str(")")
            }
            Name::Duplicate { name, ordinal } => write!(f, "{name}~{ordinal}"),
        }
    }
}

/// A solid's element names: each element's names (the first its own, any others other
/// derivations that also reach it), and each name's element, so a name is looked up either
/// way. Every name names one element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementMap {
    /// The feature that made the solid: a primitive's map names every element `feature.KindN`
    /// without listing them, and any map names an element nothing else names so.
    feature: FeatureId,
    primitive: bool,
    names: BTreeMap<ElementRef, Vec<Arc<Name>>>,
    elements: BTreeMap<Arc<Name>, ElementRef>,
    /// Names two or more elements came about by (each but the first then `~1`, `~2`, ...).
    duplicated: BTreeSet<Arc<Name>>,
}

impl ElementMap {
    /// A primitive's map: each element is named by its number (`k7f2qx3m.Face3`), as the
    /// kernel made it, which is OCCT's order (and FreeCAD's `Face3`).
    pub fn primitive(feature: FeatureId) -> Self {
        ElementMap {
            feature,
            primitive: true,
            names: BTreeMap::new(),
            elements: BTreeMap::new(),
            duplicated: BTreeSet::new(),
        }
    }

    /// The feature that made the solid.
    pub fn feature(&self) -> &FeatureId {
        &self.feature
    }

    /// The element's own name (its first).
    pub fn name(&self, element: ElementRef) -> Option<Arc<Name>> {
        if self.primitive {
            return Some(Arc::new(self.element_name(element)));
        }
        self.names
            .get(&element)
            .and_then(|names| names.first().cloned())
    }

    /// Every name of the element, its own first.
    pub fn names(&self, element: ElementRef) -> Vec<Arc<Name>> {
        if self.primitive {
            return vec![Arc::new(self.element_name(element))];
        }
        self.names.get(&element).cloned().unwrap_or_default()
    }

    /// The element `name` names, if any. A primitive's map can't say how many elements its
    /// solid has, so it answers for any number; the solid's own counts bound it.
    pub fn element(&self, name: &Name) -> Option<ElementRef> {
        if self.primitive {
            return match name {
                Name::Element { feature, element } if *feature == self.feature => Some(*element),
                _ => None,
            };
        }
        self.elements.get(name).copied()
    }

    /// Whether `name` came about for more than one element, so it and its `~n` forms are
    /// ambiguous.
    pub fn is_duplicated(&self, name: &Name) -> bool {
        self.duplicated.contains(name)
    }

    /// Every named element and its names, in element order. Empty for a primitive's map,
    /// which lists none.
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
