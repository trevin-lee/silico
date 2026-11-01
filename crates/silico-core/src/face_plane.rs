//! A sketch on a planar face, placed (docs/ports/attacher.md, *How a rebuild places it
//! (M5.9)*; F1 and F2): its face found by name in the solids live at the sketch's place, as
//! a fillet's edge is found in its base (M2.8), and its frame worked out from the face's
//! plane, the stored axis, and `reversed`. A name that finds nothing, or finds an element by
//! an unstable name, a face OCCT doesn't type as a plane, and an axis parallel to the face's
//! normal are each `reference_broken` on the sketch, with candidates, never re-pointed. A
//! live solid that failed, or was blocked, blocks the sketch where the face isn't found in
//! another; one not built yet leaves it to the next round of the rebuild.

use silico_protocol::{FaceFrame, FaceFrameError, MIN_AXIS_OFF_NORMAL, Surface};

use crate::document::{AxisToken, FacePlane, FeatureId};
use crate::kernel::{Description, ElementKind};
use crate::naming::{ElementMap, Maker, Name};
use crate::references::{CANDIDATES, Candidates, cut_short, refused, retyped, unstable};
use crate::{Error, ErrorCode};

/// The frame of a sketch on the plane of unit normal `normal` through `location` (a point
/// on it), its x following `x` and flipped if `reversed` (F1, F2), as the protocol's
/// [`FaceFrame::from_normal`] makes it: the origin o = n(n·p); x the token's axis projected
/// onto the plane (twice, so the frame is square however near the normal the axis lies);
/// y = normalize(n × x); every zero +0. A `reversed` sketch's is the frame of −n and −a:
/// the same origin and y, x negated, so its normal, x × y, is negated too. Refused where
/// the axis lies along the normal, or the frame made is one no sketch can be placed on.
pub(crate) fn frame(
    normal: [f64; 3],
    location: [f64; 3],
    x: AxisToken,
    reversed: bool,
) -> Result<FaceFrame, FaceFrameError> {
    let sign = if reversed { -1.0 } else { 1.0 };
    FaceFrame::from_normal(
        location,
        normal.map(|c| c * sign),
        x.axis().map(|c| c * sign),
    )
}

/// The tokens whose axes leave a face of normal `normal` a direction ([`frame`] takes
/// them), in their order: a parallel axis's candidates.
pub(crate) fn working_tokens(normal: [f64; 3]) -> Vec<&'static str> {
    AxisToken::ALL
        .into_iter()
        .filter(|token| {
            !matches!(
                frame(normal, [0.0; 3], *token, false),
                Err(FaceFrameError::AxisAlongNormal(_))
            )
        })
        .map(AxisToken::word)
        .collect()
}

/// A solid live at a sketch's place in the timeline, as the round of the rebuild that
/// resolves the sketch's face has it.
pub(crate) enum Live<'a> {
    /// Built: its element map, and its description, or why it has none.
    Built {
        map: &'a ElementMap,
        description: Result<&'a Description, Error>,
    },
    /// Its feature failed, or was blocked: the feature whose failure is at the root.
    Failed(FeatureId),
    /// Not built yet, its feature named: it waits on another sketch's face.
    Waits(FeatureId),
}

/// What a round of the rebuild finds of a sketch's face ([`place`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Resolved {
    /// Placed, or why it can't be.
    Placed(Placed),
    /// Not found yet: a solid that could hold it isn't built yet, the feature named.
    Waits(FeatureId),
}

/// What became of a sketch's face.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Placed {
    /// Found, and the sketch's frame worked out from it.
    Frame(FaceFrame),
    /// The face's reference is broken, or its axis is parallel to it: a `reference_broken`
    /// error on the sketch, with candidates.
    Broken(Error),
    /// A live solid it might be in failed or was blocked: blocked on that failure's root.
    Blocked(FeatureId),
    /// Its solid couldn't be described (a worker's failure, or the kernel's refusal of its
    /// values): that, on the sketch.
    Failed(Error),
}

/// Where the sketch `sketch`'s face, `plane`, is among the solids live at its place, and
/// its frame. `related` are those built from one of the face's name's makers, in timeline
/// order: only they can hold it (a name's element is in no solid but those made from its
/// feature), so only they are searched, waited for while one isn't built, or block the
/// sketch where one failed; `others`, the rest of the live solids built, are read only for
/// a broken name's candidates, so they're asked for (and named) only then, and the refusal
/// waits until each has settled. `makers` says what each feature a name mentions is, as the
/// gate read the name.
pub(crate) fn place<'a>(
    sketch: &FeatureId,
    plane: &FacePlane,
    related: &[Live<'a>],
    others: &dyn Fn() -> Vec<Live<'a>>,
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Resolved {
    match placed(sketch, plane, related, others, makers) {
        Ok(placed) => Resolved::Placed(placed),
        Err(on) => Resolved::Waits(on),
    }
}

/// [`place`]'s finding: placed, or the feature whose solid it waits for.
fn placed<'a>(
    sketch: &FeatureId,
    plane: &FacePlane,
    related: &[Live<'a>],
    others: &dyn Fn() -> Vec<Live<'a>>,
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Result<Placed, FeatureId> {
    let text = plane.face.as_str();
    let broken = |why: &str, candidates: Vec<String>| {
        Placed::Broken(
            refused("plane.face", text, why, ElementKind::Face, candidates)
                .with_feature(sketch.as_str()),
        )
    };
    let name = match Name::parse(text, makers) {
        Ok(name) => name,
        // The gate refuses a face that isn't a name: not reached from a checked document.
        Err(error) => return Ok(broken(&format!("isn't a name: {error}"), Vec::new())),
    };
    for (map, description) in built(related) {
        let Some(element) = map.element(&name) else {
            continue;
        };
        if map.is_unstable(&name) {
            return Ok(broken(&unstable(element.kind), Vec::new()));
        }
        if element.kind != ElementKind::Face {
            return Ok(broken(
                &format!("names {}, not a face", element.kind.a_word()),
                Vec::new(),
            ));
        }
        let description = match description {
            Ok(description) => *description,
            Err(error) => return Ok(Placed::Failed(error.clone().with_feature(sketch.as_str()))),
        };
        let face = element
            .index
            .checked_sub(1)
            .and_then(|i| description.faces.get(i as usize));
        let Some(face) = face else {
            return Ok(Placed::Failed(
                Error::new(
                    ErrorCode::Internal,
                    "a face's name found it in a solid whose description hasn't it",
                )
                .with_feature(sketch.as_str())
                .with_elements([cut_short(text)])
                .with_suggestion(crate::worker::SILICO_BUG),
            ));
        };
        let Surface::Plane { normal, location } = face.surface else {
            // Its candidates are read from every live solid, so the refusal waits until
            // each has settled: the same candidates, whichever round it comes in.
            let others = others();
            if let Some(on) = waiting(&others) {
                return Err(on);
            }
            let what = crate::elements::surface_type(&face.surface).word();
            return Ok(broken(
                &format!(
                    "names a face that isn't planar: OCCT types its surface a {what}, and a sketch sits only on a face typed a plane"
                ),
                planar_faces(&name, related, &others),
            ));
        };
        // A normal no plane has (not finite, or of no length): the kernel's describe gave it
        // badly, a fault in Silico, not the document's.
        if !normal.iter().all(|c| c.is_finite()) || silico_protocol::vec3::norm(normal) == 0.0 {
            return Ok(Placed::Failed(
                Error::new(
                    ErrorCode::Internal,
                    format!("the kernel described the sketch's face's normal as {normal:?}, which no plane has"),
                )
                .with_feature(sketch.as_str())
                .with_elements([cut_short(text)])
                .with_suggestion(crate::worker::SILICO_BUG),
            ));
        }
        return Ok(match frame(normal, location, plane.x, plane.reversed) {
            Ok(frame) => Placed::Frame(frame),
            Err(FaceFrameError::AxisAlongNormal(off)) => {
                Placed::Broken(parallel(sketch, plane, normal, off))
            }
            // A frame no sketch can be placed on (its origin past the reach, as a slanted
            // face far out can give): the sketch's, in the document's terms.
            Err(FaceFrameError::Invalid(invalid)) => Placed::Failed(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "the sketch's face's frame can't place a sketch: {}",
                        invalid.message
                    ),
                )
                .with_feature(sketch.as_str())
                .with_parameter("plane")
                .with_suggestion(
                    "put the sketch on a face nearer the model's origin: a face's frame is placed from the origin projected onto its plane",
                ),
            ),
        });
    }
    // Not in any related solid built: one not built yet may hold it, or one that failed.
    if let Some(on) = waiting(related) {
        return Err(on);
    }
    if let Some(root) = related.iter().find_map(|solid| match solid {
        Live::Failed(root) => Some(root.clone()),
        Live::Built { .. } | Live::Waits(_) => None,
    }) {
        return Ok(Placed::Blocked(root));
    }
    // Refused once every live solid has settled, so the candidates, read from them all,
    // are the same whichever round the refusal comes in (docs/ports/attacher.md, *In
    // rounds*).
    let others = others();
    if let Some(on) = waiting(&others) {
        return Err(on);
    }
    let maps: Vec<&ElementMap> = built(related)
        .into_iter()
        .chain(built(&others))
        .map(|(map, _)| map)
        .collect();
    let why = match retyped(&name, makers) {
        Some((feature, said, made)) => format!(
            "names an element a {said} made, but {feature} is a {made} now: its elements are another's"
        ),
        None => "names no face of the solids live at the sketch's place: it isn't one of the \
                 faces they're made from, or an edit before this sketch removed or remade it"
            .to_owned(),
    };
    Ok(broken(
        &why,
        Candidates::of(&maps, ElementKind::Face).of_name(&name),
    ))
}

/// The feature of the first of `solids` not built yet, if one isn't.
fn waiting(solids: &[Live<'_>]) -> Option<FeatureId> {
    solids.iter().find_map(|solid| match solid {
        Live::Waits(on) => Some(on.clone()),
        Live::Built { .. } | Live::Failed(_) => None,
    })
}

/// The solids built of `solids`: each one's map and description.
fn built<'s, 'a>(
    solids: &'s [Live<'a>],
) -> Vec<(&'a ElementMap, &'s Result<&'a Description, Error>)> {
    solids
        .iter()
        .filter_map(|solid| match solid {
            Live::Built { map, description } => Some((*map, description)),
            Live::Failed(_) | Live::Waits(_) => None,
        })
        .collect()
}

/// The refusal of the sketch `sketch` on a face whose profile's `faces`, placed on its
/// frame, reach past [`MAX_LENGTH`](silico_protocol::MAX_LENGTH) of the model's origin in
/// any component, as the faces request sends them (each line's ends, each arc's centre and
/// ends, each circle's centre), so no later request is refused naming a value the document
/// never wrote: `invalid_input` naming the sketch and `plane`. None where each is within it.
pub(crate) fn beyond_reach(
    sketch: &FeatureId,
    faces: &silico_protocol::FaceRegions,
) -> Option<Error> {
    use silico_protocol::Curve;
    let plane = faces.plane;
    faces.curves().find_map(|curve| {
        let points: &[[f64; 2]] = match curve {
            Curve::Line { start, end } => &[*start, *end],
            Curve::Arc {
                center, start, end, ..
            } => &[*center, *start, *end],
            Curve::Circle { center, .. } => &[*center],
        };
        points.iter().find_map(|&point| {
            let at = plane.place(point);
            let within = at
                .iter()
                .all(|c| c.is_finite() && c.abs() <= silico_protocol::MAX_LENGTH);
            (!within).then(|| {
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "the sketch's point {point:?}, placed on its face's frame, is at {at:?}: past 1e6 mm (a kilometre) of the model's origin, as far as a sketch's points may be"
                    ),
                )
                .with_feature(sketch.as_str())
                .with_parameter("plane")
                .with_suggestion(
                    "draw the sketch nearer its face's origin, or put it on a face nearer the model's origin",
                )
            })
        })
    })
}

/// The refusal of a sketch whose axis `plane.x` leaves its face of normal `normal` only
/// `off` (under [`MIN_AXIS_OFF_NORMAL`]): `reference_broken` on `plane.x`, the tokens that work the
/// candidates.
fn parallel(sketch: &FeatureId, plane: &FacePlane, normal: [f64; 3], off: f64) -> Error {
    let word = plane.x.word();
    Error::new(
        ErrorCode::ReferenceBroken,
        format!(
            "plane.x, \"{word}\", is parallel to the face's normal {normal:?}: projected onto the face it leaves {}, under the {} a sketch's x needs",
            silico_protocol::format_number(off),
            silico_protocol::format_number(MIN_AXIS_OFF_NORMAL)
        ),
    )
    .with_feature(sketch.as_str())
    .with_parameter("plane.x")
    .with_element(word)
    .with_candidates(working_tokens(normal))
    .with_suggestion(
        "give plane.x one of the candidates, an axis that isn't the face's normal: none is taken for it",
    )
}

/// The planar faces of the live solids built, `related` and `others`, by their stable
/// names, as a face that isn't planar's candidates: those of `name`'s own feature first,
/// then by name; at most [`CANDIDATES`].
fn planar_faces(name: &Name, related: &[Live<'_>], others: &[Live<'_>]) -> Vec<String> {
    let mut found: Vec<(bool, String)> = Vec::new();
    for solid in related.iter().chain(others) {
        let Live::Built {
            map,
            description: Ok(description),
            ..
        } = solid
        else {
            continue;
        };
        for (number, face) in (1..).zip(&description.faces) {
            if !matches!(face.surface, Surface::Plane { .. }) {
                continue;
            }
            let element = crate::kernel::ElementRef {
                kind: ElementKind::Face,
                index: number,
            };
            if let Some(stable) = crate::elements::stable_name(map, element) {
                found.push((stable.feature() != name.feature(), stable.to_string()));
            }
        }
    }
    found.sort();
    found.dedup();
    found
        .into_iter()
        .take(CANDIDATES)
        .map(|(_, text)| text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use silico_protocol::{SketchPlane, vec3};

    /// A frame's normal, x × y.
    fn normal(f: &FaceFrame) -> [f64; 3] {
        SketchPlane::Face(*f).normal()
    }
    use crate::document::ElementName;
    use crate::kernel::{FaceDescription, Primitive};
    use crate::naming::PrimitiveKind;
    use AxisToken::{MinusX, MinusY, MinusZ, PlusX, PlusY};

    /// A sketch's face plane on the face named `face`.
    fn face_plane(face: &str, x: AxisToken, reversed: bool) -> FacePlane {
        FacePlane {
            face: ElementName::new(face).unwrap(),
            x,
            reversed,
        }
    }

    const SQRT_HALF: f64 = std::f64::consts::FRAC_1_SQRT_2;

    fn id(text: &str) -> FeatureId {
        FeatureId::new(text).unwrap()
    }

    /// A point (u, v) of the sketch, in the model: o + u·x + v·y.
    fn at(f: &FaceFrame, [u, v]: [f64; 2]) -> [f64; 3] {
        std::array::from_fn(|k| f.origin[k] + u * f.x[k] + v * f.y[k])
    }

    #[track_caller]
    fn near(got: [f64; 3], want: [f64; 3], tolerance: f64) {
        for k in 0..3 {
            assert!(
                (got[k] - want[k]).abs() <= tolerance,
                "{got:?}, not {want:?}"
            );
        }
    }

    #[test]
    fn a_boxs_six_faces_take_the_notes_frames_and_origins_at_a_corner_off_the_origin() {
        // The note's table (*Worked frames*), for a box with its corner at p = (2, 3, 5),
        // 10 × 20 × 30: each face's n, its default token, x, y = n × x, and o, the global
        // origin projected onto the face's plane. Each face's location is a corner of it
        // (as OCCT's box places them), any point of the plane giving the same o.
        let (px, py, pz, l, w, h) = (2.0, 3.0, 5.0, 10.0, 20.0, 30.0);
        let faces = [
            (
                [-1.0, 0.0, 0.0],
                [px, py, pz],
                MinusY,
                [0.0, -1.0, 0.0],
                [0.0, 0.0, 1.0],
                [px, 0.0, 0.0],
            ),
            (
                [1.0, 0.0, 0.0],
                [px + l, py + w, pz],
                PlusY,
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [px + l, 0.0, 0.0],
            ),
            (
                [0.0, -1.0, 0.0],
                [px + l, py, pz + h],
                PlusX,
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, py, 0.0],
            ),
            (
                [0.0, 1.0, 0.0],
                [px, py + w, pz],
                MinusX,
                [-1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, py + w, 0.0],
            ),
            (
                [0.0, 0.0, -1.0],
                [px, py, pz],
                PlusX,
                [1.0, 0.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, 0.0, pz],
            ),
            (
                [0.0, 0.0, 1.0],
                [px + l, py + w, pz + h],
                PlusX,
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, pz + h],
            ),
        ];
        for (n, location, token, x, y, o) in faces {
            assert_eq!(AxisToken::default_x(n), token, "{n:?}");
            let f = frame(n, location, token, false).unwrap();
            assert_eq!((f.origin, f.x, f.y, normal(&f)), (o, x, y, n), "{n:?}");
            f.check().unwrap();
            // A point (u, v) lands at o + u·x + v·y, on the face's plane.
            let p = at(&f, [3.0, 4.0]);
            let want: [f64; 3] = std::array::from_fn(|k| o[k] + 3.0 * x[k] + 4.0 * y[k]);
            assert_eq!(p, want);
            assert_eq!(vec3::dot(n, p), vec3::dot(n, location), "{n:?}");
            // Reversed: n and x flip, y stays, o is the same.
            let r = frame(n, location, token, true).unwrap();
            assert_eq!(
                (r.origin, r.x, r.y, normal(&r)),
                (o, x.map(|c| -c + 0.0), y, n.map(|c| -c + 0.0)),
                "{n:?} reversed"
            );
            r.check().unwrap();
        }
    }

    #[test]
    fn the_top_face_reversed_has_its_normal_down_its_x_back_and_its_y_kept() {
        // The note: "Face6 reversed has n = −Z, x = −X, y = +Y".
        let f = frame([0.0, 0.0, 1.0], [0.0, 0.0, 10.0], PlusX, true).unwrap();
        assert_eq!(normal(&f), [0.0, 0.0, -1.0]);
        assert_eq!(f.x, [-1.0, 0.0, 0.0]);
        assert_eq!(f.y, [0.0, 1.0, 0.0]);
        assert_eq!(f.origin, [0.0, 0.0, 10.0]);
    }

    #[test]
    fn every_zero_of_a_frame_is_plus_zero() {
        // A normal and a location with −0s, as `describe` can give them (a pad's face
        // measured −0): (−0, −0, 1) through (−0, −0, 4). Its origin's x and y are −0 × 4
        // and its x's z −0 before the + 0; written + 0, each is +0, so the faces step's
        // key, which holds the bits, is the +0 frame's.
        let f = frame([-0.0, -0.0, 1.0], [-0.0, -0.0, 4.0], PlusX, false).unwrap();
        for v in [f.origin, f.x, f.y, normal(&f)] {
            for c in v {
                assert!(c != 0.0 || c.is_sign_positive(), "{f:?}");
            }
        }
        let reversed = frame([0.0, 0.0, 1.0], [0.0, 0.0, 4.0], PlusY, true).unwrap();
        for v in [reversed.origin, reversed.x, reversed.y, normal(&reversed)] {
            for c in v {
                assert!(c != 0.0 || c.is_sign_positive(), "{reversed:?}");
            }
        }
        assert_eq!(reversed.x, [0.0, -1.0, 0.0]);
    }

    #[test]
    fn a_tie_between_x_and_y_gives_the_same_frame_whichever_axis_wins() {
        // The note's ties: n = (1, 1, 0)/√2, bitwise equal in x and y. X wins (`+y`), x
        // = (−1, 1, 0)/√2; had Y won (`-x`), the same x. y = +Z either way. And n = (1, −1,
        // 0)/√2 (testPartDesignTNPChamfer's Face8): `+y` gives (1, 1, 0)/√2, `+x` the same.
        let n = [SQRT_HALF, SQRT_HALF, 0.0];
        assert_eq!(AxisToken::default_x(n), PlusY);
        for token in [PlusY, MinusX] {
            let f = frame(n, [0.0; 3], token, false).unwrap();
            near(f.x, [-SQRT_HALF, SQRT_HALF, 0.0], 1e-15);
            near(f.y, [0.0, 0.0, 1.0], 1e-15);
            f.check().unwrap();
        }
        let n = [SQRT_HALF, -SQRT_HALF, 0.0];
        assert_eq!(AxisToken::default_x(n), PlusY);
        // Face8's origin: the box's corner at (10, 0), chamfered by 1, puts the face
        // through (9, 0, 0); n·p = 9/√2, so o = (4.5, −4.5, 0).
        for token in [PlusY, PlusX] {
            let f = frame(n, [9.0, 0.0, 0.0], token, false).unwrap();
            near(f.x, [SQRT_HALF, SQRT_HALF, 0.0], 1e-15);
            near(f.y, [0.0, 0.0, 1.0], 1e-15);
            near(f.origin, [4.5, -4.5, 0.0], 1e-14);
        }
    }

    #[test]
    fn ties_with_z_turn_the_frame_with_the_winner_as_the_table_says() {
        // X over Z: n = (1, 0, 1)/√2, `+y`: x = +Y, y = (−1, 0, 1)/√2; had Z won (`+x`),
        // x = (1, 0, −1)/√2, y = +Y. Y over Z: n = (0, 1, 1)/√2, `-x`: x = −X, y = (0, −1,
        // 1)/√2; `+x` gives +X, turned 180°. (0, −1, 1)/√2: `+x`, y = (0, 1, 1)/√2. All
        // three: n = (1, 1, 1)/√3, `+y`: x = (−1, 2, −1)/√6, y = (−1, 0, 1)/√2.
        // Each: the normal, the token, and x and y by hand.
        type Case = ([f64; 3], AxisToken, [f64; 3], [f64; 3]);
        let cases: [Case; 6] = [
            (
                [SQRT_HALF, 0.0, SQRT_HALF],
                PlusY,
                [0.0, 1.0, 0.0],
                [-SQRT_HALF, 0.0, SQRT_HALF],
            ),
            (
                [SQRT_HALF, 0.0, SQRT_HALF],
                PlusX,
                [SQRT_HALF, 0.0, -SQRT_HALF],
                [0.0, 1.0, 0.0],
            ),
            (
                [0.0, SQRT_HALF, SQRT_HALF],
                MinusX,
                [-1.0, 0.0, 0.0],
                [0.0, -SQRT_HALF, SQRT_HALF],
            ),
            (
                [0.0, SQRT_HALF, SQRT_HALF],
                PlusX,
                [1.0, 0.0, 0.0],
                [0.0, SQRT_HALF, -SQRT_HALF],
            ),
            (
                [0.0, -SQRT_HALF, SQRT_HALF],
                PlusX,
                [1.0, 0.0, 0.0],
                [0.0, SQRT_HALF, SQRT_HALF],
            ),
            {
                let third = 1.0 / 3.0_f64.sqrt();
                let sixth = 1.0 / 6.0_f64.sqrt();
                (
                    [third, third, third],
                    PlusY,
                    [-sixth, 2.0 * sixth, -sixth],
                    [-SQRT_HALF, 0.0, SQRT_HALF],
                )
            },
        ];
        for (n, token, x, y) in cases {
            let f = frame(n, [0.0; 3], token, false).unwrap();
            near(f.x, x, 1e-15);
            near(f.y, y, 1e-15);
            f.check().unwrap();
        }
        assert_eq!(AxisToken::default_x([SQRT_HALF, 0.0, SQRT_HALF]), PlusY);
        assert_eq!(AxisToken::default_x([0.0, SQRT_HALF, SQRT_HALF]), MinusX);
        assert_eq!(AxisToken::default_x([0.0, -SQRT_HALF, SQRT_HALF]), PlusX);
    }

    #[test]
    fn an_axis_along_the_normal_is_refused_with_how_much_it_leaves() {
        // Face2 turned 90° about Z: n = +Y and the token `+y`. |a − n(n·a)| = 0, under 1e-12.
        assert_eq!(
            frame([0.0, 1.0, 0.0], [0.0; 3], PlusY, false),
            Err(FaceFrameError::AxisAlongNormal(0.0))
        );
        assert_eq!(
            frame([0.0, 1.0, 0.0], [0.0; 3], MinusY, true),
            Err(FaceFrameError::AxisAlongNormal(0.0))
        );
        // The note's case: Face2 (`+y`) turned 90° about Z, n = (cos 90°, sin 90°, 0) in
        // floating point, (6.1e-17, 1, 0). a − n(n·a) = (0, 1, 0) − n·1 = (−6.1e-17, 0, 0):
        // |…| = cos 90°, under 1e-12, refused.
        let c = libm::cos(std::f64::consts::FRAC_PI_2);
        let n = [c, libm::sin(std::f64::consts::FRAC_PI_2), 0.0];
        assert_eq!(n[1], 1.0);
        let Err(FaceFrameError::AxisAlongNormal(off)) = frame(n, [0.0; 3], PlusY, false) else {
            panic!();
        };
        assert!((off - c).abs() <= 1e-30, "{off} against {c}");
        // `+x` is perpendicular to that n: taken, x ≈ +X.
        let f = frame(n, [0.0; 3], PlusX, false).unwrap();
        near(f.x, [1.0, 0.0, 0.0], 1e-15);
        // The working tokens of n = +Y: all but ±y, in their order.
        assert_eq!(working_tokens([0.0, 1.0, 0.0]), ["+x", "-x", "+z", "-z"]);
        assert_eq!(working_tokens([0.0, 0.0, -1.0]), ["+x", "-x", "+y", "-y"]);
    }

    #[test]
    fn an_axis_1e_10_off_the_normal_is_taken_and_its_frame_is_orthonormal() {
        // The note's open point 5: n = (e, s, 0) with e = 1e-10 and s = √(1 − e²), against
        // the token `+y`. By hand, a − n(n·a) = (0, 1, 0) − s(e, s, 0) = (−es, e², 0), whose
        // length is e·√(s² + e²) = e = 1e-10, over 1e-12: taken. x is that over its length,
        // (−s, e, 0) ≈ (−1, 1e-10, 0), to about 1e-16 / 1e-10 = 1e-6 after the first
        // projection's rounding; y = n × x = (0, 0, e² + s²) = +Z. Projected once only, x·n
        // would be off by up to 1e-6, far past the protocol's 1e-12: projected again, the
        // frame passes its check, and x·n is rounding.
        let e: f64 = 1e-10;
        let n = [e, (1.0_f64 - e * e).sqrt(), 0.0];
        let f = frame(n, [0.0, 7.0, 0.0], PlusY, false).unwrap();
        near(f.x, [-1.0, 0.0, 0.0], 1e-6);
        near(f.y, [0.0, 0.0, 1.0], 1e-6);
        f.check().unwrap();
        assert!(vec3::dot(f.x, n).abs() <= 1e-15, "{f:?}");
        // The origin is n(n·p) = n × 7√(1 − e²), within rounding of (7e-10, 7, 0).
        near(f.origin, [7.0 * e, 7.0, 0.0], 1e-14);
    }

    #[test]
    fn near_the_threshold_every_frame_taken_passes_the_protocols_check() {
        // Normals leaning toward +y by 10^-k for k in 1..=11, each against `+y`: the
        // projection is about 10^-k long, taken down to 1e-11 (over 1e-12), each frame
        // orthonormal and right-handed to 1e-12 once x is projected a second time.
        for k in 1..=11 {
            let e = libm::pow(10.0, -f64::from(k));
            let n = [e, (1.0_f64 - e * e).sqrt(), 0.0];
            let f = frame(n, [3.0, -2.0, 1.0], PlusY, false).unwrap();
            f.check().unwrap_or_else(|why| panic!("10^-{k}: {why:?}"));
            let r = frame(n, [3.0, -2.0, 1.0], PlusY, true).unwrap();
            r.check()
                .unwrap_or_else(|why| panic!("10^-{k} reversed: {why:?}"));
        }
        // 1e-13 is under the threshold.
        let e: f64 = 1e-13;
        let n = [e, (1.0_f64 - e * e).sqrt(), 0.0];
        assert!(frame(n, [0.0; 3], PlusY, false).is_err());
    }

    #[test]
    fn a_nan_normal_is_refused() {
        assert!(frame([f64::NAN, 0.0, 1.0], [0.0; 3], PlusX, false).is_err());
    }

    fn cube(feature: &str) -> ElementMap {
        ElementMap::primitive(
            id(feature),
            &Primitive::Box {
                dx: 10.0,
                dy: 10.0,
                dz: 10.0,
            },
        )
    }

    fn makers(feature: &FeatureId) -> Option<Maker> {
        match feature.as_str() {
            "b0000001" | "b0000002" => Some(Maker::Itself(PrimitiveKind::Box)),
            "c0000001" => Some(Maker::Itself(PrimitiveKind::Cylinder)),
            _ => None,
        }
    }

    /// A 10 mm cube's description with its corner at the origin, OCCT's numbering: Face1 x
    /// = 0, Face2 x = 10, Face3 y = 0, Face4 y = 10, Face5 z = 0, Face6 z = 10; `bent` a
    /// face made a cylinder's.
    fn cube_described(bent: Option<usize>) -> Description {
        let planes = [
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
            ([1.0, 0.0, 0.0], [10.0, 0.0, 0.0]),
            ([0.0, -1.0, 0.0], [0.0, 0.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 10.0, 0.0]),
            ([0.0, 0.0, -1.0], [0.0, 0.0, 0.0]),
            ([0.0, 0.0, 1.0], [0.0, 0.0, 10.0]),
        ];
        Description {
            faces: planes
                .iter()
                .enumerate()
                .map(|(i, &(normal, origin))| FaceDescription {
                    surface: if bent == Some(i + 1) {
                        Surface::Cylinder {
                            axis: silico_protocol::Axis {
                                origin: [0.0; 3],
                                direction: [0.0, 0.0, 1.0],
                            },
                            radius: 1.0,
                        }
                    } else {
                        Surface::Plane {
                            normal,
                            location: origin,
                        }
                    },
                    area: 100.0,
                    centroid: origin,
                })
                .collect(),
            edges: Vec::new(),
            vertices: Vec::new(),
        }
    }

    #[test]
    fn a_face_found_in_a_live_solid_is_placed_on_its_plane() {
        let map = cube("b0000001");
        let described = cube_described(None);
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let placed = place(
            &id("s0000001"),
            &face_plane("b0000001.box.Face6", PlusX, false),
            &live,
            &Vec::new,
            &makers,
        );
        let Resolved::Placed(Placed::Frame(f)) = placed else {
            panic!("{placed:?}");
        };
        assert_eq!(
            (f.origin, f.x, f.y, normal(&f)),
            (
                [0.0, 0.0, 10.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0]
            )
        );
    }

    #[test]
    fn a_face_in_no_live_solid_is_broken_with_the_faces_nearest_it_as_candidates() {
        let map = cube("b0000001");
        let described = cube_described(None);
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let Resolved::Placed(Placed::Broken(e)) = place(
            &id("s0000001"),
            &face_plane("b0000001.box.Face7", PlusX, false),
            &live,
            &Vec::new,
            &makers,
        ) else {
            panic!();
        };
        assert_eq!(e.code(), ErrorCode::ReferenceBroken);
        assert_eq!(e.feature(), Some("s0000001"));
        assert_eq!(e.parameter(), Some("plane.face"));
        assert_eq!(e.elements(), ["b0000001.box.Face7"]);
        // A box's Face7 names nothing: its nearest by number, Face6, is the candidate.
        assert_eq!(e.candidates(), ["b0000001.box.Face6"]);
        assert_eq!(
            e.message(),
            "plane.face, \"b0000001.box.Face7\", names no face of the solids live at the sketch's place: it isn't one of the faces they're made from, or an edit before this sketch removed or remade it"
        );
    }

    #[test]
    fn an_edge_by_its_name_is_refused_as_no_face() {
        let map = cube("b0000001");
        let described = cube_described(None);
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let Resolved::Placed(Placed::Broken(e)) = place(
            &id("s0000001"),
            &face_plane("b0000001.box.Edge3", PlusX, false),
            &live,
            &Vec::new,
            &makers,
        ) else {
            panic!();
        };
        assert_eq!(e.parameter(), Some("plane.face"));
        assert!(
            e.message().ends_with("names an edge, not a face"),
            "{}",
            e.message()
        );
    }

    #[test]
    fn a_face_that_isnt_planar_is_broken_with_the_planar_faces_as_candidates() {
        let map = cube("b0000001");
        let described = cube_described(Some(6));
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let Resolved::Placed(Placed::Broken(e)) = place(
            &id("s0000001"),
            &face_plane("b0000001.box.Face6", PlusX, false),
            &live,
            &Vec::new,
            &makers,
        ) else {
            panic!();
        };
        assert_eq!(e.code(), ErrorCode::ReferenceBroken);
        assert_eq!(e.parameter(), Some("plane.face"));
        assert!(
            e.message()
                .contains("isn't planar: OCCT types its surface a cylinder"),
            "{}",
            e.message()
        );
        // The five planar faces, by name: Face1 to Face5.
        let want: Vec<String> = (1..=5).map(|i| format!("b0000001.box.Face{i}")).collect();
        assert_eq!(e.candidates(), want);
    }

    #[test]
    fn an_axis_along_the_faces_normal_is_broken_on_plane_x_with_the_working_tokens() {
        let map = cube("b0000001");
        let described = cube_described(None);
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let Resolved::Placed(Placed::Broken(e)) = place(
            &id("s0000001"),
            &face_plane("b0000001.box.Face6", MinusZ, false),
            &live,
            &Vec::new,
            &makers,
        ) else {
            panic!();
        };
        assert_eq!(e.code(), ErrorCode::ReferenceBroken);
        assert_eq!(e.feature(), Some("s0000001"));
        assert_eq!(e.parameter(), Some("plane.x"));
        assert_eq!(e.elements(), ["-z"]);
        assert_eq!(e.candidates(), ["+x", "-x", "+y", "-y"]);
    }

    #[test]
    fn a_face_not_found_waits_for_a_solid_not_built_and_is_blocked_by_one_that_failed() {
        let map = cube("b0000001");
        let described = cube_described(None);
        let built = Live::Built {
            map: &map,
            description: Ok(&described),
        };
        let plane = face_plane("b0000002.box.Face6", PlusX, false);
        let sketch = id("s0000001");
        // A related solid not built yet: it waits, naming that solid's feature, whatever
        // else failed.
        assert_eq!(
            place(
                &sketch,
                &plane,
                &[Live::Failed(id("b0000002")), Live::Waits(id("p0000001"))],
                &Vec::new,
                &makers
            ),
            Resolved::Waits(id("p0000001"))
        );
        assert_eq!(
            place(
                &sketch,
                &plane,
                &[Live::Failed(id("b0000002"))],
                &Vec::new,
                &makers
            ),
            Resolved::Placed(Placed::Blocked(id("b0000002")))
        );
        // A solid that isn't related never holds it: not searched, so its face isn't found
        // there, and the name is broken, the unrelated solid's faces its candidates.
        let unrelated = place(
            &sketch,
            &face_plane("b0000001.box.Face6", PlusX, false),
            &[],
            &|| {
                vec![Live::Built {
                    map: &map,
                    description: Ok(&described),
                }]
            },
            &makers,
        );
        let Resolved::Placed(Placed::Broken(e)) = unrelated else {
            panic!("{unrelated:?}");
        };
        assert_eq!(e.candidates(), ["b0000001.box.Face6"]);
        // Found in a related solid built, the others don't matter.
        let found = place(
            &sketch,
            &face_plane("b0000001.box.Face6", PlusX, false),
            &[
                Live::Waits(id("p0000001")),
                built,
                Live::Failed(id("b0000002")),
            ],
            &Vec::new,
            &makers,
        );
        assert!(
            matches!(found, Resolved::Placed(Placed::Frame(_))),
            "{found:?}"
        );
    }

    #[test]
    fn a_solid_that_couldnt_be_described_fails_the_sketch_with_why() {
        let map = cube("b0000001");
        let why = Error::new(ErrorCode::WorkerCrashed, "the worker crashed");
        let live = [Live::Built {
            map: &map,
            description: Err(why),
        }];
        let placed = place(
            &id("s0000001"),
            &face_plane("b0000001.box.Face6", PlusX, false),
            &live,
            &Vec::new,
            &makers,
        );
        let Resolved::Placed(Placed::Failed(e)) = placed else {
            panic!("{placed:?}");
        };
        assert_eq!(e.code(), ErrorCode::WorkerCrashed);
        assert_eq!(e.feature(), Some("s0000001"));
    }

    #[test]
    fn a_face_of_a_feature_retyped_says_so() {
        // c0000001 is a cylinder now; the name says a box made the face.
        let map = cube("b0000001");
        let described = cube_described(None);
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let Resolved::Placed(Placed::Broken(e)) = place(
            &id("s0000001"),
            &face_plane("c0000001.box.Face6", PlusX, false),
            &live,
            &Vec::new,
            &makers,
        ) else {
            panic!();
        };
        assert!(
            e.message()
                .contains("names an element a box made, but c0000001 is a cylinder now"),
            "{}",
            e.message()
        );
    }
}
