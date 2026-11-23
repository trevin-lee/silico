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
//! extrusion's top), isn't needed: a sweep of a profile's faces reports each element's end
//! itself (OCCT's `LastShape`, docs/design/m3.6-sketch-features.md, S4), and its whole solid
//! names nothing. Which side of each input a step keeps is `OpCode::side`: a sweep keeps
//! neither side of its profile, so its start elements are its own, one `M`, never collapsed.
//! A sketch's faces are named by its IDs ([`ElementMap::sketch`]), not from history.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;

use super::{
    Copied, ElementMap, Instance, LastOp, Name, OpCode, Place, Raw, Relation, Side, Source, Stage,
    TypeWord, freecad,
};
use crate::document::{ElementId, FeatureId};
use crate::kernel::{ElementKind, ElementRef, History};

/// The most sources a name lists, the first and four more, as FreeCAD's do.
pub(crate) const MOST_SOURCES: usize = 5;

/// The deepest a name may nest, and the most names it may be made of, counted as written
/// out (a name used twice counts twice). Every walk over a name (writing it, comparing it)
/// recurses and is as long as the name written out, so these bound them all: a timeline
/// thousands of features deep overflowed the recompute thread's stack, and a worker's
/// history that names two faces from the same two doubled a name with every feature (the
/// review found both). FreeCAD bounds the same growth with its string table, which Silico
/// doesn't have (N1). An element whose name would be past either is named by its number,
/// and that name is unstable.
pub(crate) const MOST_DEPTH: u32 = 256;
pub(crate) const MOST_WEIGHT: u64 = 4096;

/// The kinds of element named, from the lowest: FreeCAD walks sources in this order.
const KINDS: [ElementKind; 3] = [ElementKind::Vertex, ElementKind::Edge, ElementKind::Face];

impl ElementMap {
    /// The map of `operation`'s result, made by `feature` from `inputs` (their maps, in the
    /// operation's order) with `history`, which the core has checked (`History::validate`).
    pub fn operation(
        feature: FeatureId,
        operation: OpCode,
        inputs: &[&ElementMap],
        history: &History,
    ) -> Self {
        Self::step(feature, operation, Stage::Last, inputs, history)
    }

    /// The map of a pad's, pocket's, revolve's, or groove's sweep of its profile's faces
    /// (`faces`, the sketch's map: [`ElementMap::sketch`]), by `history`, which the core
    /// has checked (`History::validate`, `History::sweep_fault`): every element the
    /// feature's own, named from the sketch's elements in one level, the start counterparts
    /// `M` and never collapsed, since the sweep keeps neither side of its profile
    /// (docs/design/m3.6-sketch-features.md, S4: `p.pad.Face(G s.sketch.Edge.c)`, a side;
    /// `p.pad.Face(M s.sketch.Face1)`, the bottom; `p.pad.Face(G s.sketch.Face1)`, the top).
    /// The feature's fuse or cut then names its solid from its base's map and this one
    /// ([`ElementMap::operation`], whose every input a sweep feature bites).
    pub fn swept(
        feature: FeatureId,
        operation: OpCode,
        faces: &ElementMap,
        history: &History,
    ) -> Self {
        Self::step(feature, operation, Stage::Sweep, &[faces], history)
    }

    fn step(
        feature: FeatureId,
        operation: OpCode,
        stage: Stage,
        inputs: &[&ElementMap],
        history: &History,
    ) -> Self {
        let mut builder = Builder::new(feature, operation, stage);
        builder.kept(inputs, history);
        builder.derived(inputs, history);
        builder.named_from_above(history);
        builder.named_from_below(history);
        builder.by_number(history);
        builder.settle(inputs);
        builder.map
    }

    /// The map of the faces a sketch's profile makes (`KernelRequest::make_faces`), named
    /// from the sketch's IDs (design S4), with `history` the faces' (which the core has
    /// checked: each curve kept as one edge, each point as one vertex, face `n` region
    /// `n`): curve `curves[i]`'s edge `feature.sketch.Edge.<id>`, the `i`th sent; point
    /// `points[k]`'s vertex `feature.sketch.Vertex.<id>`, the `k`th sent; a circle's one
    /// vertex, which no point is, from the edge it bounds, `feature.sketch.Vertex(U
    /// feature.sketch.Edge.<id>)`; the only region `feature.sketch.Face1`, whatever its
    /// curves; and each of several regions from below, by `regions[n]`, its outer loop's
    /// curve with the smallest ID, `feature.sketch.Face(L feature.sketch.Edge.<id>)`. An
    /// element nothing names is named by its number, unstable.
    pub fn sketch(
        feature: FeatureId,
        curves: &[ElementId],
        points: &[ElementId],
        regions: &[ElementId],
        history: &History,
    ) -> Self {
        let mut builder = Builder::new(feature.clone(), OpCode::Sketch, Stage::Last);
        let id_name = |kind, id: &ElementId| {
            Arc::new(Name::Sketch {
                feature: feature.clone(),
                kind,
                id: id.clone(),
            })
        };
        if let Some(input) = history.inputs.first() {
            for (kind, ids, fates) in [
                (ElementKind::Edge, curves, &input.edges),
                (ElementKind::Vertex, points, &input.vertices),
            ] {
                for (id, fate) in ids.iter().zip(fates) {
                    if let Some(kept) = fate.kept {
                        builder.set(element_ref(kind, kept), id_name(kind, id));
                    }
                }
            }
        }
        // Each vertex no point is (a circle's), from the edges it bounds.
        let mut above: BTreeMap<u32, Vec<Arc<Name>>> = BTreeMap::new();
        for (e, ends) in history.topology.edge_vertices.iter().enumerate() {
            let Some(edge) = builder.primary(element(ElementKind::Edge, e)) else {
                continue;
            };
            for &v in ends {
                if !builder.is_named(element_ref(ElementKind::Vertex, v)) {
                    above.entry(v).or_default().push(Arc::clone(&edge));
                }
            }
        }
        for (v, mut edges) in above {
            edges.sort();
            edges.dedup();
            let mut edges = edges.into_iter().take(MOST_SOURCES);
            let Some(upper) = edges.next() else { continue };
            let name = Name::Upper {
                feature: feature.clone(),
                operation: OpCode::Sketch,
                kind: ElementKind::Vertex,
                upper,
                more: edges.collect(),
                place: None,
                nth: Raw(NonZeroU32::MIN),
            };
            builder.set(element_ref(ElementKind::Vertex, v), Arc::new(name));
        }
        // The regions: the only one Face1, several each by its outer loop's smallest curve.
        let faces = history.result.faces;
        if faces == 1 {
            let face = element_ref(ElementKind::Face, 1);
            let name = Arc::new(Name::Element {
                feature: feature.clone(),
                made: TypeWord::Operation(OpCode::Sketch),
                element: face,
            });
            builder.set(face, name);
        } else {
            for (n, id) in (1..=faces).zip(regions) {
                let name = Name::Lower {
                    feature: feature.clone(),
                    operation: OpCode::Sketch,
                    kind: ElementKind::Face,
                    first: id_name(ElementKind::Edge, id),
                    more: Vec::new(),
                };
                builder.set(element_ref(ElementKind::Face, n), Arc::new(name));
            }
        }
        builder.by_number(history);
        builder.settle(&[]);
        builder.map
    }

    /// The map of a copy of `of`'s solid, placed at `instance` by `feature`'s `operation`, a
    /// pattern or mirror (N10): each element numbered as `of` numbers it (a copy's elements
    /// are the tool's: the worker numbers copy k's as the tool's, `Op::Copies`), each of its
    /// names wrapped once, `feature.type.Kind(C<instance> <name>)` for a feature's tool, or
    /// `(S<instance> <name>)` for the base solid, as `copied` says; the step's
    /// [`ElementMap::operation`] then names its result from it as from any input. A copy of
    /// an unstable name is unstable.
    ///
    /// None where no step makes such a copy, as the parser refuses its names: an operation
    /// that copies nothing; an instance of more offsets than the operation has directions;
    /// a tool's copy (`C`) of a map that isn't a sweep step's, the only tool a pattern
    /// copies; a solid's (`S`) of a sketch's faces, which are in no solid.
    ///
    /// A tool's copy takes the original's sweep step's map ([`ElementMap::swept`]), whose
    /// every element is the sweep's own: its last step's map also holds its base's names,
    /// which aren't the tool's (on another sweep feature, of a sweep's type word, so no
    /// rule of a name's can tell them from the tool's). Each name is held to the parser's
    /// rule too ([`Copied::wraps`]), so the map never holds a name a reference to it can't
    /// store: a tool's copy leaves out a name that isn't a sweep's, and a solid's copy a
    /// sketch's. A copied name past the bounds ([`MOST_DEPTH`], [`MOST_WEIGHT`]) is left
    /// out too, never numbered here: by the tool's number every copy of the element would
    /// share one name. An element left with none is the step's to name by its number in
    /// its result, and so is every element made from it (N10, settled by M6.3).
    pub fn copy(
        of: &ElementMap,
        feature: FeatureId,
        operation: OpCode,
        copied: Copied,
        instance: &Instance,
    ) -> Option<ElementMap> {
        let directions = operation.directions()?;
        if instance.offsets().len() > directions {
            return None;
        }
        if !copied.wraps(of.made()) {
            return None;
        }
        let mut builder = Builder::new(feature, operation, Stage::Last);
        for (element, names) in &of.names {
            for name in names.iter().filter(|name| copied.wraps(name.type_word())) {
                let copy = Name::Copy {
                    feature: builder.map.feature.clone(),
                    operation,
                    kind: element.kind,
                    copied,
                    instance: instance.clone(),
                    of: Arc::clone(name),
                };
                let (depth, weight) = builder.parts_size(&copy);
                if depth <= MOST_DEPTH && weight <= MOST_WEIGHT {
                    builder.set(*element, Arc::new(copy));
                }
            }
        }
        builder.settle(&[of]);
        Some(builder.map)
    }
}

/// FreeCAD's `NameKey`, which orders an element's sources: the source's kind (vertex, edge,
/// face), its input (FreeCAD's tag, the input object's ID, so the base before the tool in
/// FreeCAD's tests and documents alike), and its name as FreeCAD writes it, byte by byte;
/// then Silico's name, so the order is total.
type SourceKey = (u8, usize, Arc<str>, Arc<Name>);

struct Builder {
    map: ElementMap,
    operation: OpCode,
    /// Which of the feature's steps it names, for what it keeps of each input
    /// ([`OpCode::side`]).
    stage: Stage,
    /// Each name measured so far, by its address (the name held, so the address is never
    /// another's): how deep it nests and how many names it's made of.
    sizes: BTreeMap<usize, (Arc<Name>, u32, u64)>,
    /// The input each element named so far came from, as FreeCAD's tag on its name says: a
    /// kept element its input's, a derived one its first source's, one from above or below
    /// its upper's or first lower's. Two names FreeCAD's comparator can't tell apart
    /// without their tags (Silico's form has none: the base's `Face1` and the tool's) are
    /// ordered by it.
    inputs: BTreeMap<ElementRef, usize>,
    /// Names two or more of this result's elements came about by (each but the first then
    /// `~1`, `~2`, ...).
    duplicated: BTreeSet<Arc<Name>>,
    /// Names by number made here: an element nothing else named, one whose name would
    /// have been past the bounds, or an input's element its map doesn't have.
    numbered: BTreeSet<Arc<Name>>,
}

impl Builder {
    fn new(feature: FeatureId, operation: OpCode, stage: Stage) -> Self {
        Builder {
            map: ElementMap {
                feature,
                made: TypeWord::Operation(operation),
                names: BTreeMap::new(),
                elements: BTreeMap::new(),
                unstable: BTreeSet::new(),
            },
            operation,
            stage,
            sizes: BTreeMap::new(),
            inputs: BTreeMap::new(),
            duplicated: BTreeSet::new(),
            numbered: BTreeSet::new(),
        }
    }

    /// What this step keeps of input `input`.
    fn side(&self, input: usize) -> Side {
        self.operation.side(self.stage, input)
    }

    /// How deep `name` nests and how many names it's made of, written out.
    fn size(&mut self, name: &Arc<Name>) -> (u32, u64) {
        let address = Arc::as_ptr(name) as usize;
        if let Some((_, depth, weight)) = self.sizes.get(&address) {
            return (*depth, *weight);
        }
        let (depth, weight) = self.parts_size(name);
        self.sizes
            .insert(address, (Arc::clone(name), depth, weight));
        (depth, weight)
    }

    /// How deep a name made of `name`'s parts nests, and how many names it's made of.
    fn parts_size(&mut self, name: &Name) -> (u32, u64) {
        let (mut depth, mut weight) = (0u32, 1u64);
        for part in name.parts() {
            let (d, w) = self.size(part);
            depth = depth.max(d);
            weight = weight.saturating_add(w);
        }
        (depth.saturating_add(1), weight)
    }

    /// `name` for `element`, or, if it would be past the bounds, the element's number.
    fn bounded(&mut self, element: ElementRef, name: Name) -> Arc<Name> {
        let (depth, weight) = self.parts_size(&name);
        if depth > MOST_DEPTH || weight > MOST_WEIGHT {
            return self.numbered(&self.map.feature.clone(), self.map.made, element);
        }
        Arc::new(name)
    }

    /// `feature`'s `element` by its number, `feature` of type `made`, unstable.
    fn numbered(&mut self, feature: &FeatureId, made: TypeWord, element: ElementRef) -> Arc<Name> {
        let name = Arc::new(Name::Element {
            feature: feature.clone(),
            made,
            element,
        });
        self.numbered.insert(Arc::clone(&name));
        name
    }

    /// Whether input `input`, of map `map`, is a copy this step places ([`ElementMap::copy`]):
    /// a pattern's or mirror's step's every input after its base (input 0, its base's map or
    /// the step before's), each of this feature's own. A sweep feature's sweep, of its own
    /// feature and type too, is no copy: its step copies nothing.
    fn is_copy(&self, map: &ElementMap, input: usize) -> bool {
        input > 0 && self.operation.directions().is_some() && map.feature() == &self.map.feature
    }

    /// An input element's own name; if its map has none (a history that lists more of an
    /// input's elements than it has; every map Silico builds names every element), one by
    /// its number, unstable.
    fn source_name(&mut self, map: &ElementMap, source: ElementRef) -> Arc<Name> {
        map.name(source)
            .unwrap_or_else(|| self.numbered(map.feature(), map.made(), source))
    }

    /// Decides which of the map's names are unstable: a duplicate's `~n`, a name that came
    /// about for more than one element, one by number, one its input's map says is
    /// unstable (a kept element keeps it), or one made from such a name.
    fn settle(&mut self, inputs: &[&ElementMap]) {
        let mut decided: BTreeMap<usize, (Arc<Name>, bool)> = BTreeMap::new();
        let held: Vec<Arc<Name>> = self.map.elements.keys().cloned().collect();
        let unstable = held
            .into_iter()
            .filter(|name| self.decide(name, inputs, &mut decided))
            .collect();
        self.map.unstable = unstable;
    }

    /// Whether `name` is unstable, as [`settle`](Self::settle) decides it, each name (by
    /// its address, held so the address is never another's) decided once.
    fn decide(
        &self,
        name: &Arc<Name>,
        inputs: &[&ElementMap],
        decided: &mut BTreeMap<usize, (Arc<Name>, bool)>,
    ) -> bool {
        let address = Arc::as_ptr(name) as usize;
        if let Some((_, unstable)) = decided.get(&address) {
            return *unstable;
        }
        let unstable = matches!(**name, Name::Duplicate { .. })
            || self.duplicated.contains(name)
            || self.numbered.contains(name)
            || inputs.iter().any(|input| input.unstable.contains(name))
            || name
                .parts()
                .into_iter()
                .any(|part| self.decide(part, inputs, decided));
        decided.insert(address, (Arc::clone(name), unstable));
        unstable
    }

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
        let mut candidate = name;
        let mut ordinal = NonZeroU32::MIN;
        loop {
            match self.map.elements.entry(candidate) {
                std::collections::btree_map::Entry::Vacant(free) => {
                    let candidate = Arc::clone(free.key());
                    free.insert(element);
                    self.map.names.entry(element).or_default().push(candidate);
                    return;
                }
                std::collections::btree_map::Entry::Occupied(taken) => {
                    if *taken.get() == element {
                        return;
                    }
                    let base = match &**taken.key() {
                        Name::Duplicate { name, .. } => Arc::clone(name),
                        _ => Arc::clone(taken.key()),
                    };
                    self.duplicated.insert(Arc::clone(&base));
                    candidate = Arc::new(Name::Duplicate {
                        name: base,
                        ordinal,
                    });
                    ordinal = ordinal.saturating_add(1);
                }
            }
        }
    }

    /// The input `element` came from, for ordering: none (last) if it isn't known.
    fn input_of(&self, element: ElementRef) -> usize {
        self.inputs.get(&element).copied().unwrap_or(usize::MAX)
    }

    /// `element`'s own name, as a candidate to name another from.
    fn candidate(&self, element: ElementRef) -> Option<Candidate> {
        let name = self.primary(element)?;
        Some(Candidate {
            text: freecad::text(&name).into(),
            input: self.input_of(element),
            name,
        })
    }

    /// Step 1: each input element the result has unchanged keeps its names, where the
    /// operation bites its input; where it keeps its inside, it's the feature's own piece,
    /// `I` (N9); where it keeps neither side (a sweep's profile), the feature's own, one `M`.
    fn kept(&mut self, inputs: &[&ElementMap], history: &History) {
        for (i, (input, map)) in history.inputs.iter().zip(inputs).enumerate() {
            for kind in KINDS {
                for (n, fate) in fates(input, kind).iter().enumerate() {
                    let Some(kept) = fate.kept else { continue };
                    let source = element(kind, n);
                    let kept = element_ref(kind, kept);
                    self.inputs.entry(kept).or_insert(i);
                    let names: Vec<Arc<Name>> = match map.names.get(&source) {
                        Some(names) => names.clone(),
                        // A copy's element its map leaves unnamed, its every name past the
                        // bounds: by its number in this result, never the tool's, which
                        // every copy of it shares (N10).
                        None if self.is_copy(map, i) => {
                            vec![self.numbered(&self.map.feature.clone(), self.map.made, kept)]
                        }
                        None => vec![self.numbered(map.feature(), map.made(), source)],
                    };
                    for name in names {
                        // Kept whole where the operation keeps what of the input lies
                        // inside its other operand: the feature's own piece (N9); of a
                        // profile, the feature's own (S4).
                        let name = match self.side(i) {
                            Side::Outside => name,
                            Side::Inside => self.own(kept, name, Relation::Inside),
                            Side::Neither => self.own(kept, name, Relation::Modified),
                        };
                        self.set(kept, name);
                    }
                }
            }
        }
    }

    /// `name`'s element kept whole as the feature's own: inside the other operand, its
    /// piece, `I`; of a profile, `M`. With no OCCT position (FreeCAD's form writes it as the
    /// source, as FreeCAD's `mapSubElement` does).
    fn own(&mut self, element: ElementRef, name: Arc<Name>, relation: Relation) -> Arc<Name> {
        let name = Name::Derived {
            feature: self.map.feature.clone(),
            operation: self.operation,
            kind: element.kind,
            first: Source {
                relation,
                place: None,
                position: Raw(None),
                name,
            },
            more: Vec::new(),
        };
        self.bounded(element, name)
    }

    /// Step 2: each element OCCT reports modified or generated, still unnamed, from its
    /// sources.
    fn derived(&mut self, inputs: &[&ElementMap], history: &History) {
        // Each result element's sources. (FreeCAD names elements in its `IndexedName` order,
        // edges, faces, vertices; two elements can come about alike only if they're of one
        // kind, so element order decides the same `~1`.)
        // Each source's relation and position, and whether it became this element alone (one
        // element modified, none generated of its kind).
        let mut sources: BTreeMap<ElementRef, BTreeMap<SourceKey, (Relation, NonZeroU32, bool)>> =
            BTreeMap::new();
        // The elements made from a copy's element its map leaves unnamed (every name of it
        // past the bounds): each named by its number in this result, never from that
        // element by the tool's number, which every copy of it shares (N10).
        let mut past: BTreeSet<ElementRef> = BTreeSet::new();
        for kind in KINDS {
            for (i, (input, map)) in history.inputs.iter().zip(inputs).enumerate() {
                for (n, fate) in fates(input, kind).iter().enumerate() {
                    if fate.modified.is_empty() && fate.generated.is_empty() {
                        continue;
                    }
                    let source = element(kind, n);
                    if self.is_copy(map, i) && map.name(source).is_none() {
                        past.extend(
                            fate.modified
                                .iter()
                                .chain(&fate.generated)
                                .map(|r| r.element)
                                .filter(|e| e.kind != ElementKind::Solid && !self.is_named(*e)),
                        );
                        continue;
                    }
                    let name = self.source_name(map, source);
                    let key: SourceKey = (rank(kind), i, freecad::text(&name).into(), name);
                    let sole = fate.modified.len() == 1
                        && fate.generated.iter().all(|r| r.element.kind != kind);
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
                            .entry(r.element)
                            .or_default()
                            .insert(key.clone(), (relation, position, sole));
                    }
                }
            }
        }
        for element in past {
            sources.remove(&element);
            let name = self.numbered(&self.map.feature.clone(), self.map.made, element);
            self.set(element, name);
        }
        // Each element's name without places, and the input it came from.
        let mut named: Vec<(ElementRef, usize, Name)> = Vec::new();
        for (element, keyed) in sources {
            if self.is_named(element) {
                continue;
            }
            let first_input = keyed.keys().next().map_or(usize::MAX, |key| key.1);
            // One source, which became this element and nothing else of its kind, of an
            // input whose outside the operation keeps (a profile's never collapses: S4).
            let alone = keyed.len() == 1
                && keyed.values().all(|&(_, _, sole)| sole)
                && self.side(first_input) == Side::Outside;
            let (operation, stage) = (self.operation, self.stage);
            let mut sources = keyed.into_iter().take(MOST_SOURCES).map(
                |((_, input, _, name), (relation, position, _))| Source {
                    // Modified where the operation keeps what lies inside its other
                    // operand is the feature's own piece, `I` (N9).
                    relation: match relation {
                        Relation::Modified if operation.side(stage, input) == Side::Inside => {
                            Relation::Inside
                        }
                        relation => relation,
                    },
                    place: None,
                    position: Raw(Some(position)),
                    name,
                },
            );
            let Some(first) = sources.next() else {
                continue;
            };
            let name = if alone
                && first.relation == Relation::Modified
                && first.position.0 == Some(NonZeroU32::MIN)
            {
                // Modified from one element alone, its only result (a face split in two
                // keeps its level, so a reference to the face isn't taken to one piece),
                // a sole `M` at OCCT's first: that element's own name, however many features
                // in a row did so (Trevin's decision). What lies inside the other operand is
                // never an `M` (N9: an `I`, the feature's own), so it never collapses.
                let origin = match &*first.name {
                    Name::Modified { name, .. } => Arc::clone(name),
                    _ => first.name,
                };
                Name::Modified {
                    name: origin,
                    last: LastOp(self.operation),
                }
            } else {
                Name::Derived {
                    feature: self.map.feature.clone(),
                    operation: self.operation,
                    kind: element.kind,
                    first,
                    more: sources.collect(),
                }
            };
            named.push((element, first_input, name));
        }
        places(&mut named);
        for (element, first_input, name) in named {
            let name = self.bounded(element, name);
            self.inputs.entry(element).or_insert(first_input);
            self.set(element, name);
        }
    }

    /// Step 3: each unnamed edge by every named face it bounds, then each unnamed vertex by
    /// every named edge it ends (Trevin's, 2026-10-17): the first by FreeCAD's comparator,
    /// then up to four more. Where elements of a kind share their elements above, each is
    /// ranked among them by FreeCAD's count, its place among the first's unnamed elements
    /// in the first's own order, which is also kept, as FreeCAD's `;:U`, for FreeCAD's form.
    fn named_from_above(&mut self, history: &History) {
        let t = &history.topology;
        for (upper_kind, lower_kind, lists) in [
            (ElementKind::Face, ElementKind::Edge, &t.face_edges),
            (ElementKind::Edge, ElementKind::Vertex, &t.edge_vertices),
        ] {
            let mut candidates: BTreeMap<ElementRef, Vec<(Candidate, NonZeroU32)>> =
                BTreeMap::new();
            for (u, lowers) in lists.iter().enumerate() {
                let Some(upper) = self.candidate(element(upper_kind, u)) else {
                    continue;
                };
                let mut nth = NonZeroU32::MIN;
                for &lower in lowers {
                    let lower = element_ref(lower_kind, lower);
                    if self.is_named(lower) {
                        continue;
                    }
                    candidates
                        .entry(lower)
                        .or_default()
                        .push((upper.clone(), nth));
                    nth = nth.saturating_add(1);
                }
            }
            let mut named: Vec<(ElementRef, usize, Name)> = Vec::new();
            for (lower, mut uppers) in candidates {
                uppers.sort_by(|a, b| a.0.cmp(&b.0));
                uppers.dedup_by(|a, b| a.0.name == b.0.name);
                let mut uppers = uppers.into_iter().take(MOST_SOURCES);
                let Some((first, nth)) = uppers.next() else {
                    continue;
                };
                named.push((
                    lower,
                    first.input,
                    Name::Upper {
                        feature: self.map.feature.clone(),
                        operation: self.operation,
                        kind: lower_kind,
                        upper: first.name,
                        more: uppers.map(|(c, _)| c.name).collect(),
                        place: None,
                        nth: Raw(nth),
                    },
                ));
            }
            upper_places(&mut named);
            for (lower, input, name) in named {
                let name = self.bounded(lower, name);
                self.inputs.entry(lower).or_insert(input);
                self.set(lower, name);
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
                let named: Option<Vec<Candidate>> = lowers
                    .iter()
                    .map(|&l| self.candidate(element_ref(lower_kind, l)))
                    .collect();
                let Some(mut named) = named else { continue };
                named.sort();
                named.dedup_by(|a, b| a.name == b.name);
                let input = named.first().map_or(usize::MAX, |first| first.input);
                let mut lowers = named.into_iter().take(MOST_SOURCES).map(|c| c.name);
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
                let name = self.bounded(this, name);
                self.inputs.entry(this).or_insert(input);
                self.set(this, name);
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
                    let name = self.numbered(&self.map.feature.clone(), self.map.made, this);
                    self.set(this, name);
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

/// A source's rank in FreeCAD's `NameKey`: vertex, edge, face.
fn rank(kind: ElementKind) -> u8 {
    match kind {
        ElementKind::Vertex => 0,
        ElementKind::Edge => 1,
        ElementKind::Face => 2,
        ElementKind::Solid => 3,
    }
}

/// A name another element might be named from, ordered as FreeCAD orders such names: by its
/// comparator on FreeCAD's form, then, as FreeCAD's tags would, by input, then by Silico's
/// order, so the order is total.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    /// Written once, and shared by every element it's a candidate for (the review found a
    /// name's text copied for each, gigabytes from a megabyte of history).
    text: Arc<str>,
    input: usize,
    name: Arc<Name>,
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        freecad::compare(&self.text, &other.text)
            .then_with(|| self.input.cmp(&other.input))
            .then_with(|| self.name.cmp(&other.name))
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Elements that came about alike (Trevin's decision, 2026-10-16): each member of a group of
/// `m` whose names, without places, are one name gets, on each source, its rank among them
/// in that source's OCCT list (`G1/2`, `G2/2`), as FreeCAD tells them apart by OCCT's
/// positions, but only where they need telling apart: OCCT reorders its lists for reasons of
/// its own, and a name that doesn't hold a position doesn't change with them.
fn places(named: &mut [(ElementRef, usize, Name)]) {
    for (members, size) in collisions(named, |name| matches!(name, Name::Derived { .. })) {
        let count = members
            .first()
            .and_then(|&i| named.get(i))
            .map_or(0, |(_, _, name)| sources(name).len());
        for s in 0..count {
            // The members by their positions in this source's list (then element order).
            let order = members
                .iter()
                .filter_map(|&i| {
                    let (element, _, name) = named.get(i)?;
                    let source = sources(name).get(s).copied()?;
                    Some((source.position.0.unwrap_or(NonZeroU32::MIN), *element, i))
                })
                .collect();
            for (i, rank) in ranked(order) {
                if let Some((_, _, name)) = named.get_mut(i)
                    && let Some(source) = sources_mut(name).into_iter().nth(s)
                {
                    source.place = Some(Place { rank, size });
                }
            }
        }
    }
}

/// Each name in `named` (elements named from above) that another shares, written the same
/// but for its place, given its rank among them, by FreeCAD's count in its first element
/// above, and how many there are.
fn upper_places(named: &mut [(ElementRef, usize, Name)]) {
    // Every name here is from above.
    for (members, size) in collisions(named, |_| true) {
        let order = members
            .iter()
            .filter_map(|&i| match named.get(i)? {
                (element, _, Name::Upper { nth, .. }) => Some((nth.0, *element, i)),
                _ => None,
            })
            .collect();
        for (i, rank) in ranked(order) {
            if let Some((_, _, Name::Upper { place, .. })) = named.get_mut(i) {
                *place = Some(Place { rank, size });
            }
        }
    }
}

/// The names in `named` that `which` takes and another shares: each group's members, by
/// their index in `named`, and how many there are.
fn collisions(
    named: &[(ElementRef, usize, Name)],
    which: impl Fn(&Name) -> bool,
) -> Vec<(Vec<usize>, NonZeroU32)> {
    let mut groups: BTreeMap<&Name, Vec<usize>> = BTreeMap::new();
    for (i, (_, _, name)) in named.iter().enumerate() {
        if which(name) {
            groups.entry(name).or_default().push(i);
        }
    }
    groups
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|members| {
            let size = NonZeroU32::new(u32::try_from(members.len()).unwrap_or(u32::MAX))
                .unwrap_or(NonZeroU32::MIN);
            (members, size)
        })
        .collect()
}

/// Each member of a group, `(key, element, index)`, with its rank by its key: one more
/// than the members before it. Members at one key (a history from a broken worker) share a
/// rank, and so the name, and are told apart as duplicates, never by their numbers.
fn ranked(mut order: Vec<(NonZeroU32, ElementRef, usize)>) -> Vec<(usize, NonZeroU32)> {
    order.sort();
    let keys: Vec<NonZeroU32> = order.iter().map(|&(key, _, _)| key).collect();
    order
        .into_iter()
        .map(|(key, _, i)| {
            let before = keys.partition_point(|&k| k < key);
            let rank = NonZeroU32::new(u32::try_from(before + 1).unwrap_or(u32::MAX))
                .unwrap_or(NonZeroU32::MIN);
            (i, rank)
        })
        .collect()
}

/// A derived name's sources, the first first.
fn sources(name: &Name) -> Vec<&Source> {
    match name {
        Name::Derived { first, more, .. } => std::iter::once(first).chain(more).collect(),
        _ => Vec::new(),
    }
}

fn sources_mut(name: &mut Name) -> Vec<&mut Source> {
    match name {
        Name::Derived { first, more, .. } => {
            std::iter::once(first).chain(more.iter_mut()).collect()
        }
        _ => Vec::new(),
    }
}
