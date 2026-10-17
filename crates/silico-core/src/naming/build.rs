//! Naming an operation's result from its history: FreeCAD's `makeShapeWithElementMap`
//! (`TopoShapeExpansion.cpp:1420-2010`), driven by the kernel's [`History`] rather than
//! OCCT's builders (docs/ports/naming.md has the algorithm and where Silico differs).
//!
//! In FreeCAD's order:
//! 1. **Kept**: an input element the result has unchanged keeps all its names (N2: as they
//!    are, with no level added).
//! 2. **Modified and generated**: every other element OCCT reports is named from the input
//!    elements it came from, the first by FreeCAD's key (the source's kind, vertex first,
//!    then its input, then its name), then up to four more, each with its position.
//! 3. **From above**: an element still unnamed that bounds a named face (an edge) or a named
//!    edge (a vertex) is named as the face's or edge's `n`th unnamed element of its kind,
//!    from the one whose name sorts first by FreeCAD's comparator.
//! 4. **From below**: an edge still unnamed is named from its vertices, then a face from its
//!    outer wire's edges, once all of those are named.
//! 5. Anything still unnamed is named by its number (FreeCAD leaves it unnamed, and its
//!    references then use the number).
//!
//! FreeCAD's second round, which names elements from a whole generated solid (an
//! extrusion's top), arrives with extrusions (M3); none of M2's operations reports one.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;

use super::{ElementMap, Name, Operation, Relation, Source, freecad};
use crate::document::FeatureId;
use crate::kernel::{ElementKind, ElementRef, History};

/// The most sources a name lists, the first and four more, as FreeCAD's do.
const MOST_SOURCES: usize = 5;

/// The kinds of element named, from the lowest: FreeCAD walks sources in this order.
const KINDS: [ElementKind; 3] = [ElementKind::Vertex, ElementKind::Edge, ElementKind::Face];

impl ElementMap {
    /// The map of `operation`'s result, made by `feature` from `inputs` (their maps, in the
    /// operation's order) with `history`, which the core has checked (`History::validate`).
    pub fn operation(
        feature: FeatureId,
        operation: Operation,
        inputs: &[&ElementMap],
        history: &History,
    ) -> Self {
        // An ambiguous name stays ambiguous in what's made from it.
        let duplicated: BTreeSet<Arc<Name>> = inputs
            .iter()
            .flat_map(|input| input.duplicated.iter().cloned())
            .collect();
        let mut builder = Builder {
            map: ElementMap {
                feature,
                primitive: false,
                names: BTreeMap::new(),
                elements: BTreeMap::new(),
                duplicated,
            },
            operation,
        };
        builder.kept(inputs, history);
        builder.derived(inputs, history);
        builder.named_from_above(history);
        builder.named_from_below(history);
        builder.by_number(history);
        builder.map
    }
}

/// FreeCAD's `NameKey`, which orders an element's sources: the source's kind (vertex, edge,
/// face), its input (FreeCAD's tag, the input object's ID, so the base before the tool in
/// FreeCAD's tests and documents alike), and its name as FreeCAD writes it, byte by byte;
/// then Silico's name, so the order is total.
type SourceKey = (u8, usize, String, Arc<Name>);

struct Builder {
    map: ElementMap,
    operation: Operation,
}

impl Builder {
    fn is_named(&self, element: ElementRef) -> bool {
        self.map.names.get(&element).is_some_and(|n| !n.is_empty())
    }

    fn primary(&self, element: ElementRef) -> Option<Arc<Name>> {
        self.map
            .names
            .get(&element)
            .and_then(|n| n.first().cloned())
    }

    /// Gives `element` the name `name`, unless it has it already. A name another element
    /// has is that name's next free `~n` (N3; FreeCAD's `;D<n>`, numbered in its debug
    /// builds, random in its releases), and the name is marked duplicated.
    fn set(&mut self, element: ElementRef, name: Arc<Name>) {
        let mut candidate = Arc::clone(&name);
        let mut ordinal = NonZeroU32::MIN;
        loop {
            match self.map.elements.get(&candidate) {
                None => break,
                Some(&holder) if holder == element => return,
                Some(_) => {
                    self.map.duplicated.insert(Arc::clone(&name));
                    candidate = Arc::new(Name::Duplicate {
                        name: Arc::clone(&name),
                        ordinal,
                    });
                    ordinal = ordinal.saturating_add(1);
                }
            }
        }
        self.map.elements.insert(Arc::clone(&candidate), element);
        self.map.names.entry(element).or_default().push(candidate);
    }

    /// Step 1: each input element the result has unchanged keeps its names.
    fn kept(&mut self, inputs: &[&ElementMap], history: &History) {
        for (input, map) in history.inputs.iter().zip(inputs) {
            for kind in KINDS {
                for (n, fate) in fates(input, kind).iter().enumerate() {
                    let Some(kept) = fate.kept else { continue };
                    let source = element(kind, n);
                    for name in input_names(map, source) {
                        self.set(element_ref(kind, kept), name);
                    }
                }
            }
        }
    }

    /// Step 2: each element OCCT reports modified or generated, still unnamed, from its
    /// sources.
    fn derived(&mut self, inputs: &[&ElementMap], history: &History) {
        // Each result element's sources, in FreeCAD's order of elements (edges, then faces,
        // then vertices), which decides only which of two that come about alike is `~1`.
        let mut sources: BTreeMap<(u8, ElementRef), BTreeMap<SourceKey, (Relation, NonZeroU32)>> =
            BTreeMap::new();
        for kind in KINDS {
            for (i, (input, map)) in history.inputs.iter().zip(inputs).enumerate() {
                for (n, fate) in fates(input, kind).iter().enumerate() {
                    if fate.modified.is_empty() && fate.generated.is_empty() {
                        continue;
                    }
                    let source = element(kind, n);
                    let name = input_name(map, source);
                    let key: SourceKey = (rank(kind), i, freecad::text(&name), name);
                    // Generated after modified, so for a source that did both to one
                    // element, generated counts (FreeCAD's map overwrites the first).
                    let reported = fate
                        .modified
                        .iter()
                        .map(|r| (r, Relation::Modified))
                        .chain(fate.generated.iter().map(|r| (r, Relation::Generated)));
                    for (r, relation) in reported {
                        // A whole solid generated (an extrusion's) names nothing here; nor
                        // does a position of 0, which a checked history never has.
                        let Some(position) = NonZeroU32::new(r.position) else {
                            continue;
                        };
                        if r.element.kind == ElementKind::Solid || self.is_named(r.element) {
                            continue;
                        }
                        sources
                            .entry((freecad_rank(r.element.kind), r.element))
                            .or_default()
                            .insert(key.clone(), (relation, position));
                    }
                }
            }
        }
        for ((_, element), keyed) in sources {
            if self.is_named(element) {
                continue;
            }
            let mut sources = keyed.into_iter().take(MOST_SOURCES).map(
                |((_, _, _, name), (relation, position))| Source {
                    relation,
                    position,
                    name,
                },
            );
            let Some(first) = sources.next() else {
                continue;
            };
            let name = Name::Derived {
                feature: self.map.feature.clone(),
                operation: self.operation,
                kind: element.kind,
                first,
                more: sources.collect(),
            };
            self.set(element, Arc::new(name));
        }
    }

    /// Step 3: each unnamed edge from a named face it bounds, then each unnamed vertex from
    /// a named edge, as the `n`th unnamed one in the face's (edge's) own order, from the
    /// face (edge) whose name sorts first.
    fn named_from_above(&mut self, history: &History) {
        let t = &history.topology;
        for (upper_kind, lower_kind, lists) in [
            (ElementKind::Face, ElementKind::Edge, &t.face_edges),
            (ElementKind::Edge, ElementKind::Vertex, &t.edge_vertices),
        ] {
            let mut candidates: BTreeMap<ElementRef, Vec<(String, Arc<Name>, NonZeroU32)>> =
                BTreeMap::new();
            for (u, lowers) in lists.iter().enumerate() {
                let Some(upper) = self.primary(element(upper_kind, u)) else {
                    continue;
                };
                let text = freecad::text(&upper);
                let mut nth = NonZeroU32::MIN;
                for &lower in lowers {
                    let lower = element_ref(lower_kind, lower);
                    if self.is_named(lower) {
                        continue;
                    }
                    candidates.entry(lower).or_default().push((
                        text.clone(),
                        Arc::clone(&upper),
                        nth,
                    ));
                    nth = nth.saturating_add(1);
                }
            }
            for (lower, mut uppers) in candidates {
                uppers.sort_by(|a, b| freecad::compare(&a.0, &b.0).then_with(|| a.1.cmp(&b.1)));
                let Some((_, upper, nth)) = uppers.into_iter().next() else {
                    continue;
                };
                let name = Name::Upper {
                    feature: self.map.feature.clone(),
                    operation: self.operation,
                    kind: lower_kind,
                    upper,
                    nth,
                };
                self.set(lower, Arc::new(name));
            }
        }
    }

    /// Step 4: each unnamed edge from its vertices, then each unnamed face from its outer
    /// wire's edges, once every one of those is named, from the first by FreeCAD's
    /// comparator, and up to four more.
    fn named_from_below(&mut self, history: &History) {
        let t = &history.topology;
        for (kind, lower_kind, lists) in [
            (ElementKind::Edge, ElementKind::Vertex, &t.edge_vertices),
            (ElementKind::Face, ElementKind::Edge, &t.outer_edges),
        ] {
            for (n, lowers) in lists.iter().enumerate() {
                let this = element(kind, n);
                if self.is_named(this) || lowers.is_empty() {
                    continue;
                }
                let named: Option<Vec<Arc<Name>>> = lowers
                    .iter()
                    .map(|&l| self.primary(element_ref(lower_kind, l)))
                    .collect();
                let Some(named) = named else { continue };
                let mut named: Vec<(String, Arc<Name>)> = named
                    .into_iter()
                    .map(|name| (freecad::text(&name), name))
                    .collect();
                named.sort_by(|a, b| freecad::compare(&a.0, &b.0).then_with(|| a.1.cmp(&b.1)));
                named.dedup_by(|a, b| a.1 == b.1);
                let mut lowers = named.into_iter().take(MOST_SOURCES).map(|(_, name)| name);
                let Some(first) = lowers.next() else {
                    continue;
                };
                let name = Name::Lower {
                    feature: self.map.feature.clone(),
                    operation: self.operation,
                    kind,
                    first,
                    more: lowers.collect(),
                };
                self.set(this, Arc::new(name));
            }
        }
    }

    /// Step 5: each element still unnamed, by its number in the result.
    fn by_number(&mut self, history: &History) {
        let counts = history.result;
        for kind in KINDS {
            for index in 1..=counts.of(kind) {
                let this = element_ref(kind, index);
                if !self.is_named(this) {
                    let name = self.map.element_name(this);
                    self.set(this, Arc::new(name));
                }
            }
        }
    }
}

/// An input's fates for elements of `kind`.
fn fates(input: &crate::kernel::InputHistory, kind: ElementKind) -> &[crate::kernel::ElementFate] {
    match kind {
        ElementKind::Face => &input.faces,
        ElementKind::Edge => &input.edges,
        ElementKind::Vertex => &input.vertices,
        ElementKind::Solid => &[],
    }
}

/// The element of `kind` at `n` (from 0) in a list.
fn element(kind: ElementKind, n: usize) -> ElementRef {
    element_ref(kind, u32::try_from(n).unwrap_or(u32::MAX).saturating_add(1))
}

fn element_ref(kind: ElementKind, index: u32) -> ElementRef {
    ElementRef { kind, index }
}

/// An input element's names; one by its number if its map has none (FreeCAD's unmapped
/// names), though every map Silico builds names every element.
fn input_names(map: &ElementMap, element: ElementRef) -> Vec<Arc<Name>> {
    let names = map.names(element);
    if names.is_empty() {
        vec![Arc::new(map.element_name(element))]
    } else {
        names
    }
}

/// An input element's own name, as [`input_names`] finds it.
fn input_name(map: &ElementMap, element: ElementRef) -> Arc<Name> {
    map.name(element)
        .unwrap_or_else(|| Arc::new(map.element_name(element)))
}

/// A source's rank in FreeCAD's `NameKey`: vertex, edge, face.
fn rank(kind: ElementKind) -> u8 {
    match kind {
        ElementKind::Vertex => 0,
        ElementKind::Edge => 1,
        ElementKind::Face => 2,
        ElementKind::Solid => 3,
    }
}

/// An element's place in FreeCAD's `IndexedName` order, its type's name compared as text:
/// `Edge`, `Face`, `Vertex`.
fn freecad_rank(kind: ElementKind) -> u8 {
    match kind {
        ElementKind::Edge => 0,
        ElementKind::Face => 1,
        ElementKind::Vertex => 2,
        ElementKind::Solid => 3,
    }
}
