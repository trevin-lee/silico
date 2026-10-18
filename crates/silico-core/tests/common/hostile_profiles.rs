//! Hostile profiles for `/break-it` (M3.5): rounded rectangles whose corner arcs are at
//! and around OCCT's tolerance, an arc and its chord with the chord at and around it, small
//! circles at the sliver boundary, and holes their tolerance from their outline, turned,
//! moved up to a kilometre out, and on every plane, each with its area in closed form.
//! Shared by silico-core's tests (the profile and the kernel request's check) and
//! silico-kernel's (the faces OCCT makes of them).
#![allow(dead_code)]

use std::f64::consts::PI;

use proptest::prelude::*;
use silico_core::document::Plane;

use super::draw::Draw;

/// One outline, drawn in its own 30 mm cell.
#[derive(Debug, Clone)]
pub enum Piece {
    /// A `w` × `h` rectangle, each corner (from the lower left, counter-clockwise) rounded
    /// by an arc of its radius (0: a sharp corner), each side's line drawn backwards where
    /// its flag says.
    Rounded {
        w: f64,
        h: f64,
        r: [f64; 4],
        flip: [bool; 4],
    },
    /// An arc of radius `r` about the cell's centre between the angles ±`half`, the long
    /// way round (`big`) or the short, closed by its chord (drawn backwards if `flip`), the
    /// arc's end `off` its circle (outwards; inwards if negative). Its area is the same:
    /// the arc's radius and angles come from its centre, its start, and its end's
    /// direction, and its chord adds nothing to a loop of two curves.
    D {
        r: f64,
        half: f64,
        big: bool,
        flip: bool,
        off: f64,
    },
    /// A circle of radius `r`.
    Circle { r: f64 },
    /// A `w` × `h` rectangle with a circle hole at its centre `gap` from its nearer sides.
    Holed { w: f64, h: f64, gap: f64 },
}

impl Piece {
    /// The area it encloses, in closed form.
    pub fn area(&self) -> f64 {
        match *self {
            Piece::Rounded { w, h, r, .. } => {
                w * h - (1.0 - PI / 4.0) * r.iter().map(|r| r * r).sum::<f64>()
            }
            Piece::D { r, half, big, .. } => {
                let theta = if big {
                    2.0 * PI - 2.0 * half
                } else {
                    2.0 * half
                };
                r * r / 2.0 * (theta - libm::sin(theta))
            }
            Piece::Circle { r } => PI * r * r,
            Piece::Holed { w, h, gap } => {
                let rho = w.min(h) / 2.0 - gap;
                w * h - PI * rho * rho
            }
        }
    }

    /// Its perimeter, roughly (for a tolerance on its area far from the origin).
    pub fn perimeter(&self) -> f64 {
        match *self {
            Piece::Rounded { w, h, .. } | Piece::Holed { w, h, .. } => {
                2.0 * (w + h) + PI * w.min(h)
            }
            Piece::D { r, .. } | Piece::Circle { r } => 2.0 * PI * r + 2.0 * r,
        }
    }
}

/// A sketch of several pieces, each in its own cell, turned by `angle` about `origin` and
/// moved there, on `plane`.
#[derive(Debug, Clone)]
pub struct Hostile {
    pub pieces: Vec<Piece>,
    pub origin: [f64; 2],
    pub angle: f64,
    pub plane: Plane,
}

impl Hostile {
    pub fn area(&self) -> f64 {
        self.pieces.iter().map(Piece::area).sum()
    }

    /// How far the profile's area may be from the closed form: a part in 1e9, and the
    /// rounding of coordinates this far out along the perimeter; and for an arc whose end
    /// is off its circle, the sliver between the arc's own end and the point its chord
    /// runs to, at most |off| × its chord (2r): the profile's measure is exact for it, the
    /// faces OCCT builds aren't.
    pub fn area_tolerance(&self) -> f64 {
        let reach = self.origin[0].abs().max(self.origin[1].abs()).max(1.0) + 200.0;
        let perimeter: f64 = self.pieces.iter().map(Piece::perimeter).sum();
        let off: f64 = self
            .pieces
            .iter()
            .map(|p| match *p {
                Piece::D { r, off, .. } => off.abs() * 2.0 * r,
                _ => 0.0,
            })
            .sum();
        1e-9 * self.area().max(1e-12) + 8.0 * f64::EPSILON * reach * perimeter + off
    }

    /// The sketch.
    pub fn draw(&self) -> Draw {
        let mut d = Draw::new();
        d.sketch.plane = self.plane;
        let (c, s) = (libm::cos(self.angle), libm::sin(self.angle));
        for (k, piece) in self.pieces.iter().enumerate() {
            let cell = 30.0 * k as f64;
            let at = |x: f64, y: f64| -> (f64, f64) {
                let (x, y) = (x + cell, y);
                (
                    self.origin[0] + c * x - s * y,
                    self.origin[1] + s * x + c * y,
                )
            };
            let point = |d: &mut Draw, x: f64, y: f64| {
                let (x, y) = at(x, y);
                d.point(x, y)
            };
            match *piece {
                Piece::Rounded { w, h, r, flip } => {
                    let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
                    // Along each side, counter-clockwise; inwards from each corner.
                    let along = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
                    // Each corner's points: where its incoming side ends and its outgoing
                    // side starts (one point for a sharp corner), and its arc.
                    let mut ends: Vec<(String, String)> = Vec::new();
                    for i in 0..4 {
                        let (cx, cy) = corners[i];
                        let (ix, iy) = along[(i + 3) % 4];
                        let (ox, oy) = along[i];
                        if r[i] == 0.0 {
                            let p = point(&mut d, cx, cy);
                            ends.push((p.clone(), p));
                        } else {
                            let a = point(&mut d, cx - ix * r[i], cy - iy * r[i]);
                            let b = point(&mut d, cx + ox * r[i], cy + oy * r[i]);
                            let o = point(
                                &mut d,
                                cx - ix * r[i] + ox * r[i],
                                cy - iy * r[i] + oy * r[i],
                            );
                            d.arc(&o, &a, &b);
                            ends.push((a, b));
                        }
                    }
                    for i in 0..4 {
                        let from = ends[i].1.clone();
                        let to = ends[(i + 1) % 4].0.clone();
                        if flip[i] {
                            d.line(&to, &from);
                        } else {
                            d.line(&from, &to);
                        }
                    }
                }
                Piece::D {
                    r,
                    half,
                    big,
                    flip,
                    off,
                } => {
                    let o = point(&mut d, 12.0, 12.0);
                    // The arc's end is s when it goes the long way round (from e), else e.
                    let (rs, re) = if big { (r + off, r) } else { (r, r + off) };
                    let s = point(
                        &mut d,
                        12.0 + rs * libm::cos(half),
                        12.0 - rs * libm::sin(half),
                    );
                    let e = point(
                        &mut d,
                        12.0 + re * libm::cos(half),
                        12.0 + re * libm::sin(half),
                    );
                    if big {
                        d.arc(&o, &e, &s);
                    } else {
                        d.arc(&o, &s, &e);
                    }
                    if flip {
                        d.line(&e, &s);
                    } else {
                        d.line(&s, &e);
                    }
                }
                Piece::Circle { r } => {
                    let o = point(&mut d, 12.0, 12.0);
                    d.circle_about(&o, r);
                }
                Piece::Holed { w, h, gap } => {
                    let p: Vec<String> = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
                        .iter()
                        .map(|&(x, y)| point(&mut d, x, y))
                        .collect();
                    for i in 0..4 {
                        d.line(&p[i], &p[(i + 1) % 4]);
                    }
                    let o = point(&mut d, w / 2.0, h / 2.0);
                    d.circle_about(&o, w.min(h) / 2.0 - gap);
                }
            }
        }
        d
    }
}

/// The tolerance, the next f64 above it, and values either side.
fn near_tolerance() -> impl Strategy<Value = f64> {
    prop::sample::select(vec![
        5e-8,
        7e-8,
        7.07e-8,
        7.1e-8,
        9e-8,
        9.9e-8,
        1e-7,
        1.0000000000000001e-7,
        1.01e-7,
        1.5e-7,
        2e-7,
        2.1e-7,
        3e-7,
        1e-6,
    ])
}

fn rounded() -> impl Strategy<Value = Piece> {
    let side = prop_oneof![
        3 => prop::sample::select(vec![1.0, 7.0, 20.0, 1e-3]),
        1 => near_tolerance().prop_map(|t| 2.0 * t),
        1 => 1e-3f64..20.0,
    ];
    let radius = prop_oneof![
        2 => Just(0.0),
        3 => near_tolerance(),
        1 => prop::sample::select(vec![1e-3, 0.5, 2.0]),
    ];
    (
        side.clone(),
        side,
        [radius.clone(), radius.clone(), radius.clone(), radius],
        any::<[bool; 4]>(),
    )
        .prop_map(|(w, h, r, flip)| {
            // A corner's arc takes at most 0.4 of the shorter side, so every side keeps a
            // line.
            let most = 0.4 * w.min(h);
            Piece::Rounded {
                w,
                h,
                r: r.map(|r: f64| r.min(most)),
                flip,
            }
        })
}

fn d_shape() -> impl Strategy<Value = Piece> {
    let r = prop::sample::select(vec![1e-7, 1.5e-7, 3e-7, 1.0, 10.0]);
    let half = prop_oneof![
        3 => prop::sample::select(vec![
            1e-8,
            2.5e-8,
            5e-8,
            1e-7,
            1e-3,
            0.5,
            PI / 2.0,
            PI / 2.0 + 1e-9,
            PI - 1e-3,
            PI - 1e-8,
        ]),
        1 => 1e-9f64..(PI - 1e-9),
    ];
    // Mostly on its circle; else at the tolerance either side, or well off.
    let off = prop_oneof![
        6 => Just(0.0f64),
        1 => prop::sample::select(vec![5e-8, -5e-8, 1e-7, 2e-7, -2e-7, 1e-6, -1e-6, 1e-5]),
    ];
    (r, half, any::<bool>(), any::<bool>(), off).prop_map(|(r, half, big, flip, off)| {
        Piece::D {
            r,
            half,
            big,
            flip,
            // Never through the centre.
            off: f64::max(off, -r / 2.0),
        }
    })
}

fn circle() -> impl Strategy<Value = Piece> {
    prop_oneof![
        2 => near_tolerance().prop_map(|t| Piece::Circle { r: 2.0 * t }),
        1 => prop::sample::select(vec![2e-7, 2.0000000000000003e-7, 1.0, 10.0])
            .prop_map(|r| Piece::Circle { r }),
    ]
}

fn holed() -> impl Strategy<Value = Piece> {
    (
        prop::sample::select(vec![1.0, 4.0, 20.0]),
        prop::sample::select(vec![1.0, 3.0, 20.0]),
        prop_oneof![near_tolerance(), Just(1e-3), Just(0.25)],
    )
        .prop_map(|(w, h, gap)| Piece::Holed { w, h, gap })
}

/// One to four pieces, turned and moved, on any plane.
pub fn hostile() -> impl Strategy<Value = Hostile> {
    let piece = prop_oneof![
        3 => rounded(),
        3 => d_shape(),
        1 => circle(),
        2 => holed(),
    ];
    let reach = prop::sample::select(vec![0.0, -50.0, 1e3, 1e5, 999_800.0]);
    let angle = prop_oneof![
        3 => Just(0.0),
        1 => prop::sample::select(vec![PI / 2.0, 1e-9, 0.3, -2.0]),
        1 => -PI..PI,
    ];
    let plane = prop::sample::select(Plane::ALL.to_vec());
    (
        prop::collection::vec(piece, 1..=4),
        reach.clone(),
        reach,
        angle,
        plane,
    )
        .prop_map(|(pieces, x, y, angle, plane)| Hostile {
            pieces,
            origin: [x, y],
            angle,
            plane,
        })
}
