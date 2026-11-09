//! The app's icons (the UI prototype's set, docs/design/m5-ui/DESIGN.md, `js/icons.js`): 84
//! strokes on a 20 by 20 grid, each the prototype's SVG body as it wrote it, drawn by
//! egui_extras' SVG loader (resvg, offline). None is Autodesk's or Rerun's art.
//!
//! Each icon is drawn in white, `currentColor` resolving to white, so an [`egui::Image`]'s
//! tint gives it its colour ([`icon`] tints it the text's). The logo's one blue stroke is
//! its own colour, so the logo keeps it only when tinted white. Each comes in the prototype's
//! two sizes, as it strokes them: [`ICON_SIZE`] at 1.7 units (rows, menus) and
//! [`TOOLBAR_ICON_SIZE`] at 1.35 (the toolbar's buttons).
//!
//! The loaders are installed by [`crate::theme::apply`]; with none installed, an icon draws
//! nothing.

use std::borrow::Cow;

use eframe::egui::{self, ImageSource, Vec2, load::Bytes};

use crate::theme;

/// A row's, a menu's, or a tree's icon: `re_ui`'s `small_icon_size`.
pub const ICON_SIZE: f32 = 14.0;

/// A toolbar button's icon (DESIGN.md, *Spacing*: buttons 30 with 20 px icons).
pub const TOOLBAR_ICON_SIZE: f32 = 20.0;

/// Which of the prototype's two sizes, each with its own stroke width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    /// [`ICON_SIZE`], stroked at 1.7 of the grid's 20 units.
    Small,
    /// [`TOOLBAR_ICON_SIZE`], stroked at 1.35.
    Toolbar,
}

impl IconSize {
    /// Its side in points.
    pub const fn points(self) -> f32 {
        match self {
            Self::Small => ICON_SIZE,
            Self::Toolbar => TOOLBAR_ICON_SIZE,
        }
    }
}

/// The icons: each name as `js/icons.js` has it ([`Icon::name`]), and its SVG at each size.
macro_rules! icons {
    ($($variant:ident $name:literal $body:literal,)*) => {
        /// One of the app's icons, by the prototype's name.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Icon {
            $(
                #[doc = concat!("`", $name, "`")]
                $variant,
            )*
        }

        impl Icon {
            /// Every icon, in `js/icons.js`'s order.
            pub const ALL: &[Icon] = &[$(Icon::$variant,)*];

            /// Its name in `js/icons.js`.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Icon::$variant => $name,)*
                }
            }

            /// Its SVG document at `size`: the prototype's body in a 20 by 20 view box,
            /// stroked as the prototype's stylesheet strokes `svg.i` (and `svg.i.lg`).
            pub const fn svg(self, size: IconSize) -> &'static str {
                match (self, size) {
                    $(
                        (Icon::$variant, IconSize::Small) => concat!(
                            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" width="20" height="20" color="#ffffff" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">"##,
                            $body,
                            "</svg>"
                        ),
                        (Icon::$variant, IconSize::Toolbar) => concat!(
                            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" width="20" height="20" color="#ffffff" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round">"##,
                            $body,
                            "</svg>"
                        ),
                    )*
                }
            }

            /// The URI egui caches its image by: one for each size, ending `.svg` so the SVG
            /// loader takes it.
            pub const fn uri(self, size: IconSize) -> &'static str {
                match (self, size) {
                    $(
                        (Icon::$variant, IconSize::Small) =>
                            concat!("bytes://silico/icons/", $name, ".svg"),
                        (Icon::$variant, IconSize::Toolbar) =>
                            concat!("bytes://silico/icons/", $name, "-toolbar.svg"),
                    )*
                }
            }
        }
    };
}

icons! {
    Chevron "chevron" r##"<path d="M8 5l5 5-5 5"/>"##,
    Down "down" r##"<path d="M5 8l5 5 5-5"/>"##,
    Menu "menu" r##"<path d="M3 6h14M3 10h14M3 14h14"/>"##,
    Eye "eye" r##"<path d="M2 10s3-5.5 8-5.5S18 10 18 10s-3 5.5-8 5.5S2 10 2 10z"/><circle cx="10" cy="10" r="2.3"/>"##,
    EyeOff "eyeOff" r##"<path d="M2 10s3-5.5 8-5.5S18 10 18 10s-3 5.5-8 5.5S2 10 2 10z" opacity=".55"/><path d="M3.5 16.5l13-13"/>"##,
    Sketch "sketch" r##"<path d="M2.5 15.5l3.5-8h11.5l-3.5 8z"/><path d="M7 13l2.5-3 2 1.5 2.5-3" />"##,
    Pad "pad" r##"<path d="M3 9l3-3h11l-3 3z" fill="currentColor" fill-opacity=".3"/><path d="M3 9v7h11V9M14 16l3-3V6"/><path d="M8.5 12.5V2.5M6.5 4.5l2-2 2 2"/>"##,
    Pocket "pocket" r##"<path d="M2.5 8l3.5-3.5h11.5L14 8z"/><path d="M2.5 8v8H14V8M14 16l3.5-3.5v-8"/><path d="M7 6.2h5" stroke-dasharray="1.5 1.5"/><path d="M9.5 9.5v4.5M7.5 12l2 2 2-2"/>"##,
    Revolve "revolve" r##"<path d="M5 2.5v15" stroke-dasharray="2 1.6"/><path d="M8 5.5c5 .2 8.5 2 8.5 4.5s-3.5 4.3-8.5 4.5"/><path d="M10 12.6l-2 1.9 2 2"/>"##,
    Groove "groove" r##"<path d="M5 2.5v15" stroke-dasharray="2 1.6"/><path d="M8 5.5c5 .2 8.5 2 8.5 4.5s-3.5 4.3-8.5 4.5"/><path d="M12 8.5v3h3" /><path d="M10 12.6l-2 1.9 2 2"/>"##,
    Fillet "fillet" r##"<path d="M4 3v8a6 6 0 0 0 6 6h7"/><path d="M4 17h3M4 14v3" opacity=".45"/>"##,
    Chamfer "chamfer" r##"<path d="M4 3v8l6 6h7"/><path d="M4 17h3M4 14v3" opacity=".45"/>"##,
    Box "box" r##"<path d="M10 2.5l6.5 3.7v7.6L10 17.5l-6.5-3.7V6.2z"/><path d="M3.5 6.2L10 10l6.5-3.8M10 10v7.5"/>"##,
    Cylinder "cylinder" r##"<ellipse cx="10" cy="5" rx="6" ry="2.4"/><path d="M4 5v10c0 1.3 2.7 2.4 6 2.4s6-1.1 6-2.4V5"/>"##,
    Sphere "sphere" r##"<circle cx="10" cy="10" r="7"/><path d="M3 10c0 1.6 3.1 2.8 7 2.8s7-1.2 7-2.8"/>"##,
    Cone "cone" r##"<path d="M10 2.5L4 15c0 1.3 2.7 2.4 6 2.4s6-1.1 6-2.4z"/>"##,
    Torus "torus" r##"<ellipse cx="10" cy="10" rx="7.5" ry="4.5"/><path d="M6.5 9.6c1.8 1.5 5.2 1.5 7 0"/><path d="M7.5 10.6c1.4-.9 3.6-.9 5 0"/>"##,
    Union "union" r##"<circle cx="7.5" cy="10" r="5" fill="currentColor" fill-opacity=".3"/><circle cx="12.5" cy="10" r="5" fill="currentColor" fill-opacity=".3"/>"##,
    Subtract "subtract" r##"<circle cx="7.5" cy="10" r="5" fill="currentColor" fill-opacity=".3"/><circle cx="12.5" cy="10" r="5" stroke-dasharray="2 1.5"/>"##,
    Intersect "intersect" r##"<circle cx="7.5" cy="10" r="5"/><circle cx="12.5" cy="10" r="5"/><path d="M10 5.7a5 5 0 0 1 0 8.6a5 5 0 0 1 0-8.6z" fill="currentColor" fill-opacity=".45"/>"##,
    Measure "measure" r##"<path d="M2.8 13.5l10.7-10.7 3.7 3.7L6.5 17.2z"/><path d="M6 10.3l1.6 1.6M8.2 8.1l1 1M10.4 5.9l1.6 1.6"/>"##,
    SelFace "selFace" r##"<path d="M10 2.5l6.5 3.7v7.6L10 17.5l-6.5-3.7V6.2z" opacity=".55"/><path d="M3.5 6.2L10 10l6.5-3.8L10 2.5z" fill="currentColor" fill-opacity=".6"/>"##,
    SelEdge "selEdge" r##"<path d="M10 2.5l6.5 3.7v7.6L10 17.5l-6.5-3.7V6.2zM3.5 6.2L10 10l6.5-3.8M10 10v7.5" opacity=".45"/><path d="M10 10v7.5" stroke-width="2.6"/>"##,
    SelVertex "selVertex" r##"<path d="M10 2.5l6.5 3.7v7.6L10 17.5l-6.5-3.7V6.2zM3.5 6.2L10 10l6.5-3.8M10 10v7.5" opacity=".45"/><circle cx="10" cy="10" r="2.4" fill="currentColor"/>"##,
    Body "body" r##"<path d="M10 2.5l6.5 3.7v7.6L10 17.5l-6.5-3.7V6.2z" fill="currentColor" fill-opacity=".25"/><path d="M3.5 6.2L10 10l6.5-3.8M10 10v7.5"/>"##,
    Folder "folder" r##"<path d="M2.5 5.5h5.5l2 2h7.5v8.5h-15z"/>"##,
    Origin "origin" r##"<circle cx="10" cy="10" r="2.2"/><path d="M10 2.5v4.5M10 13v4.5M2.5 10H7M13 10h4.5"/>"##,
    Plane "plane" r##"<path d="M2.5 14.5l4-8h11l-4 8z"/>"##,
    Axis "axis" r##"<path d="M4 16L16 4M11.5 4H16v4.5"/>"##,
    Doc "doc" r##"<path d="M5 2.5h7l3.5 3.5v11.5H5z"/><path d="M12 2.5V6h3.5"/>"##,
    Undo "undo" r##"<path d="M7.5 4.5L4 8l3.5 3.5"/><path d="M4 8h8a4.5 4.5 0 0 1 0 9H9"/>"##,
    Redo "redo" r##"<path d="M12.5 4.5L16 8l-3.5 3.5"/><path d="M16 8H8a4.5 4.5 0 0 0 0 9h3"/>"##,
    Save "save" r##"<path d="M4 3h10l3 3v11H4z"/><path d="M7 3v4h6V3M7 17v-5h6.5v5"/>"##,
    Home "home" r##"<path d="M3 9.5L10 3.5l7 6"/><path d="M5 8v9h10V8"/>"##,
    Help "help" r##"<circle cx="10" cy="10" r="7.5"/><path d="M7.8 7.8a2.3 2.3 0 1 1 3.2 2.1c-.7.3-1 .8-1 1.5v.4"/><circle cx="10" cy="14.3" r=".4" fill="currentColor"/>"##,
    Close "close" r##"<path d="M5.5 5.5l9 9M14.5 5.5l-9 9"/>"##,
    Plus "plus" r##"<path d="M10 4v12M4 10h12"/>"##,
    Gear "gear" r##"<circle cx="10" cy="10" r="2.6"/><path d="M10 2.5v2.2M10 15.3v2.2M2.5 10h2.2M15.3 10h2.2M4.7 4.7l1.6 1.6M13.7 13.7l1.6 1.6M4.7 15.3l1.6-1.6M13.7 6.3l1.6-1.6"/>"##,
    Search "search" r##"<circle cx="8.5" cy="8.5" r="5"/><path d="M12.3 12.3l4.7 4.7"/>"##,
    Warn "warn" r##"<path d="M10 3l7.5 13.5h-15z"/><path d="M10 8v4"/><circle cx="10" cy="14.3" r=".5" fill="currentColor"/>"##,
    Error "error" r##"<circle cx="10" cy="10" r="7.5"/><path d="M10 6v5"/><circle cx="10" cy="13.8" r=".5" fill="currentColor"/>"##,
    Info "info" r##"<circle cx="10" cy="10" r="7.5"/><path d="M10 9v5"/><circle cx="10" cy="6.4" r=".5" fill="currentColor"/>"##,
    Check "check" r##"<path d="M4 10.5l4 4 8-9"/>"##,
    Stop "stop" r##"<rect x="5.5" y="5.5" width="9" height="9" rx="1.2" fill="currentColor"/>"##,
    Rebuild "rebuild" r##"<path d="M16 9.5A6 6 0 0 0 5.2 6.4M4 10.5a6 6 0 0 0 10.8 3.1"/><path d="M5 3v3.6h3.6M15 17v-3.6h-3.6"/>"##,
    Lock "lock" r##"<rect x="4.5" y="9" width="11" height="8" rx="1.5"/><path d="M7 9V6.5a3 3 0 0 1 6 0V9"/>"##,
    Orbit "orbit" r##"<ellipse cx="10" cy="10" rx="7.5" ry="3.5"/><circle cx="10" cy="10" r="2" fill="currentColor"/><path d="M15 5.5l2.3 1.2-.6 2.4"/>"##,
    Pan "pan" r##"<path d="M10 2.5v15M2.5 10h15M10 2.5L8 4.5M10 2.5l2 2M10 17.5l-2-2M10 17.5l2-2M2.5 10l2-2M2.5 10l2 2M17.5 10l-2-2M17.5 10l-2 2"/>"##,
    Zoom "zoom" r##"<circle cx="8.5" cy="8.5" r="5"/><path d="M12.3 12.3l4.7 4.7M6.5 8.5h4M8.5 6.5v4"/>"##,
    Fit "fit" r##"<path d="M3 7V3h4M13 3h4v4M17 13v4h-4M7 17H3v-4"/><rect x="7" y="7" width="6" height="6" rx="1"/>"##,
    LookAt "lookAt" r##"<path d="M2.5 14.5l4-8h11l-4 8z"/><path d="M10 10.5V2.5M8 4.5l2-2 2 2" />"##,
    Display "display" r##"<path d="M10 2.5l6.5 3.7v7.6L10 17.5l-6.5-3.7V6.2z" fill="currentColor" fill-opacity=".3"/><path d="M3.5 6.2L10 10l6.5-3.8M10 10v7.5"/>"##,
    Grid "grid" r##"<path d="M3 3h14v14H3zM3 7.7h14M3 12.3h14M7.7 3v14M12.3 3v14"/>"##,
    Line "line" r##"<path d="M4.5 15.5l11-11"/><circle cx="4.5" cy="15.5" r="1.4" fill="currentColor"/><circle cx="15.5" cy="4.5" r="1.4" fill="currentColor"/>"##,
    Rect "rect" r##"<rect x="3.5" y="5.5" width="13" height="9"/><circle cx="3.5" cy="5.5" r="1.2" fill="currentColor"/><circle cx="16.5" cy="14.5" r="1.2" fill="currentColor"/>"##,
    Circle "circle" r##"<circle cx="10" cy="10" r="6.5"/><path d="M10 10h6.5" opacity=".5"/><circle cx="10" cy="10" r="1.1" fill="currentColor"/>"##,
    Arc "arc" r##"<path d="M3.5 15A7.5 7.5 0 0 1 16.5 9"/><circle cx="3.5" cy="15" r="1.3" fill="currentColor"/><circle cx="16.5" cy="9" r="1.3" fill="currentColor"/>"##,
    Point "point" r##"<circle cx="10" cy="10" r="2" fill="currentColor"/><path d="M10 3.5v3M10 13.5v3M3.5 10h3M13.5 10h3" opacity=".5"/>"##,
    Trim "trim" r##"<circle cx="6" cy="14" r="2.3"/><circle cx="14" cy="14" r="2.3"/><path d="M7.5 12.3L14 3.5M12.5 12.3L6 3.5"/>"##,
    Extend "extend" r##"<path d="M3 13h9" /><path d="M12 13h3" stroke-dasharray="1.5 1.5"/><path d="M17 6v14" /><path d="M13 10.5l2.5 2.5-2.5 2.5"/>"##,
    Split "split" r##"<path d="M3 10h5.5M11.5 10H17"/><circle cx="10" cy="10" r="1.6" fill="currentColor"/><path d="M10 4v2.5M10 13.5V16" opacity=".5"/>"##,
    Construction "construction" r##"<path d="M3.5 16.5l13-13" stroke-dasharray="2.4 2"/>"##,
    Dimension "dimension" r##"<path d="M3 6v8M17 6v8M3 10h14M5.5 7.8L3 10l2.5 2.2M14.5 7.8L17 10l-2.5 2.2"/>"##,
    CHorizontal "cHorizontal" r##"<path d="M3.5 10h13"/><path d="M3.5 7v6M16.5 7v6" opacity=".5"/>"##,
    CVertical "cVertical" r##"<path d="M10 3.5v13"/><path d="M7 3.5h6M7 16.5h6" opacity=".5"/>"##,
    CCoincident "cCoincident" r##"<circle cx="10" cy="10" r="5.5"/><circle cx="10" cy="10" r="1.8" fill="currentColor"/>"##,
    CTangent "cTangent" r##"<circle cx="10" cy="12" r="4.5"/><path d="M3 7.5h14"/>"##,
    CEqual "cEqual" r##"<path d="M4.5 7.5h11M4.5 12.5h11"/>"##,
    CParallel "cParallel" r##"<path d="M6 16L11 4M10 16l5-12"/>"##,
    CPerpendicular "cPerpendicular" r##"<path d="M4 16h12M10 16V4"/>"##,
    CFix "cFix" r##"<path d="M10 3v9"/><path d="M5 12h10M6 15l2-3M10 15l2-3M14 15l2-3"/>"##,
    CMidpoint "cMidpoint" r##"<path d="M3 13h14"/><path d="M10 7.5l3.5 5.5h-7z" fill="currentColor"/>"##,
    CSymmetric "cSymmetric" r##"<path d="M10 3v14" stroke-dasharray="2 1.6"/><path d="M6 6v8M14 6v8M3.5 6H6M3.5 14H6M14 6h2.5M14 14h2.5"/>"##,
    Open "open" r##"<path d="M2.5 5.5h5.5l2 2h7v2.5"/><path d="M2.5 5.5v10.5h12l3-6.5H5.5l-3 6.5"/>"##,
    NewDoc "newDoc" r##"<path d="M5 2.5h7l3.5 3.5v11.5H5z"/><path d="M10 9v6M7 12h6"/>"##,
    Copy "copy" r##"<rect x="6.5" y="6.5" width="10" height="10" rx="1.5"/><path d="M13.5 6.5v-3h-10v10h3"/>"##,
    Edit "edit" r##"<path d="M3.5 16.5l1-4L13 4l3 3-8.5 8.5z"/><path d="M11 6l3 3"/>"##,
    Suppress "suppress" r##"<circle cx="10" cy="10" r="7"/><path d="M5 15L15 5"/>"##,
    Target "target" r##"<circle cx="10" cy="10" r="6.5"/><circle cx="10" cy="10" r="2.5"/>"##,
    Persp "persp" r##"<path d="M2.5 15.5l4-11h7l4 11z"/>"##,
    Ortho "ortho" r##"<rect x="3.5" y="4.5" width="13" height="11"/>"##,
    Agent "agent" r##"<rect x="4" y="6" width="12" height="9.5" rx="2.5"/><path d="M10 3v3"/><circle cx="7.6" cy="10.6" r="1" fill="currentColor"/><circle cx="12.4" cy="10.6" r="1" fill="currentColor"/>"##,
    Diff "diff" r##"<path d="M6 3v8M2 7h8M12 15h6"/><path d="M14 3l-8 14" opacity=".35"/>"##,
    Logo "logo" r##"<path d="M10 2l7 4v8l-7 4-7-4V6z"/><path d="M3 6l7 4 7-4M10 10v8" opacity=".6"/><path d="M10 10l7-4" stroke="#4676ff" stroke-width="1.8"/>"##,
}

impl Icon {
    /// Its image at `size`, exactly that many points square, tinted the text's colour
    /// ([`theme::TEXT_DEFAULT`]); `.tint(…)` gives it another.
    pub fn image(self, size: IconSize) -> egui::Image<'static> {
        egui::Image::new(ImageSource::Bytes {
            uri: Cow::Borrowed(self.uri(size)),
            bytes: Bytes::Static(self.svg(size).as_bytes()),
        })
        .fit_to_exact_size(Vec2::splat(size.points()))
        .tint(theme::TEXT_DEFAULT)
    }
}

impl Icon {
    /// An element's icon, by its kind: the selection filters' (a selection's head, a box of
    /// names' chips); a solid's the face's.
    pub fn of_element(kind: silico_protocol::ElementKind) -> Icon {
        use silico_protocol::ElementKind;
        match kind {
            ElementKind::Face | ElementKind::Solid => Icon::SelFace,
            ElementKind::Edge => Icon::SelEdge,
            ElementKind::Vertex => Icon::SelVertex,
        }
    }

    /// A feature's icon, by its `type` as the document writes it: the prototype's
    /// (`FEATURE_ICON` in `js/app.js`) for a sketch, the sweeps, the fillet and chamfer, and
    /// the box; the toolbar's for the other primitives and the booleans (Union, Subtract,
    /// Intersect); and the box's for a type it doesn't know, as `FEATURE_ICON`'s callers fall
    /// back to it.
    pub fn of_kind(kind: &str) -> Icon {
        match kind {
            "sketch" => Icon::Sketch,
            "pad" => Icon::Pad,
            "pocket" => Icon::Pocket,
            "revolve" => Icon::Revolve,
            "groove" => Icon::Groove,
            "fillet" => Icon::Fillet,
            "chamfer" => Icon::Chamfer,
            "cylinder" => Icon::Cylinder,
            "sphere" => Icon::Sphere,
            "cone" => Icon::Cone,
            "torus" => Icon::Torus,
            "fuse" => Icon::Union,
            "cut" => Icon::Subtract,
            "common" => Icon::Intersect,
            _ => Icon::Box,
        }
    }
}

/// `icon`'s image for a row, a menu, or a tree ([`ICON_SIZE`]), tinted the text's colour.
pub fn icon(icon: Icon) -> egui::Image<'static> {
    icon.image(IconSize::Small)
}

/// `icon`'s image for a toolbar button ([`TOOLBAR_ICON_SIZE`]), tinted the text's colour.
pub fn toolbar_icon(icon: Icon) -> egui::Image<'static> {
    icon.image(IconSize::Toolbar)
}

#[cfg(test)]
mod tests {
    use eframe::egui::load::{ImagePoll, SizeHint};

    use super::*;

    /// `js/icons.js`'s names, in its order, as the prototype has them (copied from its
    /// `P` table, not from this module).
    const PROTOTYPE: [&str; 84] = [
        "chevron",
        "down",
        "menu",
        "eye",
        "eyeOff",
        "sketch",
        "pad",
        "pocket",
        "revolve",
        "groove",
        "fillet",
        "chamfer",
        "box",
        "cylinder",
        "sphere",
        "cone",
        "torus",
        "union",
        "subtract",
        "intersect",
        "measure",
        "selFace",
        "selEdge",
        "selVertex",
        "body",
        "folder",
        "origin",
        "plane",
        "axis",
        "doc",
        "undo",
        "redo",
        "save",
        "home",
        "help",
        "close",
        "plus",
        "gear",
        "search",
        "warn",
        "error",
        "info",
        "check",
        "stop",
        "rebuild",
        "lock",
        "orbit",
        "pan",
        "zoom",
        "fit",
        "lookAt",
        "display",
        "grid",
        "line",
        "rect",
        "circle",
        "arc",
        "point",
        "trim",
        "extend",
        "split",
        "construction",
        "dimension",
        "cHorizontal",
        "cVertical",
        "cCoincident",
        "cTangent",
        "cEqual",
        "cParallel",
        "cPerpendicular",
        "cFix",
        "cMidpoint",
        "cSymmetric",
        "open",
        "newDoc",
        "copy",
        "edit",
        "suppress",
        "target",
        "persp",
        "ortho",
        "agent",
        "diff",
        "logo",
    ];

    #[test]
    fn the_icons_are_the_prototypes_by_name_and_in_its_order() {
        let names: Vec<&str> = Icon::ALL.iter().map(|icon| icon.name()).collect();
        assert_eq!(names, PROTOTYPE);
    }

    #[test]
    fn each_icon_parses_and_draws_white_strokes_at_both_sizes() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        for &icon in Icon::ALL {
            for size in [IconSize::Small, IconSize::Toolbar] {
                let uri = icon.uri(size);
                ctx.include_bytes(uri, Bytes::Static(icon.svg(size).as_bytes()));
                // Drawn at twice its size in pixels, as on a Retina display.
                let side = 2 * size.points() as u32;
                let hint = SizeHint::Size {
                    width: side,
                    height: side,
                    maintain_aspect_ratio: true,
                };
                let image = match ctx.try_load_image(uri, hint) {
                    Ok(ImagePoll::Ready { image }) => image,
                    Ok(ImagePoll::Pending { .. }) => panic!("{} at {size:?}: pending", icon.name()),
                    Err(e) => panic!("{} at {size:?}: {e}", icon.name()),
                };
                assert_eq!(image.size, [side as usize; 2], "{}", icon.name());
                let inked = image.pixels.iter().filter(|p| p.a() > 0).count();
                // Something drawn, and not a filled square: a stroke covers some of the
                // grid, never all of it.
                assert!(
                    inked > 0 && inked < image.pixels.len(),
                    "{} at {size:?}: {inked} of {} pixels inked",
                    icon.name(),
                    image.pixels.len()
                );
                // White (premultiplied: each channel its alpha) so a tint gives the colour;
                // the logo's one blue stroke aside.
                if icon != Icon::Logo {
                    for p in &image.pixels {
                        assert!(
                            p.r() == p.a() && p.g() == p.a() && p.b() == p.a(),
                            "{} at {size:?}: {p:?} isn't white",
                            icon.name()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_two_sizes_stroke_as_the_prototype_does() {
        // svg.i: stroke-width 1.7; svg.i.lg: 1.35 (the prototype's styles.css).
        for &icon in Icon::ALL {
            assert!(icon.svg(IconSize::Small).contains(r#"stroke-width="1.7""#));
            assert!(
                icon.svg(IconSize::Toolbar)
                    .contains(r#"stroke-width="1.35""#)
            );
            assert_ne!(icon.uri(IconSize::Small), icon.uri(IconSize::Toolbar));
        }
    }

    #[test]
    fn an_icons_image_is_its_size_and_tinted_the_texts_colour() {
        for (image, uri, side) in [
            (icon(Icon::Pad), "bytes://silico/icons/pad.svg", 14.0),
            (
                toolbar_icon(Icon::Pad),
                "bytes://silico/icons/pad-toolbar.svg",
                20.0,
            ),
        ] {
            assert_eq!(image.uri(), Some(uri));
            // Whatever room it's given, and whatever the SVG's own size.
            for room in [Vec2::splat(8.0), Vec2::splat(400.0)] {
                assert_eq!(
                    image.calc_size(room, Some(Vec2::splat(20.0))),
                    Vec2::splat(side)
                );
            }
            assert_eq!(
                image.image_options().tint,
                egui::Color32::from_rgb(0xcf, 0xcf, 0xcf)
            );
        }
    }
    #[test]
    fn each_feature_type_has_the_prototypes_icon() {
        // `FEATURE_ICON` in js/app.js for the types it lists; the toolbar's CREATE and
        // MODIFY commands' icons (Union, Subtract, Intersect, and the primitives) for the
        // rest; the box's for a type nothing names.
        for (kind, name) in [
            ("sketch", "sketch"),
            ("pad", "pad"),
            ("pocket", "pocket"),
            ("fillet", "fillet"),
            ("chamfer", "chamfer"),
            ("revolve", "revolve"),
            ("groove", "groove"),
            ("box", "box"),
            ("cylinder", "cylinder"),
            ("sphere", "sphere"),
            ("cone", "cone"),
            ("torus", "torus"),
            ("fuse", "union"),
            ("cut", "subtract"),
            ("common", "intersect"),
            ("helix", "box"),
        ] {
            assert_eq!(Icon::of_kind(kind).name(), name, "{kind}");
        }
    }
}
