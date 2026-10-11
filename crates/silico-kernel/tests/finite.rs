//! Geometry that isn't finite is refused when a B-rep is read (`from_brep`, and the text
//! reader `from_text_brep`), naming the kind of element (docs/roadmap/m1-kernel.md, M1.9).
//! Each kind of geometry the check walks is written by FreeCAD as a text B-rep, with a
//! value of its own in each field, then spoiled in one field at a time: `nan` in its place.
//! Kinds FreeCAD doesn't write (stored meshes and polygons, pcurves other than lines and
//! circles) are written here, in OCCT's text format.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;

const VERTEX: &str = "B-rep has a vertex whose point or tolerance isn't finite";
const EDGE: &str = "B-rep has an edge whose curve, parameter range, or tolerance isn't finite";
const FACE: &str = "B-rep has a face whose surface, mesh, or tolerance isn't finite";
const PLACED: &str = "B-rep places a shape by a transformation whose values aren't finite";
const SCALED: &str = "B-rep places a shape by a transformation that mirrors or scales";

/// Tolerances and weights are spoiled with infinity: OCCT's reader drops a NaN in either
/// itself (a tolerance only grows, and NaN compares false; a curve whose weights don't
/// differ isn't rational, so its weights go), but keeps infinity.
///
/// One shape of each kind, as FreeCAD writes it (`exportBrepToString`).
const FIXTURES: &str = r#"import json, Part, FreeCAD
V = FreeCAD.Vector
out = {}
def add(name, shape):
    out[name] = shape.exportBrepToString()
add("vertex", Part.Vertex(1.125, 2.25, 3.375))
add("line", Part.makeLine(V(0, 0, 0), V(1.5, 0, 0)))
add("plane", Part.makePlane(2.5, 3.5))
add("cylinder", Part.makeCylinder(5.5, 7.25))
add("sphere", Part.makeSphere(4.75))
add("torus", Part.makeTorus(9.25, 2.125))
cone = Part.Cone(); cone.Radius = 3.5; cone.SemiAngle = 0.34375
add("cone", cone.toShape(0, 3, 0, 2))
add("circle", Part.makeCircle(2.625))
e = Part.Ellipse(V(0, 0, 0), 3.125, 1.875); add("ellipse", e.toShape())
h = Part.Hyperbola(); h.MajorRadius = 2.375; h.MinorRadius = 1.125; add("hyperbola", h.toShape(-1, 1))
p = Part.Parabola(); p.Focal = 1.625; add("parabola", p.toShape(-1, 1))
b = Part.BezierCurve(); b.setPoles([V(0, 0, 0), V(1.3125, 2, 0), V(3, 0, 0)]); b.setWeight(2, 1.4375); add("bezier_curve", b.toShape())
c = Part.BSplineCurve(); c.buildFromPolesMultsKnots([V(0, 0, 0), V(1.5625, 1, 0), V(2, 2, 0), V(3, 0, 0)], [3, 1, 3], [0.6875, 1.21875, 2.5], False, 2, [1, 1.09375, 1, 1]); add("bspline_curve", c.toShape())
add("offset_curve", Part.OffsetCurve(c, 0.59375, V(0, 0, 1)).toShape())
bz = Part.BezierSurface(); bz.setPole(1, 1, V(0.15625, 0, 0)); bz.setWeight(2, 2, 1.46875); add("bezier_surface", bz.toShape())
bs = Part.BSplineSurface()
bs.buildFromPolesMultsKnots([[V(0, 0, 0), V(0, 1.84375, 0)], [V(1, 0, 0), V(1, 1, 0.25)]], [2, 2], [2, 2], [0.21875, 1], [0.28125, 1], False, False, 1, 1, [[1, 1.40625], [1, 1]])
add("bspline_surface", bs.toShape())
add("offset_surface", Part.OffsetSurface(Part.Plane(), 0.78125).toShape(0, 1, 0, 1))
add("extrusion_surface", Part.SurfaceOfExtrusion(c, V(0, 0, 1)).toShape(0.6875, 2.5, 0, 1))
add("revolution_surface", Part.SurfaceOfRevolution(Part.LineSegment(V(1, 0, 0), V(1, 0, 1)), V(0.46875, 0, 0), V(0, 0, 1)).toShape(0, 3, 0, 1))
add("trimmed_surface", Part.Face(Part.RectangularTrimmedSurface(Part.Plane(), 0.53125, 1.5, 0.5, 1.5)))
moved = Part.makeBox(1, 1, 1); moved.Placement = FreeCAD.Placement(V(0.71875, 0, 0), FreeCAD.Rotation()); add("moved_box", moved)
print("SILICO_ORACLE " + json.dumps(out))
"#;

const SECTIONS: [&str; 8] = [
    "Locations",
    "Curve2ds",
    "Curves",
    "Polygon3D",
    "PolygonOnTriangulations",
    "Surfaces",
    "Triangulations",
    "TShapes",
];

/// The byte range of `section`'s contents in `text` (after its header line, up to the next
/// section's).
fn span(text: &str, section: &str) -> (usize, usize) {
    let header = |name: &str| {
        text.match_indices('\n')
            .map(|(i, _)| i + 1)
            .find(|&i| text[i..].starts_with(&format!("{name} ")))
    };
    let start = header(section).unwrap();
    let start = start + text[start..].find('\n').unwrap() + 1;
    let end = SECTIONS
        .iter()
        .filter_map(|s| header(s))
        .filter(|&i| i > start)
        .min()
        .unwrap_or(text.len());
    (start, end)
}

/// `text` with every whole-word `token` in `section` replaced by `with`.
fn spoil_with(text: &str, section: &str, token: &str, with: &str) -> String {
    let (start, end) = span(text, section);
    let body: Vec<String> = text[start..end]
        .split('\n')
        .map(|line| {
            line.split(' ')
                .map(|t| if t == token { with } else { t })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_ne!(body.join("\n"), text[start..end], "no {token} in {section}");
    format!("{}{}{}", &text[..start], body.join("\n"), &text[end..])
}

fn spoil(text: &str, section: &str, token: &str) -> String {
    spoil_with(text, section, token, "nan")
}

/// `text` with the first line of `section` replaced by `entry`.
fn first_entry(text: &str, section: &str, entry: &str) -> String {
    let (start, _) = span(text, section);
    let end = start + text[start..].find('\n').unwrap();
    format!("{}{entry}{}", &text[..start], &text[end..])
}

/// `text` with `section`'s single entry `body` (it had none), and `line` inserted in
/// `TShapes` after the first `marker` line and `skip` lines more (to point a face or an
/// edge at it).
fn graft(text: &str, section: &str, body: &str, marker: &str, skip: usize, line: &str) -> String {
    let header = format!("\n{section} 0\n");
    let at = text.find(&header).unwrap() + 1;
    let text = format!(
        "{}{section} 1\n{body}\n{}",
        &text[..at],
        &text[at + header.len() - 1..]
    );
    let (start, _) = span(&text, "TShapes");
    let mut at = start + text[start..].find(&format!("\n{marker}\n")).unwrap() + 1;
    for _ in 0..=skip {
        at += text[at..].find('\n').unwrap() + 1;
    }
    format!("{}{line}\n{}", &text[..at], &text[at..])
}

/// The trimmed surface's text with `nan` in its basis plane: OCCT's reader overflows the
/// stack on bounds that aren't finite (Geom_RectangularTrimmedSurface's trim recurses).
fn trimmed_basis(text: &str) -> String {
    let (start, end) = span(text, "Surfaces");
    let at = start + text[start..end].find("\n1 0 ").unwrap();
    format!("{}\n1 nan {}", &text[..at], &text[at + 5..])
}

/// `text` with a location that scales by 2, and the `field`th number of the line `skip`
/// lines after the first `marker` line in `TShapes` (a face's or an edge's location
/// index) pointing at it.
fn scaled(text: &str, marker: &str, skip: usize, field: usize) -> String {
    let header = "\nLocations 0\n";
    let at = text.find(header).unwrap() + 1;
    let text = format!(
        "{}Locations 1\n1\n2 0 0 0\n0 2 0 0\n0 0 2 0\n{}",
        &text[..at],
        &text[at + header.len() - 1..]
    );
    let (start, _) = span(&text, "TShapes");
    let mut line = start + text[start..].find(&format!("\n{marker}\n")).unwrap() + marker.len() + 2;
    for _ in 0..skip {
        line += text[line..].find('\n').unwrap() + 1;
    }
    let end = line + text[line..].find('\n').unwrap();
    let mut fields: Vec<&str> = text[line..end].split_whitespace().collect();
    fields[field] = "1";
    format!("{}{}{}", &text[..line], fields.join(" "), &text[end..])
}

/// `text` with `from` (which must be in it) replaced by `to`, once.
fn swap(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "no {from:?}");
    text.replacen(from, to, 1)
}

/// `text` with one location, which scales by 2 (it had none).
fn with_scale(text: &str) -> String {
    swap(
        text,
        "\nLocations 0\n",
        "\nLocations 1\n1\n2 0 0 0\n0 2 0 0\n0 0 2 0\n",
    )
}

fn refused(text: &str) -> String {
    silico_kernel::from_text_brep(text.as_bytes())
        .err()
        .map(|e| e.message().to_owned())
        .unwrap_or_else(|| "read".to_owned())
}

#[test]
fn every_kind_of_geometry_that_isnt_finite_is_refused_naming_it() {
    let written: BTreeMap<String, String> =
        serde_json::from_value(oracle::freecad(FIXTURES)).unwrap();
    for (name, text) in &written {
        assert_eq!(refused(text), "read", "{name}, unspoiled");
    }
    let f = |name: &str| written[name].as_str();
    // A face of the plane with a stored mesh, and the line with a stored polygon: both
    // read as written, then spoiled.
    let meshed = graft(
        f("plane"),
        "Triangulations",
        "4 2 0 0\n0 0 0 2.5 0 0 2.5 3.5 0 0 3.5 0\n1 2 3 1 3 4",
        "Fa",
        1,
        "2 1",
    );
    let polygon = graft(
        f("line"),
        "Polygon3D",
        "2 0\n0.1\n0 0 0 1.5 0 0",
        "Ed",
        1,
        "5  1 0",
    );
    assert_eq!(refused(&meshed), "read");
    assert_eq!(refused(&polygon), "read");
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a vertex's point",
            spoil(f("vertex"), "TShapes", "3.375"),
            VERTEX,
        ),
        (
            "a vertex's tolerance",
            spoil_with(f("vertex"), "TShapes", "1e-07", "inf"),
            VERTEX,
        ),
        (
            "an edge's tolerance",
            spoil_with(f("line"), "TShapes", "1e-07", "inf"),
            EDGE,
        ),
        ("an edge's range", spoil(f("line"), "TShapes", "1.5"), EDGE),
        (
            "a face's tolerance",
            spoil_with(f("plane"), "TShapes", "1e-07", "inf"),
            FACE,
        ),
        ("a line", spoil(f("plane"), "Curves", "2.5"), EDGE),
        ("a plane", spoil(f("plane"), "Surfaces", "0"), FACE),
        ("a line on a face", spoil(f("plane"), "Curve2ds", "0"), EDGE),
        ("a circle", spoil(f("circle"), "Curves", "2.625"), EDGE),
        ("an ellipse", spoil(f("ellipse"), "Curves", "1.875"), EDGE),
        (
            "a hyperbola",
            spoil(f("hyperbola"), "Curves", "2.375"),
            EDGE,
        ),
        ("a parabola", spoil(f("parabola"), "Curves", "1.625"), EDGE),
        (
            "a Bezier curve's pole",
            spoil(f("bezier_curve"), "Curves", "1.3125"),
            EDGE,
        ),
        (
            "a Bezier curve's weight",
            spoil_with(f("bezier_curve"), "Curves", "1.4375", "inf"),
            EDGE,
        ),
        (
            "a B-spline's pole",
            spoil(f("bspline_curve"), "Curves", "1.5625"),
            EDGE,
        ),
        (
            "a B-spline's weight",
            spoil_with(f("bspline_curve"), "Curves", "1.09375", "inf"),
            EDGE,
        ),
        (
            "a B-spline's knot",
            spoil(f("bspline_curve"), "Curves", "1.21875"),
            EDGE,
        ),
        (
            "an offset curve",
            spoil(f("offset_curve"), "Curves", "0.59375"),
            EDGE,
        ),
        // Its basis: OCCT's reader never returns from a trim that isn't finite
        // (Geom_TrimmedCurve's SetTrim and Reverse call each other forever).
        (
            "a trimmed curve",
            swap(
                f("trimmed_surface"),
                "\n1 0.53125 0 0 0 1 0",
                "\n1 nan 0 0 0 1 0",
            ),
            EDGE,
        ),
        (
            "a circle on a face",
            spoil(f("cylinder"), "Curve2ds", "5.5"),
            EDGE,
        ),
        ("a cylinder", spoil(f("cylinder"), "Surfaces", "5.5"), FACE),
        ("a sphere", spoil(f("sphere"), "Surfaces", "4.75"), FACE),
        (
            "a torus's major radius",
            spoil(f("torus"), "Surfaces", "9.25"),
            FACE,
        ),
        (
            "a torus's minor radius",
            spoil(f("torus"), "Surfaces", "2.125"),
            FACE,
        ),
        ("a cone's radius", spoil(f("cone"), "Surfaces", "3.5"), FACE),
        (
            "a cone's angle",
            spoil(f("cone"), "Surfaces", "0.34375"),
            FACE,
        ),
        (
            "a Bezier surface's pole",
            spoil(f("bezier_surface"), "Surfaces", "0.15625"),
            FACE,
        ),
        (
            "a Bezier surface's weight",
            spoil_with(f("bezier_surface"), "Surfaces", "1.46875", "inf"),
            FACE,
        ),
        (
            "a B-spline surface's pole",
            spoil(f("bspline_surface"), "Surfaces", "1.84375"),
            FACE,
        ),
        (
            "a B-spline surface's weight",
            spoil_with(f("bspline_surface"), "Surfaces", "1.40625", "inf"),
            FACE,
        ),
        (
            "a B-spline surface's u knot",
            spoil(f("bspline_surface"), "Surfaces", "0.21875"),
            FACE,
        ),
        (
            "a B-spline surface's v knot",
            spoil(f("bspline_surface"), "Surfaces", "0.28125"),
            FACE,
        ),
        (
            "an offset surface",
            spoil(f("offset_surface"), "Surfaces", "0.78125"),
            FACE,
        ),
        (
            "a surface of extrusion",
            spoil(f("extrusion_surface"), "Surfaces", "1.5625"),
            FACE,
        ),
        (
            "a surface of revolution",
            spoil(f("revolution_surface"), "Surfaces", "0.46875"),
            FACE,
        ),
        (
            "a trimmed surface",
            trimmed_basis(f("trimmed_surface")),
            FACE,
        ),
        (
            "a face's surface, scaled",
            scaled(f("plane"), "Fa", 0, 3),
            SCALED,
        ),
        (
            "an edge's curve, scaled",
            scaled(f("line"), "Ed", 1, 2),
            SCALED,
        ),
        (
            "a stored polygon, scaled",
            with_scale(&graft(
                f("line"),
                "Polygon3D",
                "2 0\n0.1\n0 0 0 1.5 0 0",
                "Ed",
                1,
                "5  1 1",
            )),
            SCALED,
        ),
        (
            "a plane's direction",
            swap(
                f("plane"),
                "\n1 0 0 0 0 0 1 1 0 -0 -0 1 0",
                "\n1 0 0 0 0 0 nan 1 0 -0 -0 1 0",
            ),
            FACE,
        ),
        (
            "a line's direction",
            swap(f("line"), "\n1 0 0 0 1 0 0", "\n1 0 0 0 nan 0 0"),
            EDGE,
        ),
        (
            "an offset curve's basis",
            swap(f("offset_curve"), " 1.5625 ", " nan "),
            EDGE,
        ),
        (
            "an offset curve's direction",
            swap(f("offset_curve"), "\n0 0 1 \n", "\n0 0 nan \n"),
            EDGE,
        ),
        (
            "an offset surface's basis",
            swap(
                f("offset_surface"),
                "\n1 0 0 0 0 0 1 1 0 0 0 1 0",
                "\n1 nan 0 0 0 0 1 1 0 0 0 1 0",
            ),
            FACE,
        ),
        (
            "a surface of extrusion's direction",
            swap(f("extrusion_surface"), "6 0 0 1", "6 0 0 nan"),
            FACE,
        ),
        (
            "an ellipse's major radius",
            swap(f("ellipse"), " 3.125 1.875", " nan 1.875"),
            EDGE,
        ),
        (
            "a hyperbola's minor radius",
            swap(f("hyperbola"), " 1.125", " nan"),
            EDGE,
        ),
        (
            "a stored mesh's surface parameters",
            graft(
                f("plane"),
                "Triangulations",
                "4 2 1 0\n0 0 0 2.5 0 0 2.5 3.5 0 0 3.5 0\n0 0 1 0 1 1 0 nan\n1 2 3 1 3 4",
                "Fa",
                1,
                "2 1",
            ),
            FACE,
        ),
        (
            "a location",
            spoil(f("moved_box"), "Locations", "0.71875"),
            PLACED,
        ),
        (
            "a stored mesh",
            spoil(&meshed, "Triangulations", "3.5"),
            FACE,
        ),
        (
            "a stored polygon",
            spoil(&polygon, "Polygon3D", "1.5"),
            EDGE,
        ),
    ];
    // Curves on a face of the kinds FreeCAD doesn't write, in place of the plane's first.
    let curves_on_faces = [
        ("an ellipse on a face", "3 0 0 1 0 0 1 2 nan"),
        ("an ellipse on a face's major radius", "3 0 0 1 0 0 1 nan 1"),
        (
            "a hyperbola on a face's minor radius",
            "5 0 0 1 0 0 1 1 nan",
        ),
        ("a Bezier curve on a face's weight", "6 1 1 0 0 inf 1 0 1"),
        (
            "a B-spline on a face's weight",
            "7 1 0 1 2 2 0 0 inf 1 0 1 0 2 1 2",
        ),
        (
            "a B-spline on a face's knot",
            "7 0 0 1 2 2 0 0 1 0 0 2 nan 2",
        ),
        ("an offset curve on a face's basis", "9 1\n1 nan 0 1 0"),
        ("a parabola on a face", "4 0 0 1 0 0 1 nan"),
        ("a hyperbola on a face", "5 0 0 1 0 0 1 nan 1"),
        ("a Bezier curve on a face", "6 0 1 0 0 1 nan"),
        ("a B-spline on a face", "7 0 0 1 2 2 0 0 1 nan 0 2 1 2"),
        ("a trimmed curve on a face", "8 0 1\n1 nan 0 1 0"),
        ("an offset curve on a face", "9 nan\n1 0 0 1 0"),
    ];
    let mut wrong = Vec::new();
    let all = cases.into_iter().chain(
        curves_on_faces
            .into_iter()
            .map(|(what, entry)| (what, first_entry(f("plane"), "Curve2ds", entry), EDGE)),
    );
    for (what, text, says) in all {
        let got = refused(&text);
        if !got.starts_with(says) {
            wrong.push(format!("{what}: {got}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_shape_too_large_to_mesh_says_so() {
    // Finite, but 1e200 wide: its extent overflows. Meshing it is refused as the shape's
    // fault, not the deflection's.
    let written: BTreeMap<String, String> =
        serde_json::from_value(oracle::freecad(FIXTURES)).unwrap();
    let mut far = written["plane"].clone();
    for section in ["Curves", "Curve2ds", "TShapes"] {
        far = spoil_with(&far, section, "2.5", "1e200");
    }
    let shape = silico_kernel::from_text_brep(far.as_bytes()).unwrap();
    let err = silico_kernel::tessellate(&shape, 1.0).unwrap_err();
    assert!(err.message().contains("extent is too large"), "{err:?}");
}

#[test]
fn the_converter_writes_the_shape_it_reads() {
    // silico-brep-convert: a part's text in, Silico's B-rep of the same shape out.
    use std::io::Write as _;
    let written: BTreeMap<String, String> =
        serde_json::from_value(oracle::freecad(FIXTURES)).unwrap();
    for name in ["cylinder", "bspline_surface", "moved_box"] {
        let text = &written[name];
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_silico-brep-convert"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{name}");
        let converted = silico_kernel::from_brep(&out.stdout).unwrap();
        let read = silico_kernel::from_text_brep(text.as_bytes()).unwrap();
        assert_eq!(
            silico_kernel::to_brep(&converted).unwrap(),
            silico_kernel::to_brep(&read).unwrap(),
            "{name}"
        );
        let s = silico_kernel::summarize(&converted).unwrap();
        assert_eq!(s, silico_kernel::summarize(&read).unwrap(), "{name}");
    }
}
