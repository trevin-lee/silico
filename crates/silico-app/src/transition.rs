//! The viewport's view transitions (DESIGN.md's *egui mapping*: "a 200 ms view transition";
//! the approved prototype's `VP.animateTo` and `stepAnim`, docs/design/m5-ui/js/viewport.js):
//! a move to a view worked out whole (Home, Fit, a named view, a ViewCube click, Look At)
//! eases from the camera as it is to that view over [`theme::VIEW_TRANSITION_SECONDS`], by
//! the prototype's quadratic in-out ([`ease`]), and ends on that view exactly. Orbit, pan,
//! and zoom, by a drag or the wheel, move the view at once, and one during a transition ends
//! it where it is (a drag from its press; a wheel's turn that zooms).
//!
//! The prototype's camera is an azimuth and an elevation about its target, and a height;
//! its up follows from them (`upOf`). It eases the azimuth the short way round, and the
//! elevation, the target, and the height, each straight. The app's camera is an eye, a
//! target, and an up ([`Camera`]), which turns about its own axis too (a sketch's Look At
//! whose y isn't the view's up, an orbit over a pole): so each camera is read as the
//! prototype's azimuth and elevation of its direction from the target to the eye, a roll of
//! its up from the prototype's up there, its target, its eye's distance, and, orthographic,
//! its view's height; each of those is eased as the prototype eases its own (the azimuth and
//! the roll the short way round), and the camera made again from them ([`between`]). Where
//! neither end is rolled, the path is the prototype's. Straight up or down, where the
//! azimuth is any, it's read from the camera's right (the prototype's right is `(cos az,
//! sin az, 0)` at every elevation), so a top or a bottom view's up is its own, unrolled, and
//! the up turns smoothly into it. A perspective view's size is its eye's distance, eased
//! straight, as the prototype's height (its distance times a constant) is.

use std::f64::consts::{PI, TAU};

use silico_protocol::vec3::dot;
use silico_protocol::{Camera, Projection};

use crate::{navigate, theme};

/// The prototype's ease (viewport.js's `ease`), quadratic in and out: `2t²` to the middle,
/// `1 − (2 − 2t)²/2` after it, from 0 at 0 to 1 at 1 through ½ at ½.
pub fn ease(t: f64) -> f64 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - u * u / 2.0
    }
}

/// The turn from angle `from` to angle `to` the short way round, in radians, in [−π, π):
/// the prototype's `daz`, `((to − from + π) % 2π + 2π) % 2π − π`.
pub fn short_turn(from: f64, to: f64) -> f64 {
    ((to - from + PI) % TAU + TAU) % TAU - PI
}

/// Below this, a direction's part across the world's z (`hypot(x, y)` of a unit vector) is
/// straight up or down: its azimuth is read from the camera's right instead.
const POLE: f64 = 1e-9;

/// A camera as the prototype's camera holds a view, and its roll about its own axis.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    /// The direction from the target to the eye: `(cos el sin az, −cos el cos az, sin el)`
    /// (the prototype's `dirOf`).
    azimuth: f64,
    elevation: f64,
    /// The up's turn about that direction from the prototype's (`upOf`), toward its right's
    /// opposite: the up is `upOf cos roll − rightOf sin roll`.
    roll: f64,
    target: [f64; 3],
    distance: f64,
}

/// The prototype's right at azimuth `azimuth`, which no elevation changes.
fn right_of(azimuth: f64) -> [f64; 3] {
    [libm::cos(azimuth), libm::sin(azimuth), 0.0]
}

/// The prototype's direction from the target to the eye (`dirOf`) and its up (`upOf`).
fn direction_and_up(azimuth: f64, elevation: f64) -> ([f64; 3], [f64; 3]) {
    let (sa, ca) = (libm::sin(azimuth), libm::cos(azimuth));
    let (se, ce) = (libm::sin(elevation), libm::cos(elevation));
    ([ce * sa, -ce * ca, se], [-se * sa, se * ca, ce])
}

/// `camera` as a [`Pose`]; none for a camera that makes no frame.
fn pose(camera: &Camera) -> Option<Pose> {
    let frame = camera.frame().ok()?;
    let back = frame.back;
    let (azimuth, elevation) = navigate::azimuth_and_elevation(back);
    let (azimuth, roll) = if libm::hypot(back[0], back[1]) < POLE {
        (libm::atan2(frame.right[1], frame.right[0]), 0.0)
    } else {
        let (_, up) = direction_and_up(azimuth, elevation);
        let right = right_of(azimuth);
        (
            azimuth,
            libm::atan2(-dot(frame.up, right), dot(frame.up, up)),
        )
    };
    Some(Pose {
        azimuth,
        elevation,
        roll,
        target: camera.target,
        distance: frame.distance,
    })
}

fn lerp(a: f64, b: f64, along: f64) -> f64 {
    a + (b - a) * along
}

/// The turn of the roll, of `roll` (the short way round) or a whole turn either side of it,
/// that turns the image least about the view's axis, the azimuth's turn turning it by `swing`
/// meanwhile: the first of them where two turn it as little. A view turns about its own axis
/// by its roll's turn and, at elevation `el`, `sin el` of its azimuth's, so near a pole the
/// two each taken the short way could add to a whole turn of the image, where an orbit over
/// the top left both at a half turn (`short_turn` takes either half turn as −π).
fn least_twist(swing: f64, roll: f64) -> f64 {
    let mut least = roll;
    for other in [roll - TAU, roll + TAU] {
        if (swing + other).abs() < (swing + least).abs() {
            least = other;
        }
    }
    least
}

/// How far the image turns about the view's axis with the azimuth's turn `turn`, the
/// elevation moving straight from `from` to `to` meanwhile: the integral of `sin el` times
/// the azimuth's step, `turn (cos from − cos to) / (to − from)`, or `turn sin from` where the
/// elevation keeps still.
fn swing(turn: f64, from: f64, to: f64) -> f64 {
    let rise = to - from;
    if rise.abs() < 1e-9 {
        turn * libm::sin((from + to) / 2.0)
    } else {
        turn * (libm::cos(from) - libm::cos(to)) / rise
    }
}

/// The camera `along` of the way from `from` to `to` (0 at `from`, 1 at `to`, both as
/// [`Pose`]s made again): the azimuth turned the short way round, and the roll by the turn
/// that with it turns the image least ([`least_twist`]); the elevation, the target, the eye's
/// distance, and an orthographic view's height each moved straight.
/// None if either makes no frame, or their projections differ in kind or in angle, which no
/// transition joins.
pub fn between(from: &Camera, to: &Camera, along: f64) -> Option<Camera> {
    let projection = match (from.projection, to.projection) {
        (
            Projection::Orthographic { view_height: a },
            Projection::Orthographic { view_height: b },
        ) => Projection::Orthographic {
            view_height: lerp(a, b, along),
        },
        (
            Projection::Perspective { field_of_view: a },
            Projection::Perspective { field_of_view: b },
        ) if a == b => to.projection,
        _ => return None,
    };
    let (a, b) = (pose(from)?, pose(to)?);
    let turn = short_turn(a.azimuth, b.azimuth);
    let azimuth = a.azimuth + turn * along;
    let elevation = lerp(a.elevation, b.elevation, along);
    let roll_turn = least_twist(
        swing(turn, a.elevation, b.elevation),
        short_turn(a.roll, b.roll),
    );
    let roll = a.roll + roll_turn * along;
    let target: [f64; 3] = std::array::from_fn(|k| lerp(a.target[k], b.target[k], along));
    let distance = lerp(a.distance, b.distance, along);
    let (back, plain_up) = direction_and_up(azimuth, elevation);
    let right = right_of(azimuth);
    let (sr, cr) = (libm::sin(roll), libm::cos(roll));
    let up = std::array::from_fn(|k| plain_up[k] * cr - right[k] * sr);
    let eye = std::array::from_fn(|k| target[k] + back[k] * distance);
    let camera = Camera {
        projection,
        eye,
        target,
        up,
    };
    camera.frame().ok().map(|_| camera)
}

/// Whether a view moves to a view worked out whole over time, and from when: egui's clock,
/// and how long it takes, 0 to jump.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// egui's clock, in seconds (`InputState::time`).
    pub now: f64,
    pub seconds: f64,
}

impl Motion {
    /// A jump, at once.
    pub const JUMP: Motion = Motion {
        now: 0.0,
        seconds: 0.0,
    };

    /// As egui's context has it this frame: [`theme::VIEW_TRANSITION_SECONDS`], or a jump
    /// where egui's `animation_time` is 0 (or less, or not a number), which is what turns
    /// its own animations off (egui_kittest sets it so; DESIGN.md's `prefers-reduced-motion`
    /// has no setting in the app yet, and would set it so).
    pub fn of(ctx: &eframe::egui::Context) -> Motion {
        let moving = ctx.global_style().animation_time > 0.0;
        Motion {
            now: ctx.input(|input| input.time),
            seconds: if moving {
                theme::VIEW_TRANSITION_SECONDS
            } else {
                0.0
            },
        }
    }
}

/// A view transition under way: from the camera as it was, to the view asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    from: Camera,
    to: Camera,
    /// The camera the view label names until the transition ends: the one it named when the
    /// first of the transitions in a row began (the prototype writes its label again only
    /// once one ends, `onView`).
    labelled: Camera,
    start: f64,
    seconds: f64,
}

/// Where a transition is at a moment.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Under way: the camera to draw.
    Moving(Camera),
    /// Ended: the view asked for, exactly as it was worked out.
    Ended(Camera),
}

impl Transition {
    /// From `from` to `to`, begun at `motion.now` and lasting `motion.seconds`, the label
    /// naming `labelled` meanwhile.
    pub fn new(from: Camera, to: Camera, labelled: Camera, motion: Motion) -> Transition {
        Transition {
            from,
            to,
            labelled,
            start: motion.now,
            seconds: motion.seconds,
        }
    }

    /// The view asked for.
    pub fn to(&self) -> &Camera {
        &self.to
    }

    /// The camera the view label names while it's under way.
    pub fn labelled(&self) -> &Camera {
        &self.labelled
    }

    /// Where it is at `now`: eased by [`ease`] of the time gone over the time it takes;
    /// ended at that time or after (or at any time that isn't a number, or for a camera no
    /// step between makes: [`between`]).
    pub fn at(&self, now: f64) -> Step {
        let t = (now - self.start) / self.seconds;
        if t.is_nan() || t >= 1.0 {
            return Step::Ended(self.to.clone());
        }
        match between(&self.from, &self.to, ease(t.max(0.0))) {
            Some(camera) => Step::Moving(camera),
            None => Step::Ended(self.to.clone()),
        }
    }
}

#[cfg(test)]
mod tests;
