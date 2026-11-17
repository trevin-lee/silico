//! The start page's thumbnails: each recent document's row shows a picture of its part (the
//! prototype's `.recent-row .thumb`, docs/design/m5-ui/js/app.js's `thumb()`, a 260 by 160
//! render cut to the 52 by 40 cell as `object-fit: cover` cuts it), or its icon if there's
//! none.
//!
//! A picture is the core's `render` of a tab's model, Home's view at [`WIDTH`] by
//! [`HEIGHT`], drawn only when the start page will show it: as a tab closes, and each time
//! the start page is shown, once for each open tab. Either way only for a tab with a file,
//! outside a transaction, whose model is built at its revision: the document as its file
//! holds it. As a tab closes, it goes at once, and its text and a render of its published
//! model are asked, its session closed once they've answered ([`Closing`]); on the start
//! page, an open tab's text is read
//! first, and its render asked only if its bytes have no picture. Never on a publish
//! otherwise, so an agent's edits reloaded every few seconds, or saves, draw nothing while
//! the start page is hidden. A render is never cancelled (a cancel kills the shared render
//! worker): one that comes back of another revision is dropped.
//!
//! A picture is kept in a folder of the app's own beside the recent documents
//! ([`Thumbs::default_folder`]), named by [`key`], the SHA-256 of the file's bytes the model
//! was built from, which the session's `text` at the model's revision gives exactly. So a
//! picture is shown only for the exact bytes it was drawn from: a file changed since (an
//! agent's edit) shows the icon until the app draws it again.
//!
//! The folder is kept by a thread of its own, so the UI thread never touches the disk or
//! hashes a document (invariant 4): it hashes each recent document's file and each text
//! read, reads and decodes the picture a key names, and cuts it to the cell, ready to
//! upload; it writes each picture drawn; and it prunes the folder ([`prune`]). A picture
//! that can't be read, is more than [`MAX_PNG_BYTES`], isn't a PNG, or is more than
//! [`MAX_SIDE`] a side is no picture: the icon shows. A render that fails (nothing built, no
//! bodies, no render worker) is logged, never said: a thumbnail is a nicety.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime};

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use sha2::{Digest as _, Sha256};
use silico_core::command::render::{Render, Rendering};
use silico_core::command::wire::Named;
use silico_core::command::{
    Closed, ModelChoice, Response, Revision, Running, SessionId, Sessions, TextAnswer,
};
use silico_core::kernel::Clients;

use crate::recent::Recents;
use crate::tab::Tab;
use crate::theme;
use crate::wake::Wake;

/// The folder's name in the app-data folder, beside the recent documents' list.
pub const FOLDER_NAME: &str = "thumbnails";

/// A render's width in pixels: the prototype's `VP.thumb(kind, 260, 160)`.
pub const WIDTH: u32 = 260;
/// A render's height in pixels.
pub const HEIGHT: u32 = 160;

/// The most of a picture's file read: a 260 by 160 PNG is far less. A longer file is no
/// picture.
pub const MAX_PNG_BYTES: usize = 1 << 20;

/// The longest side of a picture decoded: a PNG that says it's larger is no picture, and
/// nothing is allocated for it.
pub const MAX_SIDE: u32 = 1024;

/// How many pictures the folder keeps besides those of the recent documents' bytes: the
/// newest, by when they were written, so another instance's picture, or one of a document
/// that can't be read just now, isn't removed at once ([`prune`]).
pub const SPARE: usize = crate::recent::KEPT;

/// How old a file left by a write that never finished (`<key>.png.<pid>.<n>.new`, a quit
/// mid-write) is before it's removed: far longer than any write takes.
pub const LEFT_OVER_AGE: Duration = Duration::from_secs(60 * 60);

/// How many of a picture's pixels a point of the cell has, each way: a Retina display's 2
/// shows it pixel for pixel, a display of 1 at half (egui's wgpu textures have no mipmaps,
/// so the thread cuts it down, averaging, rather than leave a quarter of a 260 by 160
/// render to linear filtering).
pub const PICTURE_SCALE: f32 = 2.0;

/// The picture's place in the cell: inside its 1-point border (`box-sizing: border-box`), at
/// each side.
pub const CELL: egui::Vec2 = egui::vec2(
    theme::START_THUMB_WIDTH - theme::BORDER_WIDTH - theme::BORDER_WIDTH,
    theme::START_THUMB_HEIGHT - theme::BORDER_WIDTH - theme::BORDER_WIDTH,
);

/// A picture's size in pixels as the thread gives it: [`CELL`] at [`PICTURE_SCALE`], 100 by
/// 76.
pub const PICTURE: [usize; 2] = [
    (CELL.x * PICTURE_SCALE) as usize,
    (CELL.y * PICTURE_SCALE) as usize,
];

/// The render a picture is drawn by: Home's view, [`WIDTH`] by [`HEIGHT`].
fn request() -> Render {
    Render {
        view: Some(crate::navigate::HOME),
        camera: None,
        width: WIDTH,
        height: HEIGHT,
        highlight: Vec::new(),
        to: None,
        replace: false,
    }
}

/// The key a picture of a document is kept by: the SHA-256 of its file's bytes, in lower-case
/// hexadecimal.
pub fn key(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The name of the file the picture keyed `key` is kept in.
pub fn file_name(key: &str) -> String {
    format!("{key}.png")
}

/// Whether `key` is one [`key`] gives: 64 lower-case hexadecimal digits.
fn is_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The key a file in the folder named `name` is a picture of, if its name is one
/// [`file_name`] gives: never another file, which is left alone.
fn key_named(name: &str) -> Option<&str> {
    let key = name.strip_suffix(".png")?;
    is_key(key).then_some(key)
}

/// Whether a file in the folder named `name` is one a write of a picture left
/// (`app_data::write_bytes`'s `<key>.png.<pid>.<n>.new`), and nothing else.
fn left_by_a_write(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(".new") else {
        return false;
    };
    let mut parts = rest.rsplitn(3, '.');
    let (Some(n), Some(pid), Some(picture)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let number = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    number(n) && number(pid) && key_named(picture).is_some()
}

/// The key of the document at `path`, as its bytes are now: none if it isn't a regular file
/// (a pipe could hang the read), can't be read, or is longer than a document can be
/// (`json::MAX_DOCUMENT_LEN`), which no session opens.
pub fn key_of_file(path: &Path) -> Option<String> {
    let limit = silico_core::document::json::MAX_DOCUMENT_LEN;
    let bytes = crate::app_data::read_up_to(path, limit)?;
    (bytes.len() <= limit).then(|| key(&bytes))
}

/// The picture in `file`, cut to [`PICTURE`]: none if it can't be read, is longer than
/// [`MAX_PNG_BYTES`], or isn't a picture ([`picture`]).
fn read_picture(file: &Path) -> Option<ColorImage> {
    let bytes = crate::app_data::read_up_to(file, MAX_PNG_BYTES)?;
    if bytes.len() > MAX_PNG_BYTES {
        return None;
    }
    picture(&bytes)
}

/// `png` decoded and cut to [`PICTURE`] as `object-fit: cover` cuts it ([`cover`]): none if
/// it isn't a PNG the `png` crate decodes (which refuses a side of 0), or either side is more
/// than [`MAX_SIDE`].
pub fn picture(png: &[u8]) -> Option<ColorImage> {
    let image = decode(png)?;
    Some(cover(&image, PICTURE))
}

/// `png` decoded, its pixels as egui holds them (premultiplied): none as [`picture`] says.
pub fn decode(png: &[u8]) -> Option<ColorImage> {
    let most = MAX_SIDE as usize * MAX_SIDE as usize;
    let mut decoder = png::Decoder::new_with_limits(
        std::io::Cursor::new(png),
        // Every byte the decoder may allocate: the largest picture's, 16-bit RGBA, and room.
        png::Limits {
            bytes: most * 8 + (1 << 20),
        },
    );
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let mut bytes = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut bytes).ok()?;
    let bytes = bytes.get(..frame.buffer_size())?;
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        // Expanded to RGB or RGBA by the transformations: never given.
        png::ColorType::Indexed => return None,
    };
    if frame.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let (width, height) = (frame.width as usize, frame.height as usize);
    let row = frame.line_size;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        let line = bytes.get(y * row..y * row + width * channels)?;
        pixels.extend(line.chunks_exact(channels).map(|p| match *p {
            [grey] => Color32::from_gray(grey),
            [grey, alpha] => Color32::from_rgba_unmultiplied(grey, grey, grey, alpha),
            [red, green, blue] => Color32::from_rgb(red, green, blue),
            [red, green, blue, alpha] => Color32::from_rgba_unmultiplied(red, green, blue, alpha),
            _ => Color32::TRANSPARENT,
        }));
    }
    Some(ColorImage::new([width, height], pixels))
}

/// `image` cut to the shape of `size` about its centre, as CSS's `object-fit: cover` cuts it
/// (the whole of one side kept, the other cut equally at both ends), and scaled to `size`,
/// each pixel the average of the part of the image it covers, by area.
pub fn cover(image: &ColorImage, size: [usize; 2]) -> ColorImage {
    let [width, height] = image.size;
    let [to_width, to_height] = size;
    if width == 0 || height == 0 || to_width == 0 || to_height == 0 {
        return ColorImage::new(size, vec![Color32::TRANSPARENT; to_width * to_height]);
    }
    // The image's pixels each of the result's covers, each way: the smaller of the two, so
    // the side that fits is kept whole.
    let step = (width as f64 / to_width as f64).min(height as f64 / to_height as f64);
    let start_x = (width as f64 - step * to_width as f64) / 2.0;
    let start_y = (height as f64 - step * to_height as f64) / 2.0;
    let across = spans(start_x, step, to_width, width);
    let down = spans(start_y, step, to_height, height);
    // Across each row of the image, then down each column of that.
    let mut rows = vec![[0.0f64; 4]; to_width * height];
    for y in 0..height {
        for (x, span) in across.iter().enumerate() {
            let mut sum = [0.0f64; 4];
            for &(from, weight) in span {
                let pixel = image.pixels.get(y * width + from).copied();
                let pixel = pixel.unwrap_or(Color32::TRANSPARENT).to_array();
                for (sum, channel) in sum.iter_mut().zip(pixel) {
                    *sum += weight * f64::from(channel);
                }
            }
            if let Some(at) = rows.get_mut(y * to_width + x) {
                *at = sum;
            }
        }
    }
    let mut pixels = Vec::with_capacity(to_width * to_height);
    for span in &down {
        for x in 0..to_width {
            let mut sum = [0.0f64; 4];
            for &(from, weight) in span {
                let row = rows.get(from * to_width + x).copied().unwrap_or_default();
                for (sum, channel) in sum.iter_mut().zip(row) {
                    *sum += weight * channel;
                }
            }
            let [r, g, b, a] =
                sum.map(|channel| (channel / (step * step)).round().clamp(0.0, 255.0) as u8);
            pixels.push(Color32::from_rgba_premultiplied(r, g, b, a));
        }
    }
    ColorImage::new(size, pixels)
}

/// For each of `count` spans `step` long from `start` along a side `len` pixels long, the
/// pixels it covers and how much of each.
fn spans(start: f64, step: f64, count: usize, len: usize) -> Vec<Vec<(usize, f64)>> {
    (0..count)
        .map(|at| {
            let from = start + step * at as f64;
            let to = from + step;
            let first = from.floor().max(0.0) as usize;
            let last = (to.ceil() as usize).min(len);
            (first..last)
                .filter_map(|pixel| {
                    let covered = (to.min(pixel as f64 + 1.0) - from.max(pixel as f64)).max(0.0);
                    (covered > 0.0).then_some((pixel, covered))
                })
                .collect()
        })
        .collect()
}

/// The picture of the document at `path`, as the thread found it.
struct Found {
    path: String,
    key: String,
    image: ColorImage,
}

/// The bytes a picture drawn is keyed by: their key, or a document's text, which the thread
/// hashes.
enum Of {
    Key(String),
    Text(String),
}

/// What the window asks the folder's thread.
enum Ask {
    /// The recent documents' paths, as Recent lists them: each one's picture found, and the
    /// folder pruned ([`prune`]).
    Look(Vec<String>),
    /// Whether the bytes `text` has a picture, for `session`'s model at `revision`
    /// ([`Told::Have`], [`Told::Missing`]).
    Want {
        session: SessionId,
        revision: Revision,
        text: String,
    },
    /// A picture drawn, `png`, to write as `of`'s, unless it has one.
    Write { of: Of, png: Vec<u8> },
}

/// What the folder's thread tells the window.
enum Told {
    /// The pictures of the recent documents last looked at, by path: those that have one.
    Pictures(Vec<Found>),
    /// A picture is kept for the bytes of `session`'s model at `revision`: it isn't drawn.
    Have {
        session: SessionId,
        revision: Revision,
    },
    /// No picture is kept as `key`'s, the key of `session`'s model at `revision`: it's to be
    /// drawn.
    Missing {
        session: SessionId,
        revision: Revision,
        key: String,
    },
}

/// A command's answer, while it's out, then once it's taken.
enum Waiting<T> {
    Out(Running<T>),
    Taken(Response<T>),
}

/// A tab's picture asked as it closes, and then its session's close: taken by the app's
/// thumbnails, which finish it after the tab has gone. Its text at its revision and a render
/// of its published model (its revision's, or the picture is dropped: a close never waits
/// on a rebuild) are sent at once; the close only once both have answered, since a session
/// closing stops its published reads, and a render stopped stops the render worker every
/// session shares. Its close can't be refused: a picture is asked only outside a transaction.
pub struct Closing {
    session: SessionId,
    revision: Revision,
    /// Its document's path, as its tab had it: an open of it waits for the close.
    path: String,
    stage: Stage,
}

/// A [`Closing`]'s text and render, while either is out.
struct Out {
    text: Waiting<TextAnswer>,
    rendering: Waiting<Rendering>,
}

/// How far a [`Closing`] has come.
enum Stage {
    /// Its text and its render out (boxed: a render's answer holds its PNG's handle).
    Drawing(Box<Out>),
    /// Its session's close sent.
    Closing(Running<Closed>),
}

impl Closing {
    /// The text and the render of `session`'s model at `revision`, sent; its close sent once
    /// they've answered ([`Thumbs::closing`]).
    pub(crate) fn sent<P: Clients + 'static>(
        sessions: &Arc<Sessions<P>>,
        session: SessionId,
        revision: Revision,
        path: String,
    ) -> Self {
        Closing {
            session,
            revision,
            path,
            stage: Stage::Drawing(Box::new(Out {
                text: Waiting::Out(sessions.text_at(
                    &session,
                    None,
                    Some(Named::Revision(revision)),
                )),
                rendering: Waiting::Out(sessions.render(
                    &session,
                    ModelChoice::Published,
                    request(),
                )),
            })),
        }
    }
}

impl<T> Waiting<T> {
    /// Its answer taken, if it has come.
    fn polled(self) -> Self {
        match self {
            Waiting::Out(running) => match running.try_take() {
                Ok(response) => Waiting::Taken(response),
                Err(running) => Waiting::Out(running),
            },
            taken => taken,
        }
    }
}

/// Where an open tab's picture stands while the start page is shown, for the revision it's
/// of.
enum Drawing {
    /// The text the model was built from, read to key it.
    Reading {
        revision: Revision,
        running: Running<TextAnswer>,
    },
    /// The thread asked whether its bytes have a picture.
    Asking { revision: Revision },
    /// Drawn by the core's `render`, of the published model.
    Rendering {
        revision: Revision,
        key: String,
        running: Running<Rendering>,
    },
}

/// A picture uploaded, and the key it's of.
struct Shown {
    key: String,
    texture: TextureHandle,
}

/// The png of a render of a model at `revision`, `response`: or why it's not that, the
/// render refused, or a model of another revision drawn (one published since it was asked
/// for), whose picture isn't of the bytes keyed.
fn drawn_at(revision: Revision, response: Response<Rendering>) -> Result<Vec<u8>, String> {
    let of = response.model.as_ref().map(|model| model.revision);
    match response.result {
        Ok(rendering) if of == Some(revision) => Ok(rendering.png),
        Ok(_) => Err(format!(
            "the model drawn wasn't revision {}'s",
            revision.number()
        )),
        Err(problems) => Err(format!(
            "the render was refused: {}",
            crate::tab::said(&problems)
        )),
    }
}

/// The recent documents' pictures, as the window last heard them from the thread that keeps
/// them, and the pictures of open tabs being drawn.
pub struct Thumbs {
    asks: Option<Sender<Ask>>,
    told: Option<Receiver<Told>>,
    wake: Wake,
    shown: BTreeMap<String, Shown>,
    drawing: BTreeMap<SessionId, Drawing>,
    closing: Vec<Closing>,
    /// The open tabs drawn, or found pictured, since the start page was last shown.
    this_showing: BTreeSet<SessionId>,
    /// Each open tab's revision known to have a picture: not drawn again.
    pictured: BTreeMap<SessionId, Revision>,
    /// The paths last told the thread, and whether the start page was shown last frame.
    looked: Option<Vec<String>>,
    was_shown: bool,
    /// What a test counts: renders whose picture was sent to be written; renders refused,
    /// or of another revision's model, so not written; models whose bytes were found to have
    /// a picture, so weren't drawn; and how many times the thread has told the recent
    /// documents' pictures.
    pub renders_written: u64,
    pub renders_dropped: u64,
    pub already_pictured: u64,
    pub looks: u64,
}

impl Thumbs {
    /// The pictures kept in `folder`, by a thread of its own.
    pub fn at(folder: PathBuf) -> Self {
        let (asks, asked) = mpsc::channel();
        let (tell, told) = mpsc::channel();
        let wake = Wake::default();
        let waking = wake.clone();
        let started = std::thread::Builder::new()
            .name("silico-thumbnails".to_owned())
            .spawn(move || keep_folder(&folder, &asked, &tell, &waking));
        match started {
            Ok(_) => Thumbs {
                asks: Some(asks),
                told: Some(told),
                ..Self::unkept(wake)
            },
            Err(e) => {
                tracing::warn!("the start page's thumbnails can't be kept: {e}");
                Self::unkept(wake)
            }
        }
    }

    /// Pictures not kept: none shown, none drawn.
    fn unkept(wake: Wake) -> Self {
        Thumbs {
            asks: None,
            told: None,
            wake,
            shown: BTreeMap::new(),
            drawing: BTreeMap::new(),
            closing: Vec::new(),
            this_showing: BTreeSet::new(),
            pictured: BTreeMap::new(),
            looked: None,
            was_shown: false,
            renders_written: 0,
            renders_dropped: 0,
            already_pictured: 0,
            looks: 0,
        }
    }

    /// Where the pictures are kept: `thumbnails` in the platform's app-data folder, beside
    /// the recent documents' list (`crate::app_data::folder`); none if the platform names no
    /// home folder.
    pub fn default_folder() -> Option<PathBuf> {
        crate::app_data::folder().map(|folder| folder.join(FOLDER_NAME))
    }

    /// The picture of the recent document at `path`, if it has one: [`PICTURE`] in pixels.
    pub fn picture(&self, path: &str) -> Option<&TextureHandle> {
        self.shown.get(path).map(|shown| &shown.texture)
    }

    /// The thread asks `ctx` for a frame whenever it tells something.
    pub(crate) fn waking(&self, ctx: &egui::Context) {
        self.wake.set(ctx);
    }

    fn ask(&self, ask: Ask) {
        if let Some(asks) = &self.asks {
            // A thread that has ended keeps nothing more; the pictures shown stay.
            let _ = asks.send(ask);
        }
    }

    /// A tab's picture asked as it closed, and its session's close: finished from now on, the
    /// tab gone.
    pub(crate) fn closing(&mut self, closing: Closing) {
        self.closing.push(closing);
    }

    /// Whether the document at `path` is held by a session still closing, its tab gone: an
    /// open of it would be refused until the close answers, which it always does. Paths are
    /// compared as text made absolute, as an open tab's are, never by the file system.
    pub fn still_closing(&self, path: &str) -> bool {
        let Ok(path) = std::path::absolute(path) else {
            return false;
        };
        self.closing
            .iter()
            .any(|closing| std::path::absolute(&closing.path).is_ok_and(|at| at == path))
    }

    /// One frame's worth, never waiting: what the thread told taken (pictures uploaded to
    /// `ctx`, and drawings it found missing started); every drawing polled; each tab of
    /// `tabs` told whether it's to draw its picture as it closes (not while `quitting`, nor
    /// for a revision known to have one); and, while the start page is `shown`, each open
    /// tab with a file, outside a transaction, and built at its revision, read once a
    /// showing and drawn if its bytes have no picture. Once `recents` is read, its paths told
    /// the thread each time they change and each time the start page is shown again.
    /// Nothing at all while the app keeps no list of recent documents: there's nowhere to
    /// show a picture.
    pub(crate) fn frame<P: Clients + 'static>(
        &mut self,
        ctx: &egui::Context,
        sessions: &Arc<Sessions<P>>,
        tabs: &mut [Tab],
        recents: Option<&Recents>,
        shown: bool,
        quitting: bool,
    ) {
        let Some(recents) = recents.filter(|recents| recents.is_kept()) else {
            for tab in tabs {
                tab.draws_on_close = false;
            }
            return;
        };
        self.take_told(ctx, sessions);
        self.poll_closing(sessions);
        let sessions_drawing: Vec<SessionId> = self.drawing.keys().copied().collect();
        for session in sessions_drawing {
            self.poll_drawing(session);
        }
        let open = |session: &SessionId| tabs.iter().any(|tab| tab.id == *session);
        self.pictured.retain(|session, _| open(session));
        self.this_showing.retain(|session| open(session));
        if shown && !self.was_shown {
            self.this_showing.clear();
        }
        for tab in tabs.iter_mut() {
            let pictured = self.pictured.get(&tab.id) == Some(&tab.revision);
            tab.draws_on_close = !quitting && !pictured;
            let ready =
                tab.path.is_some() && !tab.transaction && tab.published == Some(tab.revision);
            if shown
                && ready
                && !pictured
                && !self.this_showing.contains(&tab.id)
                && !self.drawing.contains_key(&tab.id)
            {
                self.this_showing.insert(tab.id);
                let running = sessions.text_at(&tab.id, None, Some(Named::Revision(tab.revision)));
                self.drawing.insert(
                    tab.id,
                    Drawing::Reading {
                        revision: tab.revision,
                        running,
                    },
                );
            }
        }
        if recents.is_read() {
            let listed = recents.list().iter().map(|recent| &recent.path);
            let same = self
                .looked
                .as_ref()
                .is_some_and(|looked| looked.iter().eq(listed.clone()));
            if !same || (shown && !self.was_shown) {
                let paths: Vec<String> = listed.cloned().collect();
                self.ask(Ask::Look(paths.clone()));
                self.looked = Some(paths);
            }
        }
        self.was_shown = shown;
    }

    /// What the thread has told since the last frame.
    fn take_told<P: Clients + 'static>(
        &mut self,
        ctx: &egui::Context,
        sessions: &Arc<Sessions<P>>,
    ) {
        let told: Vec<Told> = match &self.told {
            Some(told) => told.try_iter().collect(),
            None => return,
        };
        for told in told {
            match told {
                Told::Pictures(found) => {
                    let mut shown = BTreeMap::new();
                    for Found { path, key, image } in found {
                        let texture = match self.shown.remove(&path) {
                            Some(was) if was.key == key => was.texture,
                            _ => ctx.load_texture(
                                format!("thumbnail {key}"),
                                image,
                                TextureOptions::LINEAR,
                            ),
                        };
                        shown.insert(path, Shown { key, texture });
                    }
                    self.shown = shown;
                    self.looks += 1;
                }
                Told::Have { session, revision } => {
                    if self.asked(session, revision) {
                        self.drawing.remove(&session);
                        self.pictured.insert(session, revision);
                        self.already_pictured += 1;
                    }
                }
                Told::Missing {
                    session,
                    revision,
                    key,
                } => {
                    if self.asked(session, revision) {
                        let running = sessions.render(&session, ModelChoice::Published, request());
                        self.drawing.insert(
                            session,
                            Drawing::Rendering {
                                revision,
                                key,
                                running,
                            },
                        );
                    }
                }
            }
        }
    }

    /// Whether `session`'s drawing is waiting on the thread to say whether its model at
    /// `revision` has a picture: a word of another revision's is of a drawing gone.
    fn asked(&self, session: SessionId, revision: Revision) -> bool {
        matches!(
            self.drawing.get(&session),
            Some(Drawing::Asking { revision: asked }) if *asked == revision
        )
    }

    /// `session`'s drawing moved on by its command's answer, if it has come: dropped once
    /// it's done.
    fn poll_drawing(&mut self, session: SessionId) {
        let Some(drawing) = self.drawing.remove(&session) else {
            return;
        };
        let next = match drawing {
            Drawing::Reading { revision, running } => match running.try_take() {
                Ok(response) => match response.result {
                    Ok(answer) => {
                        self.ask(Ask::Want {
                            session,
                            revision,
                            text: answer.text,
                        });
                        Some(Drawing::Asking { revision })
                    }
                    Err(problems) => {
                        tracing::info!(
                            %session,
                            "no thumbnail: the text of revision {} wasn't read: {}",
                            revision.number(),
                            crate::tab::said(&problems)
                        );
                        None
                    }
                },
                Err(running) => Some(Drawing::Reading { revision, running }),
            },
            Drawing::Rendering {
                revision,
                key,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    self.took_render(session, revision, Of::Key(key), response);
                    None
                }
                Err(running) => Some(Drawing::Rendering {
                    revision,
                    key,
                    running,
                }),
            },
            asking @ Drawing::Asking { .. } => Some(asking),
        };
        if let Some(next) = next {
            self.drawing.insert(session, next);
        }
    }

    /// Each picture asked as its tab closed: once its text and its render have answered,
    /// written if both are its revision's, and its session's close sent; once that has
    /// answered, done.
    fn poll_closing<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>) {
        for Closing {
            session,
            revision,
            path,
            stage,
        } in std::mem::take(&mut self.closing)
        {
            let stage = match stage {
                Stage::Drawing(out) => match (out.text.polled(), out.rendering.polled()) {
                    (Waiting::Taken(text), Waiting::Taken(rendering)) => {
                        match text.result {
                            Ok(answer) => {
                                self.took_render(
                                    session,
                                    revision,
                                    Of::Text(answer.text),
                                    rendering,
                                );
                            }
                            Err(problems) => {
                                tracing::info!(
                                    %session,
                                    "no thumbnail: the text of revision {} wasn't read: {}",
                                    revision.number(),
                                    crate::tab::said(&problems)
                                );
                                self.renders_dropped += 1;
                            }
                        }
                        Some(Stage::Closing(sessions.close(&session)))
                    }
                    (text, rendering) => Some(Stage::Drawing(Box::new(Out { text, rendering }))),
                },
                Stage::Closing(running) => match running.try_take() {
                    Ok(response) => {
                        if let Err(problems) = response.result {
                            tracing::warn!(
                                %session,
                                "a closed tab's session didn't close: {}",
                                crate::tab::said(&problems)
                            );
                        }
                        None
                    }
                    Err(running) => Some(Stage::Closing(running)),
                },
            };
            if let Some(stage) = stage {
                self.closing.push(Closing {
                    session,
                    revision,
                    path,
                    stage,
                });
            }
        }
    }

    /// A render of `session`'s model at `revision` answered, `response`: its picture sent to
    /// be written as `of`'s if it's that revision's ([`drawn_at`]), else logged and dropped.
    fn took_render(
        &mut self,
        session: SessionId,
        revision: Revision,
        of: Of,
        response: Response<Rendering>,
    ) {
        match drawn_at(revision, response) {
            Ok(png) => {
                self.ask(Ask::Write { of, png });
                self.pictured.insert(session, revision);
                self.renders_written += 1;
            }
            Err(why) => {
                tracing::info!(%session, "no thumbnail: {why}");
                self.renders_dropped += 1;
            }
        }
    }
}

/// The thread that keeps the folder: each ask taken as it comes. A look finds each recent
/// document's picture and prunes the folder; a want hashes a text and tells back whether
/// its key has a picture (a file that isn't one doesn't count); a write writes a picture
/// (unless its key has one), then, once the recent documents have been looked at, prunes
/// and looks again, so a picture drawn shows.
fn keep_folder(folder: &Path, asked: &Receiver<Ask>, tell: &Sender<Told>, wake: &Wake) {
    let _span = tracing::info_span!("thumbnails", folder = %folder.display()).entered();
    let mut listed: Option<Vec<String>> = None;
    while let Ok(ask) = asked.recv() {
        let told = match ask {
            Ask::Look(paths) => {
                let told = look_and_prune(folder, &paths);
                listed = Some(paths);
                Some(told)
            }
            Ask::Want {
                session,
                revision,
                text,
            } => {
                let key = key(text.as_bytes());
                Some(if read_picture(&folder.join(file_name(&key))).is_some() {
                    Told::Have { session, revision }
                } else {
                    Told::Missing {
                        session,
                        revision,
                        key,
                    }
                })
            }
            Ask::Write { of, png } => {
                let key = match of {
                    Of::Key(key) => key,
                    Of::Text(text) => key(text.as_bytes()),
                };
                let file = folder.join(file_name(&key));
                if read_picture(&file).is_none()
                    && let Err(e) = crate::app_data::write_bytes(&file, &png)
                {
                    tracing::info!("a thumbnail couldn't be written to {}: {e}", file.display());
                }
                listed.as_ref().map(|paths| look_and_prune(folder, paths))
            }
        };
        if let Some(told) = told {
            if tell.send(told).is_err() {
                return;
            }
            wake.wake();
        }
    }
}

/// The pictures of the documents at `paths` found in `folder`, the folder pruned to them
/// ([`prune`]): no picture removed if a document there couldn't be hashed, since its key
/// isn't known.
fn look_and_prune(folder: &Path, paths: &[String]) -> Told {
    let mut found = Vec::new();
    let mut keys = BTreeSet::new();
    let mut unknown = false;
    for path in paths {
        let Some(key) = key_of_file(Path::new(path)) else {
            // Something there that can't be read just now (not nothing: a missing document's
            // picture may go).
            unknown |= std::fs::symlink_metadata(path).is_ok();
            continue;
        };
        if let Some(image) = read_picture(&folder.join(file_name(&key))) {
            found.push(Found {
                path: path.clone(),
                key: key.clone(),
                image,
            });
        }
        keys.insert(key);
    }
    prune(folder, (!unknown).then_some(&keys), SystemTime::now());
    Told::Pictures(found)
}

/// `folder` pruned at `now`: every picture but those of `keys` and the newest [`SPARE`] of
/// the rest (by when each was modified, then by name) removed, or none if `keys` is none;
/// and every file a write left ([`left_by_a_write`]) older than [`LEFT_OVER_AGE`]. Any other
/// file is left. So the folder holds at most twice as many pictures as Recent lists.
fn prune(folder: &Path, keys: Option<&BTreeSet<String>>, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut others = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(key) = key_named(name) {
            if keys.is_some_and(|keys| !keys.contains(key)) {
                others.push((modified, name.to_owned()));
            }
        } else if left_by_a_write(name)
            && now
                .duration_since(modified)
                .is_ok_and(|age| age > LEFT_OVER_AGE)
        {
            remove(&folder.join(name));
        }
    }
    // Newest first.
    others.sort_by(|a, b| b.cmp(a));
    for (_, name) in others.into_iter().skip(SPARE) {
        remove(&folder.join(name));
    }
}

/// `file` removed, a failure logged.
fn remove(file: &Path) {
    if let Err(e) = std::fs::remove_file(file) {
        tracing::info!("{} couldn't be removed: {e}", file.display());
    }
}

/// The picture `texture` drawn in the cell `thumb` (the thumbnail's place, its border
/// included): inside its border, its corners rounded to match.
pub(crate) fn paint(painter: &egui::Painter, thumb: egui::Rect, texture: &TextureHandle) {
    let inside = thumb.shrink(theme::BORDER_WIDTH);
    painter.add(
        egui::epaint::RectShape::filled(
            inside,
            theme::SMALL_CORNER_RADIUS.saturating_sub(theme::BORDER_WIDTH as u8),
            Color32::WHITE,
        )
        .with_texture(
            texture.id(),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG of `width` by `height` pixels of `color_type`, its samples `data`, encoded by the
    /// png crate.
    fn png_of(width: u32, height: u32, color_type: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color_type);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        out
    }

    /// A PNG of one colour, `width` by `height`.
    fn plain_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let data: Vec<u8> = (0..width * height).flat_map(|_| rgb).collect();
        png_of(width, height, png::ColorType::Rgb, &data)
    }

    /// The SHA-256 test vectors of FIPS 180-2 (appendix B.1) and of the empty message.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_key_is_the_sha_256_of_the_bytes_in_lower_case_hex() {
        assert_eq!(key(b"abc"), ABC);
        assert_eq!(key(b""), EMPTY);
        assert_eq!(file_name(ABC), format!("{ABC}.png"));
    }

    #[test]
    fn a_files_key_is_its_bytes_and_none_for_what_isnt_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("part.silico");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(key_of_file(&file).as_deref(), Some(ABC));
        std::fs::write(&file, b"").unwrap();
        assert_eq!(key_of_file(&file).as_deref(), Some(EMPTY));
        assert_eq!(key_of_file(dir.path()), None);
        assert_eq!(key_of_file(&dir.path().join("none.silico")), None);
    }

    #[test]
    fn a_file_longer_than_a_document_can_be_has_no_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("huge.silico")).unwrap();
        let limit = silico_core::document::json::MAX_DOCUMENT_LEN as u64;
        // Sparse: no 16 MiB written.
        file.set_len(limit).unwrap();
        assert!(key_of_file(&dir.path().join("huge.silico")).is_some());
        file.set_len(limit + 1).unwrap();
        assert_eq!(key_of_file(&dir.path().join("huge.silico")), None);
    }

    #[test]
    fn only_a_keys_own_name_is_a_picture_of_the_folder() {
        assert_eq!(key_named(&format!("{ABC}.png")), Some(ABC));
        for name in [
            ABC.to_owned(),
            format!("{}.png", ABC.to_uppercase()),
            format!("{}.png", &ABC[1..]),
            format!("{ABC}0.png"),
            format!("{ABC}.png.1.2.new"),
            format!("{}g.png", &ABC[1..]),
            "notes.txt".to_owned(),
            ".png".to_owned(),
        ] {
            assert_eq!(key_named(&name), None, "{name}");
        }
    }

    #[test]
    fn a_png_is_decoded_as_its_pixels_whatever_its_colour_type() {
        // Two by one: red then a half-transparent blue (RGBA), and grey 7 then 200 (grey).
        let rgba = png_of(
            2,
            1,
            png::ColorType::Rgba,
            &[255, 0, 0, 255, 0, 0, 255, 128],
        );
        let image = decode(&rgba).unwrap();
        assert_eq!(image.size, [2, 1]);
        assert_eq!(
            image.pixels,
            [
                Color32::from_rgb(255, 0, 0),
                Color32::from_rgba_unmultiplied(0, 0, 255, 128)
            ]
        );
        let grey = decode(&png_of(2, 1, png::ColorType::Grayscale, &[7, 200])).unwrap();
        assert_eq!(
            grey.pixels,
            [Color32::from_gray(7), Color32::from_gray(200)]
        );
        // Grey with alpha: grey 7, opaque, then grey 200 at 128.
        let grey_alpha = png_of(2, 1, png::ColorType::GrayscaleAlpha, &[7, 255, 200, 128]);
        assert_eq!(
            decode(&grey_alpha).unwrap().pixels,
            [
                Color32::from_rgba_unmultiplied(7, 7, 7, 255),
                Color32::from_rgba_unmultiplied(200, 200, 200, 128)
            ]
        );
        // Three rows of an odd width, RGB: each row read from its own line.
        let data: Vec<u8> = (0..3 * 3 * 3).map(|n| n as u8).collect();
        let rgb = decode(&png_of(3, 3, png::ColorType::Rgb, &data)).unwrap();
        assert_eq!(rgb.size, [3, 3]);
        assert_eq!(rgb.pixels[4], Color32::from_rgb(12, 13, 14));
        assert_eq!(rgb.pixels[8], Color32::from_rgb(24, 25, 26));
    }

    #[test]
    fn what_isnt_a_png_or_is_too_large_is_no_picture() {
        let good = plain_png(4, 4, [1, 2, 3]);
        assert!(picture(&good).is_some());
        // Nothing; text; the signature alone; the file cut short; a byte of its header changed
        // (the CRC fails).
        let mut flipped = good.clone();
        flipped[20] ^= 1;
        for bytes in [
            &b""[..],
            b"PNG 260x160",
            &good[..8],
            &good[..good.len() / 2],
            &flipped,
        ] {
            assert!(picture(bytes).is_none(), "{bytes:?}");
        }
        // One pixel more than the longest side, either way.
        assert!(decode(&plain_png(MAX_SIDE, 1, [0, 0, 0])).is_some());
        assert!(decode(&plain_png(1, MAX_SIDE, [0, 0, 0])).is_some());
        assert!(decode(&plain_png(MAX_SIDE + 1, 1, [0, 0, 0])).is_none());
        assert!(decode(&plain_png(1, MAX_SIDE + 1, [0, 0, 0])).is_none());
    }

    #[test]
    fn a_header_that_claims_a_huge_picture_allocates_nothing() {
        // A header, hand-made, of 100,000 by 100,000 pixels of RGBA (40 GB), and no data.
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend(100_000u32.to_be_bytes());
        ihdr.extend(100_000u32.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(13u32.to_be_bytes());
        png.extend(&ihdr);
        png.extend(crc(&ihdr).to_be_bytes());
        assert!(decode(&png).is_none());
    }

    /// `png` with a `tEXt` chunk of `len` bytes (a keyword of one letter, its null, and text)
    /// after its header for each of `lens`.
    fn with_text(png: &[u8], lens: &[usize]) -> Vec<u8> {
        // The signature (8) and the header chunk: its length (4), type (4), data (13), and
        // CRC (4).
        let (head, rest) = png.split_at(8 + 4 + 4 + 13 + 4);
        let mut out = head.to_vec();
        for &len in lens {
            let mut chunk = b"tEXt".to_vec();
            chunk.extend(b"k\0");
            chunk.resize(4 + len, b'a');
            out.extend(u32::try_from(len).unwrap().to_be_bytes());
            out.extend(&chunk);
            out.extend(crc(&chunk).to_be_bytes());
        }
        out.extend(rest);
        out
    }

    #[test]
    fn the_decoder_may_allocate_nine_mebibytes_and_no_more() {
        // Its allowance: the largest picture's 16-bit RGBA, 1024 · 1024 · 8 = 8 MiB, and
        // 1 MiB of room, 9 MiB (9,437,184 bytes). A text chunk costs the first time its
        // length twice (the decoder's chunk buffer grown to it from 128 bytes, doubling,
        // then the text kept), and after that once (the buffer kept).
        const MIB: usize = 1 << 20;
        let good = plain_png(2, 2, [1, 2, 3]);
        assert!(decode(&with_text(&good, &[MIB / 2])).is_some());
        // Three texts of 2 MiB and one of 0.5 MiB: 2 MiB − 128 for the buffer, and 6.5 MiB
        // kept, 8.5 MiB − 128 (8,912,768): within it, but past 8 MiB.
        let within = with_text(&good, &[2 * MIB, 2 * MIB, 2 * MIB, MIB / 2]);
        assert_eq!(
            decode(&within).map(|image| image.pixels),
            Some(vec![Color32::from_rgb(1, 2, 3); 4])
        );
        // A text of 1 MiB more: 9.5 MiB − 128, past it.
        let past = with_text(&good, &[2 * MIB, 2 * MIB, 2 * MIB, MIB / 2, MIB]);
        assert!(decode(&past).is_none());
    }

    /// PNG's CRC-32 (ISO 3309), bit by bit, as its specification's appendix gives it.
    fn crc(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    0xedb8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn a_picture_file_longer_than_a_mebibyte_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(file_name(ABC));
        // A good PNG, then bytes after its end, which a decoder never reads.
        let mut bytes = plain_png(4, 4, [9, 9, 9]);
        std::fs::write(&file, &bytes).unwrap();
        assert!(read_picture(&file).is_some());
        bytes.resize(MAX_PNG_BYTES, 0);
        std::fs::write(&file, &bytes).unwrap();
        assert!(read_picture(&file).is_some(), "exactly the most");
        assert!(picture(&bytes).is_some());
        bytes.push(0);
        std::fs::write(&file, &bytes).unwrap();
        assert!(read_picture(&file).is_none(), "a byte more");
        assert!(read_picture(dir.path()).is_none());
    }

    /// A grey image of `width` by `height` whose pixels are `greys`, row by row.
    fn greys(width: usize, height: usize, greys: &[u8]) -> ColorImage {
        ColorImage::new(
            [width, height],
            greys.iter().map(|&g| Color32::from_gray(g)).collect(),
        )
    }

    #[test]
    fn a_picture_is_cut_about_its_middle_and_each_pixel_averaged_by_area() {
        // The same shape: each pixel of the two the average of a square of four.
        let image = greys(4, 2, &[10, 20, 30, 40, 50, 60, 70, 80]);
        let cut = cover(&image, [2, 1]);
        // (10 + 20 + 50 + 60) / 4, (30 + 40 + 70 + 80) / 4.
        assert_eq!(cut.pixels, [Color32::from_gray(35), Color32::from_gray(55)]);
        // Wider: 6 by 2 into 2 by 1 keeps the height whole and cuts a column off each end.
        let image = greys(6, 2, &[0, 10, 20, 30, 40, 250, 250, 50, 60, 70, 80, 0]);
        let cut = cover(&image, [2, 1]);
        // (10 + 20 + 50 + 60) / 4, (30 + 40 + 70 + 80) / 4.
        assert_eq!(cut.pixels, [Color32::from_gray(35), Color32::from_gray(55)]);
        // Taller: 1 by 3 into 1 by 2 keeps the width whole and cuts half a row off each end,
        // each pixel half of one and half of the next: (0 + 100) / 2, (100 + 200) / 2.
        let cut = cover(&greys(1, 3, &[0, 100, 200]), [1, 2]);
        assert_eq!(
            cut.pixels,
            [Color32::from_gray(50), Color32::from_gray(150)]
        );
        // And wider by half a column: 3 by 1 into 2 by 1, (0 + 90) / 2, (90 + 210) / 2.
        let cut = cover(&greys(3, 1, &[0, 90, 210]), [2, 1]);
        assert_eq!(
            cut.pixels,
            [Color32::from_gray(45), Color32::from_gray(150)]
        );
        // Into a size it's not a whole number of times: 3 by 3 into 2 by 2, each pixel 1.5 a
        // side, 2.25 of the image's: the middle pixel, 90, a quarter in each (90 · 0.25 / 2.25
        // = 10), the corner, 225, wholly in the last (225 / 2.25 = 100).
        let cut = cover(&greys(3, 3, &[0, 0, 0, 0, 90, 0, 0, 0, 225]), [2, 2]);
        assert_eq!(cut.pixels, [10, 10, 10, 110].map(Color32::from_gray),);
    }

    #[test]
    fn a_picture_narrower_than_the_cell_keeps_its_width_and_its_middle_rows() {
        // 2 by 4 into 2 by 2: each pixel one of the image's (2 / 2 across, 4 / 2 down, the
        // smaller), the width kept whole and a row cut off each end.
        let image = greys(2, 4, &[10, 20, 30, 40, 50, 60, 70, 80]);
        let cut = cover(&image, [2, 2]);
        assert_eq!(cut.pixels, [30, 40, 50, 60].map(Color32::from_gray));
    }

    #[test]
    fn a_render_is_cut_to_the_cell_at_twice_its_size() {
        // The cell inside its border, 50 by 38, at 2 pixels a point.
        assert_eq!(PICTURE, [100, 76]);
        // A render of one colour, 260 by 160: that colour throughout.
        let cut = picture(&plain_png(WIDTH, HEIGHT, [12, 34, 56])).unwrap();
        assert_eq!(cut.size, [100, 76]);
        assert!(
            cut.pixels
                .iter()
                .all(|&pixel| pixel == Color32::from_rgb(12, 34, 56))
        );
        // Its left half black, its right white: the cut keeps the middle 210.5 of its 260
        // columns, each of the result's 100 covering 2.1, so the middle falls between the
        // 50th and the 51st: black then white, half and half.
        let data: Vec<u8> = (0..HEIGHT)
            .flat_map(|_| (0..WIDTH).flat_map(|x| if x < WIDTH / 2 { [0; 3] } else { [255; 3] }))
            .collect();
        let cut = picture(&png_of(WIDTH, HEIGHT, png::ColorType::Rgb, &data)).unwrap();
        let row: Vec<u8> = cut.pixels[..100].iter().map(|pixel| pixel.r()).collect();
        assert!(row[..50].iter().all(|&r| r == 0), "{row:?}");
        assert!(row[50..].iter().all(|&r| r == 255), "{row:?}");
    }

    /// The folder's thread, on `folder`: what it's asked, and what it tells.
    fn kept(folder: &Path) -> (Sender<Ask>, Receiver<Told>) {
        let (asks, asked) = mpsc::channel();
        let (tell, told) = mpsc::channel();
        let folder = folder.to_path_buf();
        std::thread::spawn(move || keep_folder(&folder, &asked, &tell, &Wake::default()));
        (asks, told)
    }

    /// The thread's next word, within a minute.
    fn next(told: &Receiver<Told>) -> Told {
        told.recv_timeout(std::time::Duration::from_secs(60))
            .unwrap()
    }

    /// The names in `folder`, sorted.
    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// The paths a [`Told::Pictures`] has pictures of.
    fn pictured(told: Told) -> Vec<String> {
        match told {
            Told::Pictures(found) => found.into_iter().map(|found| found.path).collect(),
            _ => panic!("not pictures"),
        }
    }

    /// `name` in `folder` written with `bytes`, modified `ago` before now.
    fn written_ago(folder: &Path, name: &str, bytes: &[u8], ago: Duration) {
        let path = folder.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - ago)
            .unwrap();
    }

    /// `n` hours.
    fn hours(n: u64) -> Duration {
        Duration::from_secs(n * 60 * 60)
    }

    /// The pictures of twelve documents no listed document is now, the first written last.
    fn twelve_others(folder: &Path) -> Vec<String> {
        (0..12)
            .map(|n| {
                let name = file_name(&key(format!("another document {n}").as_bytes()));
                written_ago(folder, &name, &plain_png(4, 4, [0; 3]), hours(n + 1));
                name
            })
            .collect()
    }

    #[test]
    fn a_look_keeps_the_listed_documents_pictures_and_the_ten_newest_others() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(FOLDER_NAME);
        std::fs::create_dir(&folder).unwrap();
        // Three documents: "abc", pictured; "" with a corrupt picture; one with none; and
        // one not there.
        let pictured_file = dir.path().join("a.silico");
        std::fs::write(&pictured_file, b"abc").unwrap();
        let corrupt = dir.path().join("b.silico");
        std::fs::write(&corrupt, b"").unwrap();
        let bare = dir.path().join("c.silico");
        std::fs::write(&bare, b"bare").unwrap();
        let gone = dir.path().join("gone.silico");
        written_ago(
            &folder,
            &file_name(ABC),
            &plain_png(4, 4, [1, 2, 3]),
            hours(30),
        );
        written_ago(&folder, &file_name(EMPTY), b"not a png", hours(30));
        let others = twelve_others(&folder);
        // Files that aren't pictures of the folder's: left, however old.
        let upper = format!("{}.png", key(b"x").to_uppercase());
        // Files a write left: removed once older than an hour, if named as a write names
        // them.
        let left = |n: u64| format!("{ABC}.png.4242.{n}.new");
        let not_left = [
            "notes.txt".to_owned(),
            upper,
            "picture.png.4242.1.new".to_owned(),
            format!("{ABC}.png.pid.1.new"),
            format!("{ABC}.png.4242..new"),
            format!("{ABC}.png.4242.1.old"),
        ];
        for name in &not_left {
            written_ago(&folder, name, b"", hours(30));
        }
        written_ago(&folder, &left(1), b"", hours(2));
        written_ago(&folder, &left(2), b"", Duration::from_secs(60));
        let (asks, told) = kept(&folder);
        let paths: Vec<String> = [&pictured_file, &corrupt, &bare, &gone]
            .iter()
            .map(|path| path.to_str().unwrap().to_owned())
            .collect();
        asks.send(Ask::Look(paths.clone())).unwrap();
        assert_eq!(pictured(next(&told)), [paths[0].clone()]);
        // The listed documents' two, the ten newest of the twelve others (the two written
        // longest ago removed), the write a minute old, and the rest.
        let mut want = vec![file_name(ABC), file_name(EMPTY), left(2)];
        want.extend(others[..10].iter().cloned());
        want.extend(not_left.iter().cloned());
        want.sort();
        assert_eq!(names(&folder), want);
        // A document no longer listed: its picture one of the others now, the oldest of
        // them, so removed.
        asks.send(Ask::Look(paths[1..].to_vec())).unwrap();
        assert!(pictured(next(&told)).is_empty());
        want.retain(|name| *name != file_name(ABC));
        assert_eq!(names(&folder), want);
    }

    #[test]
    fn a_listed_document_that_cant_be_read_has_no_picture_removed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(FOLDER_NAME);
        std::fs::create_dir(&folder).unwrap();
        let others = twelve_others(&folder);
        // Something at the listed path, but no document that can be read: its key unknown,
        // so every picture might be its.
        let unreadable = dir.path().join("folder.silico");
        std::fs::create_dir(&unreadable).unwrap();
        let unreadable = unreadable.to_str().unwrap().to_owned();
        let (asks, told) = kept(&folder);
        asks.send(Ask::Look(vec![unreadable])).unwrap();
        assert!(pictured(next(&told)).is_empty());
        let mut all = others.clone();
        all.sort();
        assert_eq!(names(&folder), all);
        // Not listed: the two oldest go.
        asks.send(Ask::Look(Vec::new())).unwrap();
        assert!(pictured(next(&told)).is_empty());
        let mut newest = others[..10].to_vec();
        newest.sort();
        assert_eq!(names(&folder), newest);
    }

    #[test]
    fn a_want_is_told_whether_its_texts_bytes_have_a_picture_and_a_write_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        // A folder not made yet.
        let folder = dir.path().join("app-data").join(FOLDER_NAME);
        let document = dir.path().join("a.silico");
        std::fs::write(&document, b"abc").unwrap();
        let (asks, told) = kept(&folder);
        let session = SessionId::parse("s1").unwrap();
        let revision = Revision::from_number(3).unwrap();
        let want = |text: &str| Ask::Want {
            session,
            revision,
            text: text.to_owned(),
        };
        asks.send(want("abc")).unwrap();
        assert!(matches!(
            next(&told),
            Told::Missing { key, .. } if key == ABC
        ));
        // Written before any look: nothing pruned or told.
        let png = plain_png(4, 4, [5, 6, 7]);
        asks.send(Ask::Write {
            of: Of::Key(ABC.to_owned()),
            png: png.clone(),
        })
        .unwrap();
        asks.send(want("abc")).unwrap();
        assert!(matches!(next(&told), Told::Have { .. }));
        assert_eq!(std::fs::read(folder.join(file_name(ABC))).unwrap(), png);
        // Listed, then the document saved as other bytes, and those drawn, keyed by its
        // text: the new one shown.
        let path = document.to_str().unwrap().to_owned();
        asks.send(Ask::Look(vec![path.clone()])).unwrap();
        assert_eq!(pictured(next(&told)), std::slice::from_ref(&path));
        std::fs::write(&document, b"").unwrap();
        asks.send(Ask::Write {
            of: Of::Text(String::new()),
            png: png.clone(),
        })
        .unwrap();
        assert_eq!(pictured(next(&told)), std::slice::from_ref(&path));
        let mut both = vec![file_name(ABC), file_name(EMPTY)];
        both.sort();
        assert_eq!(names(&folder), both);
        // A picture kept already isn't written again.
        asks.send(Ask::Write {
            of: Of::Key(EMPTY.to_owned()),
            png: plain_png(4, 4, [0; 3]),
        })
        .unwrap();
        next(&told);
        assert_eq!(std::fs::read(folder.join(file_name(EMPTY))).unwrap(), png);
        // A key whose file isn't a picture: missing, so drawn again over it.
        std::fs::write(folder.join(file_name(EMPTY)), b"PNG 260x160").unwrap();
        asks.send(want("")).unwrap();
        assert!(matches!(next(&told), Told::Missing { .. }));
    }

    /// Thumbs whose asks the test reads, and whose thread's words the test tells.
    fn listening() -> (Thumbs, Receiver<Ask>, Sender<Told>) {
        let (asks, asked) = mpsc::channel();
        let (tell, told) = mpsc::channel();
        let thumbs = Thumbs {
            asks: Some(asks),
            told: Some(told),
            ..Thumbs::unkept(Wake::default())
        };
        (thumbs, asked, tell)
    }

    /// A render's answer, `PNG`, of the published model at `revision`.
    fn rendered(revision: u64) -> Response<Rendering> {
        use silico_core::command::render::{AnsweredCamera, RenderResult};
        let camera = silico_protocol::Camera {
            projection: silico_protocol::Projection::Orthographic { view_height: 10.0 },
            eye: [10.0, -10.0, 10.0],
            target: [0.0; 3],
            up: [0.0, 0.0, 1.0],
        };
        Response {
            session: None,
            model: Some(silico_core::command::ModelRead {
                revision: Revision::from_number(revision).unwrap(),
                of: ModelChoice::Published,
                fault: None,
            }),
            result: Ok(Rendering {
                result: RenderResult {
                    png_bytes: 3,
                    path: None,
                    width: WIDTH,
                    height: HEIGHT,
                    camera: AnsweredCamera::of(&camera).unwrap(),
                },
                png: b"PNG".to_vec(),
            }),
        }
    }

    #[test]
    fn a_render_of_another_revisions_model_is_dropped_and_nothing_written() {
        let (mut thumbs, asked, _tell) = listening();
        let session = SessionId::parse("s1").unwrap();
        let three = Revision::from_number(3).unwrap();
        // Revision 4's model, published since revision 3's text was keyed: not its bytes.
        thumbs.took_render(session, three, Of::Key(ABC.to_owned()), rendered(4));
        assert_eq!((thumbs.renders_written, thumbs.renders_dropped), (0, 1));
        assert!(asked.try_recv().is_err(), "nothing to write");
        assert!(thumbs.pictured.is_empty());
        // Revision 3's: written, as its key's.
        thumbs.took_render(session, three, Of::Key(ABC.to_owned()), rendered(3));
        assert_eq!((thumbs.renders_written, thumbs.renders_dropped), (1, 1));
        assert!(matches!(
            asked.try_recv(),
            Ok(Ask::Write { of: Of::Key(key), png }) if key == ABC && png == b"PNG"
        ));
        assert_eq!(thumbs.pictured.get(&session), Some(&three));
        // A model read with none named: dropped.
        let mut unnamed = rendered(3);
        unnamed.model = None;
        thumbs.took_render(session, three, Of::Key(ABC.to_owned()), unnamed);
        assert_eq!((thumbs.renders_written, thumbs.renders_dropped), (1, 2));
    }

    #[test]
    fn a_word_of_another_revision_is_of_a_drawing_gone_and_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let sessions = Arc::new(Sessions::with_stand_ins(
            kernel,
            move |cache| silico_core::cache::Cache::with_secret(cache, &secret),
            || {
                Ok(Box::new(silico_core::fake::FakeSketches::new())
                    as Box<dyn silico_core::sketch::SketchSolver>)
            },
        ));
        let ctx = egui::Context::default();
        let (mut thumbs, _asked, tell) = listening();
        let (one, two) = (
            SessionId::parse("s1").unwrap(),
            SessionId::parse("s2").unwrap(),
        );
        let three = Revision::from_number(3).unwrap();
        let four = Revision::from_number(4).unwrap();
        for session in [one, two] {
            thumbs
                .drawing
                .insert(session, Drawing::Asking { revision: three });
        }
        let asking = |thumbs: &Thumbs, session| {
            matches!(
                thumbs.drawing.get(&session),
                Some(Drawing::Asking { revision }) if *revision == three
            )
        };
        // Revision 4's words, to drawings asking of revision 3's: nothing done.
        tell.send(Told::Have {
            session: one,
            revision: four,
        })
        .unwrap();
        tell.send(Told::Missing {
            session: two,
            revision: four,
            key: ABC.to_owned(),
        })
        .unwrap();
        thumbs.take_told(&ctx, &sessions);
        assert!(asking(&thumbs, one) && asking(&thumbs, two));
        assert_eq!(thumbs.already_pictured, 0);
        assert!(thumbs.pictured.is_empty());
        // Revision 3's: the one found pictured, the other drawn.
        tell.send(Told::Have {
            session: one,
            revision: three,
        })
        .unwrap();
        tell.send(Told::Missing {
            session: two,
            revision: three,
            key: ABC.to_owned(),
        })
        .unwrap();
        thumbs.take_told(&ctx, &sessions);
        assert!(!thumbs.drawing.contains_key(&one));
        assert_eq!(thumbs.already_pictured, 1);
        assert_eq!(thumbs.pictured.get(&one), Some(&three));
        assert!(matches!(
            thumbs.drawing.get(&two),
            Some(Drawing::Rendering { revision, key, .. }) if *revision == three && key == ABC
        ));
    }
    #[test]
    fn the_pictures_are_kept_beside_the_recent_documents() {
        let recents = Recents::default_file();
        let folder = Thumbs::default_folder();
        assert_eq!(recents.is_some(), folder.is_some());
        if let (Some(recents), Some(folder)) = (recents, folder) {
            assert_eq!(folder.parent(), recents.parent());
            assert_eq!(folder.file_name().unwrap(), "thumbnails");
        }
    }

    #[test]
    fn the_thread_asks_the_window_for_a_frame_when_it_tells_something() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let ctx = egui::Context::default();
        let asked = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&asked);
        ctx.set_request_repaint_callback(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let dir = tempfile::tempdir().unwrap();
        let thumbs = Thumbs::at(dir.path().join(FOLDER_NAME));
        thumbs.waking(&ctx);
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        thumbs.ask(Ask::Look(Vec::new()));
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while asked.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline, "no frame asked for");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_closing_tabs_text_refused_drops_its_picture_and_still_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        let kernel = Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let sessions = Arc::new(Sessions::with_stand_ins(
            kernel,
            move |cache| silico_core::cache::Cache::with_secret(cache, &secret),
            || {
                Ok(Box::new(silico_core::fake::FakeSketches::new())
                    as Box<dyn silico_core::sketch::SketchSolver>)
            },
        ));
        let (mut thumbs, asked, _tell) = listening();
        let refused = Response {
            session: None,
            model: None,
            result: Err(silico_core::command::Problems::from(
                silico_core::Error::new(
                    silico_core::ErrorCode::WorkerCrashed,
                    "the text wasn't read",
                ),
            )),
        };
        thumbs.closing(Closing {
            session: SessionId::parse("s1").unwrap(),
            revision: Revision::from_number(3).unwrap(),
            path: "/a/part.silico".to_owned(),
            stage: Stage::Drawing(Box::new(Out {
                text: Waiting::Taken(refused),
                rendering: Waiting::Taken(rendered(3)),
            })),
        });
        thumbs.poll_closing(&sessions);
        assert_eq!((thumbs.renders_written, thumbs.renders_dropped), (0, 1));
        assert!(asked.try_recv().is_err(), "nothing to write");
        assert!(thumbs.pictured.is_empty());
        assert!(matches!(
            thumbs.closing.as_slice(),
            [Closing {
                stage: Stage::Closing(_),
                ..
            }]
        ));
    }

    #[test]
    fn a_file_a_write_left_is_removed_once_more_than_an_hour_old() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let name = format!("{ABC}.png.4242.1.new");
        let file = folder.join(&name);
        std::fs::write(&file, b"").unwrap();
        let written = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(written)
            .unwrap();
        // 59 minutes, then exactly an hour: kept. A second more: removed.
        for (age, kept) in [(59 * 60, true), (60 * 60, true), (60 * 60 + 1, false)] {
            prune(folder, None, written + Duration::from_secs(age));
            assert_eq!(names(folder) == [name.clone()], kept, "{age} s");
        }
    }
}
