//! Each face's orientation, as `describe` reports it (docs/design/m6.0-patterns.md, section
//! 4.2, *Each face's orientation*): a raw fact, OCCT's `TopAbs_Orientation` of the face as
//! `TopExp::MapShapes` first meets it, which a pattern reads a face's outside from. Each
//! expected value is FreeCAD's (`Face.Orientation` of each of `Shape.Faces`, in the same
//! map's order), or the fixtures' own (tests/fixtures/brep/README.md).

use super::brep_fixture::fixture;
use super::oracle;

use silico_kernel::{Primitive, describe_whole, from_brep, from_text_brep, make};
use silico_protocol::FaceOrientation;

/// FreeCAD's word for each orientation.
fn word(orientation: FaceOrientation) -> &'static str {
    match orientation {
        FaceOrientation::Forward => "Forward",
        FaceOrientation::Reversed => "Reversed",
        FaceOrientation::Internal => "Internal",
        FaceOrientation::External => "External",
    }
}

#[test]
fn a_boxs_faces_are_forward_or_reversed_as_occt_holds_them() {
    // FreeCAD's `Part.makeBox` is OCCT's `BRepPrimAPI_MakeBox`, as Silico's box is: each of
    // its six faces' orientation in FreeCAD's `Faces`, the same map's order.
    let freecad = oracle::freecad(
        r#"
import json, Part
print("SILICO_ORACLE " + json.dumps([f.Orientation for f in Part.makeBox(10, 20, 30).Faces]))
"#,
    );
    let want: Vec<&str> = freecad
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_str().unwrap())
        .collect();
    let shape = make(&Primitive::Box {
        dx: 10.0,
        dy: 20.0,
        dz: 30.0,
    })
    .unwrap();
    let got: Vec<&str> = describe_whole(&shape)
        .unwrap()
        .faces
        .iter()
        .map(|face| word(face.orientation))
        .collect();
    assert_eq!(got, want);
    // Both kinds, as a box has: each is a face with an outside.
    assert!(
        got.contains(&"Forward") && got.contains(&"Reversed"),
        "{got:?}"
    );
}

#[test]
fn a_fuse_that_leaves_a_face_inside_its_solid_reports_it_internal() {
    // A 10 × 20 × 30 box fused with a square inside it at x = 5 by FreeCAD 1.1.3's
    // generalFuse, which keeps the square as an INTERNAL face of the box, valid (as the
    // export's membrane is): its seventh face is internal, the box's six forward or reversed.
    let text = oracle::freecad(
        r#"
import json, FreeCAD, Part
V = FreeCAD.Vector
m = Part.Face(Part.makePolygon([V(5, 5, 5), V(5, 15, 5), V(5, 15, 25), V(5, 5, 25), V(5, 5, 5)]))
fused, _ = Part.makeBox(10, 20, 30).generalFuse([m])
s = fused.Solids[0]
assert s.isValid()
print("SILICO_ORACLE " + json.dumps([s.exportBrepToString(), [f.Orientation for f in s.Faces]]))
"#,
    );
    let [brep, orientations] = text.as_array().unwrap().as_slice() else {
        panic!("{text}");
    };
    let want: Vec<&str> = orientations
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_str().unwrap())
        .collect();
    assert_eq!(want.iter().filter(|o| **o == "Internal").count(), 1);
    let shape = from_text_brep(brep.as_str().unwrap().as_bytes()).unwrap();
    let got: Vec<&str> = describe_whole(&shape)
        .unwrap()
        .faces
        .iter()
        .map(|face| word(face.orientation))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn a_box_whose_face_is_internal_or_external_in_its_shell_reports_it_so() {
    // The fixtures' 10 × 20 × 30 box whose face at x = 0 is INTERNAL, or EXTERNAL, in its
    // shell (tests/fixtures/brep/README.md): that face, its centre of area on x = 0, has no
    // outside; its other five are forward or reversed.
    for (name, orientation) in [
        ("box_internal_face1.bin", FaceOrientation::Internal),
        ("box_external_face1.bin", FaceOrientation::External),
    ] {
        let shape = from_brep(&fixture(name)).unwrap();
        let faces = describe_whole(&shape).unwrap().faces;
        assert_eq!(faces.len(), 6, "{name}");
        for face in &faces {
            if face.centroid[0].abs() < 1e-9 {
                assert_eq!(face.orientation, orientation, "{name}");
                assert!(!face.orientation.has_outside(), "{name}");
            } else {
                assert!(
                    face.orientation.has_outside(),
                    "{name}: {:?}",
                    face.orientation
                );
            }
        }
        assert_eq!(
            faces
                .iter()
                .filter(|face| face.orientation == orientation)
                .count(),
            1,
            "{name}"
        );
    }
}
