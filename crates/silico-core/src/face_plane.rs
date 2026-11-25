//! A sketch on a planar face, placed (docs/ports/attacher.md, *How a rebuild places it
//! (M5.9)*; F1 and F2): its face found by name in the solids live at the sketch's place, as
//! a fillet's edge is found in its base (M2.8), and its frame worked out from the face's
//! plane, the stored axis, and `reversed`. A name that finds nothing, or finds an element by
//! an unstable name, a face OCCT doesn't type as a plane, and an axis parallel to the face's
//! normal are each `reference_broken` on the sketch, with candidates, never re-pointed. A
//! live solid that failed, or was blocked, blocks the sketch where the face isn't found in
//! another; one not built yet leaves it to the next round of the rebuild.

use silico_protocol::{
    FaceFrame, FaceFrameError, MIN_AXIS_OFF_NORMAL, NORMAL_LENGTH_TOLERANCE, Surface,
};

use crate::document::{AxisToken, ElementName, FacePlane, FeatureId};
use crate::kernel::{Description, ElementKind};
use crate::naming::{ElementMap, Maker, Name};
use crate::references::{CANDIDATES, Candidates, Lookup, cut_short, look_up, refused};
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

/// What the face of a sketch added without its `x` gives it (M4.1 section 5's
/// `add_feature`; [`default_x`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Defaulted {
    /// The face's default token (F1), as `elements` reports it as its `default_x`.
    X(AxisToken),
    /// Why there's none: the face's name is broken, or its solid failed or was blocked, as a
    /// rebuild would place the sketch ([`Placed`], never a frame).
    Refused(Placed),
    /// The solid that holds it couldn't be described: why, as met, naming no sketch (a
    /// kernel fault is worded as it was met, then put on the sketch).
    Undescribed(Error),
    /// A solid that could hold it isn't built yet, the feature named.
    Waits(FeatureId),
}

/// The default `x` (F1) of the sketch `sketch` on the face `face`, found among the solids
/// live at its place as [`place`] finds it, by the same rule and in the same words: the
/// token `AxisToken::default_x` gives the face's normal, as `elements` reports it.
pub(crate) fn default_x<'a>(
    sketch: &FeatureId,
    face: &ElementName,
    related: &[Live<'a>],
    others: &dyn Fn() -> Vec<Live<'a>>,
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Defaulted {
    match found_face(sketch, face.as_str(), related, others, makers) {
        Ok(Found::Plane { normal, .. }) => Defaulted::X(AxisToken::default_x(normal)),
        Ok(Found::Not(placed)) => Defaulted::Refused(placed),
        Ok(Found::Undescribed(error)) => Defaulted::Undescribed(error),
        Err(on) => Defaulted::Waits(on),
    }
}

/// What a sketch's face is found to be among the solids live at its place ([`found_face`]).
enum Found {
    /// A face OCCT types a plane: its unit normal, out of its solid, and a point on it.
    Plane {
        normal: [f64; 3],
        location: [f64; 3],
    },
    /// Not found, or not a face a sketch can sit on: why, as the sketch's placing.
    Not(Placed),
    /// Found in a solid that couldn't be described: why, as the solid's own read has it.
    Undescribed(Error),
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
    let (normal, location) = match found_face(sketch, text, related, others, makers)? {
        Found::Plane { normal, location } => (normal, location),
        Found::Not(placed) => return Ok(placed),
        // As met: describing the solid, which names no feature of its own. A read of the
        // sketch names it, the feature the fault stopped (`elements::worded_of`).
        Found::Undescribed(error) => return Ok(Placed::Failed(error)),
    };
    Ok(match frame(normal, location, plane.x, plane.reversed) {
        Ok(frame) => Placed::Frame(frame),
        Err(FaceFrameError::AxisAlongNormal(off)) => {
            Placed::Broken(parallel(sketch, plane, normal, off))
        }
        // A normal that isn't a unit to `describe`'s tolerance (or not finite): the kernel
        // described the face badly, a fault in Silico, not the document's.
        Err(FaceFrameError::Invalid(invalid)) if !is_unit(normal) => Placed::Failed(
            silico_bug(&format!(
                "the kernel described the sketch's face's plane so no frame can be made of it: {}",
                invalid.message
            ))
            .with_feature(sketch.as_str())
            .with_elements([cut_short(text)]),
        ),
        // A frame no sketch can be placed on (its origin past the reach, as a slanted face
        // far out can give): the sketch's, in the document's terms.
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
    })
}

/// The face `text` names of the sketch `sketch`, among the solids live at its place
/// (`related`, those that can hold it, and `others`, read for a broken name's candidates),
/// as [`place`] says: a face OCCT types a plane, or why it's none a sketch can sit on; or
/// the feature whose solid it waits for.
fn found_face<'a>(
    sketch: &FeatureId,
    text: &str,
    related: &[Live<'a>],
    others: &dyn Fn() -> Vec<Live<'a>>,
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Result<Found, FeatureId> {
    // The sketch's refusal of the face for `why`, its candidates the caller's to give.
    let face_refused = |why: &str| {
        refused("plane.face", text, why, ElementKind::Face).with_feature(sketch.as_str())
    };
    let broken = |error: Error| Found::Not(Placed::Broken(error));
    let solids = built(related);
    let maps: Vec<&ElementMap> = solids.iter().map(|(map, _)| *map).collect();
    // A broken name's candidates are read from every live solid, so its refusal waits until
    // each has settled: the same candidates whichever round it comes in (docs/ports/
    // attacher.md, *In rounds*).
    let refusal = |why: &str, name: &Name| -> Result<Found, FeatureId> {
        let others = others();
        if let Some(on) = waiting(&others) {
            return Err(on);
        }
        let every: Vec<&ElementMap> = maps
            .iter()
            .copied()
            .chain(built(&others).into_iter().map(|(map, _)| map))
            .collect();
        Ok(broken(face_refused(why).with_ranked(
            Candidates::of(&every, ElementKind::Face).of_name(name),
        )))
    };
    // Read as a fillet's edge is (`references::look_up`): only the related solids built can
    // hold it.
    let (description, element, name) = match look_up(text, ElementKind::Face, &maps, makers) {
        Lookup::Found { map, element, name } => match solids.get(map) {
            Some((_, description)) => (*description, element, name),
            None => {
                return Ok(Found::Not(Placed::Failed(
                    silico_bug("a face was found in a solid that isn't among those searched")
                        .with_feature(sketch.as_str()),
                )));
            }
        },
        // The gate refuses a face that isn't a name: not reached from a checked document.
        Lookup::Refused { why, name: None } => return Ok(broken(face_refused(&why))),
        // Found, by an unstable name or as another kind of element.
        Lookup::Refused {
            why,
            name: Some(name),
        } => return refusal(&why, &name),
        Lookup::Missing { name, retyped } => {
            // Not in any related solid built: one not built yet may hold it, or one that
            // failed.
            if let Some(on) = waiting(related) {
                return Err(on);
            }
            if let Some(root) = related.iter().find_map(|solid| match solid {
                Live::Failed(root) => Some(root.clone()),
                Live::Built { .. } | Live::Waits(_) => None,
            }) {
                return Ok(Found::Not(Placed::Blocked(root)));
            }
            let why = retyped.unwrap_or_else(|| {
                "names no face of the solids live at the sketch's place: it isn't one of the \
                 faces they're made from, or an edit before this sketch removed or remade it"
                    .to_owned()
            });
            return refusal(&why, &name);
        }
    };
    let description = match description {
        Ok(description) => *description,
        Err(error) => return Ok(Found::Undescribed(error.clone())),
    };
    let face = element
        .index
        .checked_sub(1)
        .and_then(|i| description.faces.get(i as usize));
    let Some(face) = face else {
        return Ok(Found::Not(Placed::Failed(
            silico_bug("a face's name found it in a solid whose description hasn't it")
                .with_feature(sketch.as_str())
                .with_elements([cut_short(text)]),
        )));
    };
    let Surface::Plane { normal, location } = face.surface else {
        // Its candidates are read from every live solid, so the refusal waits until each
        // has settled: the same candidates, whichever round it comes in.
        let others = others();
        if let Some(on) = waiting(&others) {
            return Err(on);
        }
        let what = crate::elements::surface_type(&face.surface).word();
        // The planar faces, by a rule of their own: unranked.
        return Ok(broken(
            face_refused(&format!(
                "names a face that isn't planar: OCCT types its surface a {what}, and a sketch sits only on a face typed a plane"
            ))
            .with_candidates(planar_faces(&name, related, &others)),
        ));
    };
    Ok(Found::Plane { normal, location })
}

/// Whether `normal` is a unit vector to [`NORMAL_LENGTH_TOLERANCE`], as `describe` gives a
/// plane's.
fn is_unit(normal: [f64; 3]) -> bool {
    (silico_protocol::vec3::norm(normal) - 1.0).abs() <= NORMAL_LENGTH_TOLERANCE
}

/// An error that's a bug in Silico's.
fn silico_bug(message: &str) -> Error {
    Error::new(ErrorCode::Internal, message).with_suggestion(crate::worker::SILICO_BUG)
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
        // 1e-13 is under the threshold: refused as along the normal, and how far it stands
        // off said (an_axis_along_the_normal_is_refused_saying_how_far_it_stands_off…).
        let e: f64 = 1e-13;
        let n = [e, (1.0_f64 - e * e).sqrt(), 0.0];
        assert!(matches!(
            frame(n, [0.0; 3], PlusY, false),
            Err(FaceFrameError::AxisAlongNormal(_))
        ));
    }

    #[test]
    fn a_nan_normal_is_refused() {
        let Err(FaceFrameError::Invalid(e)) = frame([f64::NAN, 0.0, 1.0], [0.0; 3], PlusX, false)
        else {
            panic!();
        };
        assert_eq!(e.parameter, "plane");
        assert_eq!(
            e.message,
            "the face's normal must be finite, got [NaN, 0.0, 1.0]"
        );
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
            "f0000001" => Some(Maker::By(crate::naming::OpCode::Fuse)),
            _ => None,
        }
    }

    /// A 10 mm cube's description with its corner at the origin, OCCT's numbering: Face1 x
    /// = 0, Face2 x = 10, Face3 y = 0, Face4 y = 10, Face5 z = 0, Face6 z = 10; `bent` a
    /// face made a cylinder's.
    fn cube_described(bent: Option<usize>) -> Description {
        cube_bent(&bent.into_iter().collect::<Vec<_>>())
    }

    /// [`cube_described`] with each face of `bent` made a cylinder's. Each face's centroid
    /// is off its plane (a fake's, never a solid's): the frame is placed from the plane's
    /// location, so a resolver that read the centroid would place it elsewhere.
    fn cube_bent(bent: &[usize]) -> Description {
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
                    surface: if bent.contains(&(i + 1)) {
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
                    centroid: [origin[0] + 5.0, origin[1] + 6.0, origin[2] + 7.0],
                    orientation: silico_protocol::FaceOrientation::Forward,
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
        // A box's Face7 names nothing: its nearest by number, Face6, is the candidate, 1
        // from it, the box's own, sharing nothing.
        assert_eq!(e.candidates(), ["b0000001.box.Face6"]);
        assert_eq!(
            e.ranking(),
            [crate::Rank {
                shared: Vec::new(),
                unshared: vec!["b0000001.box.Face7".to_owned()],
                same_feature: true,
                nearest: Some(crate::Nearest {
                    element: "b0000001.box.Face6".to_owned(),
                    gap: 1,
                }),
            }]
        );
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
        // By a rule of their own, which the suggestion states: unranked.
        assert!(e.ranking().is_empty(), "{:?}", e.ranking());
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
        // Axis tokens, not names: unranked.
        assert!(e.ranking().is_empty(), "{:?}", e.ranking());
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
        // As met, describing the box: it names no feature, so it's worded as the box's read
        // words it, and a read of the sketch then names the sketch (`elements::worded_of`).
        assert_eq!(
            e,
            Error::new(ErrorCode::WorkerCrashed, "the worker crashed")
        );
        assert_eq!(e.feature(), None);
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

    #[test]
    fn a_stored_token_that_isnt_the_faces_default_is_the_frames_x() {
        // Face6 (n = +Z), whose default is `+x`, stored `+y`: x = +Y, y = n × x = (0, 0, 1)
        // × (0, 1, 0) = (−1, 0, 0), by hand. A resolver that re-derived the default would
        // give x = +X.
        let map = cube("b0000001");
        let described = cube_described(None);
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let placed = place(
            &id("s0000001"),
            &face_plane("b0000001.box.Face6", PlusY, false),
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
                [0.0, 1.0, 0.0],
                [-1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0]
            )
        );
    }

    /// A fuse, `f0000001`, of the cube `b0000001`, whose result has three faces: the first
    /// two generated alike from the cube's Face1 and Face6 (so their name is unstable, N3),
    /// the third from its Face1 and Face3; and that solid's description, three planes.
    fn fused_alike() -> (ElementMap, Description) {
        use crate::kernel::{
            ElementCounts, ElementFate, ElementRef, History, InputHistory, Reported, Topology,
        };
        let face = |index| Reported {
            element: ElementRef {
                kind: ElementKind::Face,
                index,
            },
            position: 1,
        };
        let mut faces = vec![ElementFate::default(); 6];
        faces[0].generated = vec![face(1), face(2), face(3)];
        faces[2].generated = vec![face(3)];
        faces[5].generated = vec![face(1), face(2)];
        let history = History {
            inputs: vec![InputHistory {
                faces,
                edges: vec![ElementFate::default(); 12],
                vertices: vec![ElementFate::default(); 8],
            }],
            result: ElementCounts {
                faces: 3,
                edges: 0,
                vertices: 0,
                solids: 1,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![vec![]; 3],
                outer_edges: vec![vec![]; 3],
                edge_vertices: vec![],
                solid_faces: vec![vec![1, 2, 3]],
            },
        };
        history.validate(1).unwrap();
        let map = ElementMap::operation(
            id("f0000001"),
            crate::naming::OpCode::Fuse,
            &[&cube("b0000001")],
            &history,
        );
        let plane = |z: f64| FaceDescription {
            surface: Surface::Plane {
                normal: [0.0, 0.0, 1.0],
                location: [0.0, 0.0, z],
            },
            area: 1.0,
            centroid: [0.0, 0.0, z],
            orientation: silico_protocol::FaceOrientation::Forward,
        };
        let described = Description {
            faces: vec![plane(1.0), plane(2.0), plane(3.0)],
            edges: Vec::new(),
            vertices: Vec::new(),
        };
        (map, described)
    }

    #[test]
    fn a_face_by_an_unstable_name_is_broken_with_the_stable_faces_sharing_its_sources() {
        // Found by the tests review: the face's resolve said an unstable name, but offered
        // no candidates, where a fillet's offers them for its edges (one `look_up` both take
        // now). The two faces from Face1 and Face6 share a name, unstable; the face from
        // Face1 and Face3 shares Face1 with it, so it's the candidate, as the fillet's would
        // be for an edge.
        let (map, described) = fused_alike();
        let alike = "f0000001.fuse.Face(G1/2 b0000001.box.Face1, G1/2 b0000001.box.Face6)";
        let stable = "f0000001.fuse.Face(G b0000001.box.Face1, G b0000001.box.Face3)";
        let unstable: Vec<String> = map.unstable().map(ToString::to_string).collect();
        assert!(unstable.contains(&alike.to_owned()), "{unstable:?}");
        assert!(!unstable.contains(&stable.to_owned()), "{unstable:?}");
        for name in [alike.to_owned(), format!("{alike}~1")] {
            let live = [Live::Built {
                map: &map,
                description: Ok(&described),
            }];
            let placed = place(
                &id("s0000001"),
                &face_plane(&name, PlusX, false),
                &live,
                &Vec::new,
                &makers,
            );
            let Resolved::Placed(Placed::Broken(e)) = placed else {
                panic!("{name}: {placed:?}");
            };
            assert_eq!(e.code(), ErrorCode::ReferenceBroken, "{name}");
            assert_eq!(e.parameter(), Some("plane.face"), "{name}");
            // The name cut short in the message (it's in `elements` whole).
            assert!(
                e.message().starts_with("plane.face, \"f0000001.fuse.Face(G1/2 ")
                    && e.message().ends_with(
                        ", names a face, whose name is unstable (it came about as another element did, or is its number): an edit could make it another element"
                    ),
                "{}",
                e.message()
            );
            assert_eq!(e.elements(), [name.as_str()]);
            assert_eq!(e.candidates(), [stable], "{name}");
            assert_eq!(
                e.suggestion(),
                Some(
                    "name the face as its feature now names it: the candidates are the likeliest, none taken for it"
                )
            );
            // And it waits, as a name found nowhere does, while a live solid it could read
            // candidates from isn't built: the same candidates whichever round refuses it.
            let waits = place(
                &id("s0000001"),
                &face_plane(&name, PlusX, false),
                &live,
                &|| vec![Live::Waits(id("p0000009"))],
                &makers,
            );
            assert_eq!(waits, Resolved::Waits(id("p0000009")), "{name}");
        }
    }

    #[test]
    fn an_edge_by_its_name_waits_for_every_live_solid_before_its_refusal() {
        // Refused as no face only once every live solid has settled, as a name found
        // nowhere is: its candidates are read from them all.
        let map = cube("b0000001");
        let described = cube_described(None);
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let plane = face_plane("b0000001.box.Edge3", PlusX, false);
        assert_eq!(
            place(
                &id("s0000001"),
                &plane,
                &live,
                &|| vec![Live::Waits(id("p0000009"))],
                &makers
            ),
            Resolved::Waits(id("p0000009"))
        );
        let Resolved::Placed(Placed::Broken(e)) =
            place(&id("s0000001"), &plane, &live, &Vec::new, &makers)
        else {
            panic!();
        };
        assert_eq!(
            e.message(),
            "plane.face, \"b0000001.box.Edge3\", names an edge, not a face"
        );
        // An edge's name shares no element with a face's, and isn't a face's by number:
        // none.
        assert!(e.candidates().is_empty(), "{:?}", e.candidates());
    }

    #[test]
    fn a_face_that_isnt_planar_offers_its_own_features_planar_faces_first_though_they_sort_after() {
        // The face's feature, b0000002, sorts after the other live solid's, b0000001: its
        // one planar face (Face1, the rest bent) comes first all the same, then b0000001's
        // by name, five in all. By name alone, b0000001's five would be the candidates.
        let own = cube("b0000002");
        let own_described = cube_bent(&[2, 3, 4, 5, 6]);
        let other = cube("b0000001");
        let other_described = cube_described(None);
        let live = [Live::Built {
            map: &own,
            description: Ok(&own_described),
        }];
        let placed = place(
            &id("s0000001"),
            &face_plane("b0000002.box.Face6", PlusX, false),
            &live,
            &|| {
                vec![Live::Built {
                    map: &other,
                    description: Ok(&other_described),
                }]
            },
            &makers,
        );
        let Resolved::Placed(Placed::Broken(e)) = placed else {
            panic!("{placed:?}");
        };
        assert_eq!(
            e.candidates(),
            [
                "b0000002.box.Face1",
                "b0000001.box.Face1",
                "b0000001.box.Face2",
                "b0000001.box.Face3",
                "b0000001.box.Face4",
            ]
        );
    }

    #[test]
    fn an_axis_along_the_normal_is_refused_saying_how_far_it_stands_off_and_what_to_do() {
        // 1e-13 off the normal: under 1e-12, refused saying 1e-13 (to its rounding).
        let e: f64 = 1e-13;
        let n = [e, (1.0_f64 - e * e).sqrt(), 0.0];
        let Err(FaceFrameError::AxisAlongNormal(off)) = frame(n, [0.0; 3], PlusY, false) else {
            panic!();
        };
        assert!((off - e).abs() <= 1e-28, "{off}");
        // Through the resolve, on Face4 (n = +Y) with `+y`: its words, its candidates.
        let map = cube("b0000001");
        let described = cube_described(None);
        let live = [Live::Built {
            map: &map,
            description: Ok(&described),
        }];
        let Resolved::Placed(Placed::Broken(e)) = place(
            &id("s0000001"),
            &face_plane("b0000001.box.Face4", PlusY, false),
            &live,
            &Vec::new,
            &makers,
        ) else {
            panic!();
        };
        assert_eq!(
            e.message(),
            "plane.x, \"+y\", is parallel to the face's normal [0.0, 1.0, 0.0]: projected onto the face it leaves 0, under the 1e-12 a sketch's x needs"
        );
        assert_eq!(e.elements(), ["+y"]);
        assert_eq!(e.candidates(), ["+x", "-x", "+z", "-z"]);
        assert_eq!(
            e.suggestion(),
            Some(
                "give plane.x one of the candidates, an axis that isn't the face's normal: none is taken for it"
            )
        );
    }

    #[test]
    fn a_normal_the_kernel_gave_badly_is_a_bug_naming_the_face_not_the_documents_fault() {
        // NaN, 1e200 long, 1e-160 long, and 1 + 2e-6 long: none a unit to describe's 1e-6,
        // so no frame, and the fault is the kernel's description: `internal`, naming the
        // sketch and the face, saying why, never `invalid_input` on the document's plane.
        let map = cube("b0000001");
        for (n, said) in [
            (
                [f64::NAN, 0.0, 1.0],
                "the face's normal must be finite, got [NaN, 0.0, 1.0]",
            ),
            (
                [0.0, 0.0, 1e200],
                "the face's normal must be a unit vector to 1e-6, got [0.0, 0.0, 1e200], 1e200 long",
            ),
            (
                [0.0, 0.0, 1e-160],
                "the face's normal must be a unit vector to 1e-6, got [0.0, 0.0, 1e-160], 1e-160 long",
            ),
            (
                [0.0, 0.0, 1.000002],
                "the face's normal must be a unit vector to 1e-6, got [0.0, 0.0, 1.000002], 1.000002 long",
            ),
        ] {
            let mut described = cube_described(None);
            described.faces[5].surface = Surface::Plane {
                normal: n,
                location: [0.0, 0.0, 10.0],
            };
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
            let Resolved::Placed(Placed::Failed(e)) = placed else {
                panic!("{n:?}: {placed:?}");
            };
            assert_eq!(e.code(), ErrorCode::Internal, "{n:?}");
            assert_eq!(e.feature(), Some("s0000001"));
            assert_eq!(e.elements(), ["b0000001.box.Face6"]);
            assert_eq!(
                e.message(),
                format!(
                    "the kernel described the sketch's face's plane so no frame can be made of it: {said}"
                )
            );
            assert_eq!(e.suggestion(), Some(crate::worker::SILICO_BUG));
        }
    }
}
