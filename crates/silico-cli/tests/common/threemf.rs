//! A 3MF archive as Silico writes it, read back in the CLI's tests from its bytes: its
//! stored entries, its model's objects as written (coordinates read as the 32-bit floats a
//! slicer reads), and the checks M4.8 promises of a printable mesh, made here from the
//! triangles alone (docs/ports/mesh-export.md, *A printable mesh*).

use std::collections::BTreeMap;

/// An object of a 3MF model: its type, its name, its vertices, and its triangles, as
/// written.
pub struct Object {
    pub kind: String,
    pub name: String,
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[usize; 3]>,
}

/// The archive's entries, each name and its data, from the local headers (APPNOTE 6.3.3,
/// 4.3.7), each stored, as Silico writes them.
pub fn entries(archive: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let u16_at = |at: usize| usize::from(u16::from_le_bytes([archive[at], archive[at + 1]]));
    let u32_at = |at: usize| u32::from_le_bytes(archive[at..at + 4].try_into().unwrap());
    let mut out = BTreeMap::new();
    let mut at = 0;
    while u32_at(at) == 0x0403_4b50 {
        assert_eq!(u16_at(at + 8), 0, "stored");
        let size = u32_at(at + 18) as usize;
        assert_eq!(u32_at(at + 22) as usize, size, "stored: as long as written");
        let (name_len, extra) = (u16_at(at + 26), u16_at(at + 28));
        let name = String::from_utf8(archive[at + 30..at + 30 + name_len].to_vec()).unwrap();
        let data = at + 30 + name_len + extra;
        out.insert(name, archive[data..data + size].to_vec());
        at = data + size;
    }
    // The central directory follows the last entry.
    assert_eq!(u32_at(at), 0x0201_4b50);
    out
}

/// A tag's attribute `name`, from the tag's text after its name.
fn attribute(tag: &str, name: &str) -> String {
    let tag = format!(" {tag}");
    let key = format!(" {name}=\"");
    let at = tag
        .find(&key)
        .unwrap_or_else(|| panic!("no {name} in {tag}"))
        + key.len();
    tag[at..].split('"').next().unwrap().to_owned()
}

/// The objects of the archive's model (`3D/3dmodel.model`, in millimetres), in order.
pub fn model(archive: &[u8]) -> Vec<Object> {
    let entries = entries(archive);
    let model = String::from_utf8(entries["3D/3dmodel.model"].clone()).unwrap();
    let header = &model[model.find("<model ").expect("a model in the archive")..];
    assert_eq!(attribute(header, "unit"), "millimeter");
    model
        .split("<object ")
        .skip(1)
        .map(|object| Object {
            kind: attribute(object, "type"),
            name: attribute(object, "name"),
            vertices: object
                .split("<vertex ")
                .skip(1)
                .map(|tag| {
                    ["x", "y", "z"].map(|k| attribute(tag, k).parse::<f32>().unwrap().into())
                })
                .collect(),
            triangles: object
                .split("<triangle ")
                .skip(1)
                .map(|tag| ["v1", "v2", "v3"].map(|k| attribute(tag, k).parse().unwrap()))
                .collect(),
        })
        .collect()
}

/// The volume `o`'s triangles enclose, from the coordinates as written: the sum of each
/// triangle's signed tetrahedron with the origin, its determinants summed before the one
/// division, so whole-number coordinates give an exact volume.
pub fn volume(o: &Object) -> f64 {
    let determinants: f64 = o
        .triangles
        .iter()
        .map(|&[a, b, c]| {
            let (a, b, c) = (o.vertices[a], o.vertices[b], o.vertices[c]);
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0])
        })
        .sum();
    determinants / 6.0
}

/// Checks `o` is printable as M4.8 promises, from its triangles alone: a model, not a
/// surface; no triangle with a repeated index or two corners at one place; every directed
/// edge on exactly one triangle and its reverse on exactly one other (closed, manifold,
/// consistently oriented); and the volume it encloses, which it gives, positive (facing
/// outward).
pub fn printable(o: &Object) -> f64 {
    assert_eq!(o.kind, "model", "{}", o.name);
    let mut directed: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    for &[a, b, c] in &o.triangles {
        assert!(a != b && b != c && a != c, "a repeated index: {a} {b} {c}");
        let (p, q, s) = (o.vertices[a], o.vertices[b], o.vertices[c]);
        assert!(
            p != q && q != s && p != s,
            "two corners at one place: {a} {b} {c}"
        );
        for edge in [(a, b), (b, c), (c, a)] {
            *directed.entry(edge).or_default() += 1;
        }
    }
    for (&(a, b), &count) in &directed {
        assert_eq!(count, 1, "edge {a}→{b} on {count} triangles one way");
        assert_eq!(
            directed.get(&(b, a)),
            Some(&1),
            "edge {a}→{b} without its reverse: open, or misoriented"
        );
    }
    let volume = volume(o);
    assert!(volume > 0.0, "the mesh faces inward: {volume}");
    volume
}
