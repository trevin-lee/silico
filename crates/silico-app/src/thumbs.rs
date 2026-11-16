//! The start page's thumbnails: each recent document's row shows a picture of its part (the
//! prototype's `.recent-row .thumb`, docs/design/m5-ui/js/app.js's `thumb()`, a 260 by 160
//! render cut to the 52 by 40 cell as `object-fit: cover` cuts it), or its icon if there's
//! none.
//!
//! A picture is the core's `render` of a tab's model, Home's view at [`WIDTH`] by
//! [`HEIGHT`], asked once the model is built and the document is as its file holds it: a
//! session outside a transaction whose model is published at its revision, as it is once a
//! document opens, once a Save or a Save As writes it, and once a reload takes another
//! program's edit. It's kept in a folder of the app's own beside the recent documents
//! ([`Thumbs::default_folder`]), named by [`key`], the SHA-256 of the file's bytes the model
//! was built from, which the session's `text` at the model's revision gives exactly. So a
//! picture is shown only for the exact bytes it was drawn from: a file changed since (an
//! agent's edit) shows the icon until the app draws it again. A model whose key already has
//! a picture isn't drawn again.
//!
//! The folder is kept by a thread of its own, so the UI thread never touches the disk
//! (invariant 4): it hashes each recent document's file, reads and decodes the picture its
//! key names, and cuts it to the cell, ready to upload; it writes each picture drawn; and it
//! removes every picture no recent document's bytes name, so the folder holds at most as
//! many as Recent lists. A picture that can't be read, is more than [`MAX_PNG_BYTES`], isn't
//! a PNG, or is more than [`MAX_SIDE`] a side is no picture: the icon shows. A render that
//! fails (nothing built, no bodies, no render worker) is logged, never said: a thumbnail is
//! a nicety.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use sha2::{Digest as _, Sha256};
use silico_core::command::render::{Render, Rendering};
use silico_core::command::wire::Named;
use silico_core::command::{ModelChoice, Revision, Running, SessionId, Sessions, TextAnswer};
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

/// How many of a picture's pixels a point of the cell has, each way: a Retina display's 2
/// shows it pixel for pixel, a display of 1 at half (egui's wgpu textures have no mipmaps,
/// so the thread cuts it down, averaging, rather than leave a quarter of a 260 by 160
/// render to linear filtering).
pub const PICTURE_SCALE: f32 = 2.0;

/// The picture's place in the cell: inside its 1-point border (`box-sizing: border-box`).
pub const CELL: egui::Vec2 = egui::vec2(
    theme::START_THUMB_WIDTH - 2.0 * theme::BORDER_WIDTH,
    theme::START_THUMB_HEIGHT - 2.0 * theme::BORDER_WIDTH,
);

/// A picture's size in pixels as the thread gives it: [`CELL`] at [`PICTURE_SCALE`], 100 by
/// 76.
pub const PICTURE: [usize; 2] = [
    (CELL.x * PICTURE_SCALE) as usize,
    (CELL.y * PICTURE_SCALE) as usize,
];

/// The key a picture of a document is kept by: the SHA-256 of its file's bytes, in lower-case
/// hexadecimal.
pub fn key(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// `bytes` in lower-case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The name of the file the picture keyed `key` is kept in.
pub fn file_name(key: &str) -> String {
    format!("{key}.png")
}

/// The key a file in the folder named `name` is a picture of, if its name is one
/// [`file_name`] gives: never another file, which is left alone.
fn key_named(name: &str) -> Option<&str> {
    let key = name.strip_suffix(".png")?;
    (key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(key)
}

/// The key of the document at `path`, as its bytes are now: none if it isn't a regular file
/// (a pipe could hang the read), can't be read, or is longer than a document can be
/// (`json::MAX_DOCUMENT_LEN`), which no session opens.
pub fn key_of_file(path: &Path) -> Option<String> {
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let limit = silico_core::document::json::MAX_DOCUMENT_LEN as u64;
    let mut hasher = Sha256::new();
    let read = std::io::copy(
        &mut std::fs::File::open(path).ok()?.take(limit + 1),
        &mut hasher,
    )
    .ok()?;
    (read <= limit).then(|| hex(&hasher.finalize()))
}

/// The picture in `file`, cut to [`PICTURE`]: none if it can't be read, is longer than
/// [`MAX_PNG_BYTES`], or isn't a picture ([`picture`]).
fn read_picture(file: &Path) -> Option<ColorImage> {
    if !std::fs::metadata(file).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .ok()?
        .take(MAX_PNG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_PNG_BYTES {
        return None;
    }
    picture(&bytes)
}

/// `png` decoded and cut to [`PICTURE`] as `object-fit: cover` cuts it ([`cover`]): none if
/// it isn't a PNG the `png` crate decodes, or either side is 0 or more than [`MAX_SIDE`].
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
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
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

/// What the window asks the folder's thread.
enum Ask {
    /// The recent documents' paths, as Recent lists them: each one's picture found, and every
    /// picture none of them names removed.
    Look(Vec<String>),
    /// Whether a picture keyed `key` is kept, for `session`'s model at `revision`
    /// ([`Told::Have`], [`Told::Missing`]).
    Want {
        session: SessionId,
        revision: Revision,
        key: String,
    },
    /// A picture drawn, `png`, to keep as `key`'s.
    Keep { key: String, png: Vec<u8> },
}

/// What the folder's thread tells the window.
enum Told {
    /// The pictures of the recent documents last looked at, by path: those that have one.
    Pictures(Vec<Found>),
    /// A picture is kept as the key `session`'s model at `revision` was asked for: it isn't
    /// drawn again.
    Have {
        session: SessionId,
        revision: Revision,
    },
    /// No picture is kept as `key`'s: `session`'s model at `revision` is to be drawn.
    Missing {
        session: SessionId,
        revision: Revision,
        key: String,
    },
}

/// Where a tab's picture stands, for the revision it's of.
enum Drawing {
    /// The text the model was built from, read to key it.
    Reading {
        revision: Revision,
        running: Running<TextAnswer>,
    },
    /// The thread asked whether its key has a picture.
    Asking { revision: Revision },
    /// Drawn by the core's `render`.
    Rendering {
        revision: Revision,
        key: String,
        running: Running<Rendering>,
    },
    /// Done with: kept, already had, or not drawn.
    Done { revision: Revision },
}

impl Drawing {
    fn revision(&self) -> Revision {
        match self {
            Drawing::Reading { revision, .. }
            | Drawing::Asking { revision }
            | Drawing::Rendering { revision, .. }
            | Drawing::Done { revision } => *revision,
        }
    }

    /// Stopped, if a command of its is running: one for a revision gone by.
    fn cancel(&self) {
        match self {
            Drawing::Reading { running, .. } => {
                running.cancel();
            }
            Drawing::Rendering { running, .. } => {
                running.cancel();
            }
            Drawing::Asking { .. } | Drawing::Done { .. } => {}
        }
    }
}

/// A picture uploaded, and the key it's of.
struct Shown {
    key: String,
    texture: TextureHandle,
}

/// The recent documents' pictures, as the window last heard them from the thread that keeps
/// them, and each open tab's being drawn.
pub struct Thumbs {
    asks: Option<Sender<Ask>>,
    told: Option<Receiver<Told>>,
    wake: Wake,
    shown: BTreeMap<String, Shown>,
    drawing: BTreeMap<SessionId, Drawing>,
    /// The paths last told the thread, and whether the start page was shown last frame.
    looked: Option<Vec<String>>,
    was_shown: bool,
    /// How many pictures have been drawn and sent to be kept, how many models were found to
    /// have one already, so weren't drawn, and how many times the thread has told the recent
    /// documents' pictures: what a test counts.
    pub kept: u64,
    pub had: u64,
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
            .spawn(move || keep(&folder, &asked, &tell, &waking));
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
            looked: None,
            was_shown: false,
            kept: 0,
            had: 0,
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

    /// One frame's worth, never waiting: what the thread told taken (pictures uploaded to
    /// `ctx`, and drawings it found missing started); each tab of `tabs` whose model is
    /// built at its revision, outside a transaction, and has a file, keyed and drawn if its
    /// key has no picture; and, once `recents` is read, its paths told the thread each time
    /// they change and each time the start page is `shown` again. Nothing at all while the
    /// app keeps no list of recent documents: there's nowhere to show a picture.
    pub(crate) fn frame<P: Clients + 'static>(
        &mut self,
        ctx: &egui::Context,
        sessions: &Arc<Sessions<P>>,
        tabs: &[Tab],
        recents: Option<&Recents>,
        shown: bool,
    ) {
        let Some(recents) = recents.filter(|recents| recents.is_kept()) else {
            return;
        };
        self.take_told(ctx, sessions);
        self.drawing
            .retain(|session, _| tabs.iter().any(|tab| tab.id == *session));
        for tab in tabs {
            self.draw(sessions, tab);
        }
        if recents.is_read() {
            let paths: Vec<String> = recents
                .list()
                .iter()
                .map(|recent| recent.path.clone())
                .collect();
            if self.looked.as_ref() != Some(&paths) || (shown && !self.was_shown) {
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
                        self.drawing.insert(session, Drawing::Done { revision });
                        self.had += 1;
                    }
                }
                Told::Missing {
                    session,
                    revision,
                    key,
                } => {
                    if self.asked(session, revision) {
                        let request = Render {
                            view: Some(crate::navigate::HOME),
                            camera: None,
                            width: WIDTH,
                            height: HEIGHT,
                            highlight: Vec::new(),
                            to: None,
                            replace: false,
                        };
                        let running = sessions.render(&session, ModelChoice::Published, request);
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
    /// `revision` has a picture.
    fn asked(&self, session: SessionId, revision: Revision) -> bool {
        matches!(
            self.drawing.get(&session),
            Some(Drawing::Asking { revision: asked }) if *asked == revision
        )
    }

    /// `tab`'s drawing polled, and one started for its revision if it's built, outside a
    /// transaction, and has a file, and none is under way for that revision (one for a
    /// revision gone by stopped).
    fn draw<P: Clients + 'static>(&mut self, sessions: &Arc<Sessions<P>>, tab: &Tab) {
        let drawing = self
            .drawing
            .remove(&tab.id)
            .map(|drawing| self.answered(tab.id, drawing));
        let wanted = tab.path.is_some() && !tab.transaction && tab.published == Some(tab.revision);
        let drawing = match drawing {
            Some(drawing) if !wanted || drawing.revision() == tab.revision => Some(drawing),
            gone_by => {
                if let Some(gone_by) = gone_by {
                    gone_by.cancel();
                }
                wanted.then(|| Drawing::Reading {
                    revision: tab.revision,
                    running: sessions.text_at(&tab.id, None, Some(Named::Revision(tab.revision))),
                })
            }
        };
        if let Some(drawing) = drawing {
            self.drawing.insert(tab.id, drawing);
        }
    }

    /// `drawing`, of `session`, moved on by its command's answer, if it has come.
    fn answered(&mut self, session: SessionId, drawing: Drawing) -> Drawing {
        match drawing {
            Drawing::Reading { revision, running } => match running.try_take() {
                Ok(response) => {
                    match response.result {
                        Ok(answer) => {
                            self.ask(Ask::Want {
                                session,
                                revision,
                                key: key(answer.text.as_bytes()),
                            });
                            return Drawing::Asking { revision };
                        }
                        Err(problems) => tracing::info!(
                            %session,
                            "no thumbnail: the text of revision {} wasn't read: {}",
                            revision.number(),
                            crate::tab::said(&problems)
                        ),
                    }
                    Drawing::Done { revision }
                }
                Err(running) => Drawing::Reading { revision, running },
            },
            Drawing::Rendering {
                revision,
                key,
                running,
            } => match running.try_take() {
                Ok(response) => {
                    let of = response.model.as_ref().map(|model| model.revision);
                    match response.result {
                        Ok(rendering) if of == Some(revision) => {
                            self.ask(Ask::Keep {
                                key,
                                png: rendering.png,
                            });
                            self.kept += 1;
                        }
                        // Another revision's model, published since: not the bytes keyed.
                        Ok(_) => tracing::info!(
                            %session,
                            "no thumbnail: the model drawn wasn't revision {}'s",
                            revision.number()
                        ),
                        Err(problems) => tracing::info!(
                            %session,
                            "no thumbnail: the render was refused: {}",
                            crate::tab::said(&problems)
                        ),
                    }
                    Drawing::Done { revision }
                }
                Err(running) => Drawing::Rendering {
                    revision,
                    key,
                    running,
                },
            },
            asking_or_done => asking_or_done,
        }
    }
}

/// The thread that keeps the folder: each ask taken as it comes. A look finds each recent
/// document's picture and removes every other; a want tells back whether a key has a
/// picture (a file that isn't one doesn't count); a keep writes a picture, removes every
/// other the recent documents last looked at don't name, and looks again, so a picture drawn
/// shows.
fn keep(folder: &Path, asked: &Receiver<Ask>, tell: &Sender<Told>, wake: &Wake) {
    let _span = tracing::info_span!("thumbnails", folder = %folder.display()).entered();
    let mut listed: Option<Vec<String>> = None;
    while let Ok(ask) = asked.recv() {
        let told = match ask {
            Ask::Look(paths) => {
                let (found, keys) = look(folder, &paths);
                remove_all_but(folder, &keys);
                listed = Some(paths);
                Some(Told::Pictures(found))
            }
            Ask::Want {
                session,
                revision,
                key,
            } => Some(
                // One that isn't a picture (a corrupt file) is drawn again, over it.
                if read_picture(&folder.join(file_name(&key))).is_some() {
                    Told::Have { session, revision }
                } else {
                    Told::Missing {
                        session,
                        revision,
                        key,
                    }
                },
            ),
            Ask::Keep { key, png } => {
                let file = folder.join(file_name(&key));
                if let Err(e) = crate::app_data::write_bytes(&file, &png) {
                    tracing::info!("a thumbnail couldn't be written to {}: {e}", file.display());
                }
                listed.as_ref().map(|paths| {
                    // The picture just kept stays, whether or not its document is listed yet
                    // (a Save As's new path is listed once Recent's thread has written it).
                    let (found, mut keys) = look(folder, paths);
                    keys.insert(key);
                    remove_all_but(folder, &keys);
                    Told::Pictures(found)
                })
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

/// The pictures of the documents at `paths` found in `folder`, and the keys of every one of
/// those documents that could be read, pictured or not.
fn look(folder: &Path, paths: &[String]) -> (Vec<Found>, BTreeSet<String>) {
    let mut found = Vec::new();
    let mut keys = BTreeSet::new();
    for path in paths {
        let Some(key) = key_of_file(Path::new(path)) else {
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
    (found, keys)
}

/// Every picture in `folder` but those of `keys` removed: only files named as pictures are
/// (anything else there is left).
fn remove_all_but(folder: &Path, keys: &BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(key) = name.to_str().and_then(key_named) else {
            continue;
        };
        if !keys.contains(key)
            && let Err(e) = std::fs::remove_file(entry.path())
        {
            tracing::info!("a thumbnail couldn't be removed: {e}");
        }
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
        std::thread::spawn(move || keep(&folder, &asked, &tell, &Wake::default()));
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

    #[test]
    fn a_look_finds_each_listed_documents_picture_and_removes_every_other() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(FOLDER_NAME);
        std::fs::create_dir(&folder).unwrap();
        // Three documents: "abc", pictured; "" with a corrupt picture; one with none.
        let pictured_file = dir.path().join("a.silico");
        std::fs::write(&pictured_file, b"abc").unwrap();
        let corrupt = dir.path().join("b.silico");
        std::fs::write(&corrupt, b"").unwrap();
        let bare = dir.path().join("c.silico");
        std::fs::write(&bare, b"bare").unwrap();
        let gone = dir.path().join("gone.silico");
        std::fs::write(folder.join(file_name(ABC)), plain_png(4, 4, [1, 2, 3])).unwrap();
        std::fs::write(folder.join(file_name(EMPTY)), b"not a png").unwrap();
        // Pictures of what no listed document is now: the old bytes of one, another's.
        let old = key(b"what a.silico was");
        std::fs::write(folder.join(file_name(&old)), plain_png(4, 4, [0; 3])).unwrap();
        let other = key(b"another document");
        std::fs::write(folder.join(file_name(&other)), plain_png(4, 4, [0; 3])).unwrap();
        // And files that aren't pictures of the folder's: left.
        let upper = format!("{}.png", key(b"x").to_uppercase());
        std::fs::write(folder.join(&upper), b"").unwrap();
        std::fs::write(folder.join("notes.txt"), b"").unwrap();
        let (asks, told) = kept(&folder);
        let paths: Vec<String> = [&pictured_file, &corrupt, &bare, &gone]
            .iter()
            .map(|path| path.to_str().unwrap().to_owned())
            .collect();
        asks.send(Ask::Look(paths.clone())).unwrap();
        assert_eq!(pictured(next(&told)), [paths[0].clone()]);
        let mut want = vec![
            file_name(ABC),
            file_name(EMPTY),
            upper.clone(),
            "notes.txt".to_owned(),
        ];
        want.sort();
        assert_eq!(names(&folder), want);
        // A document no longer listed: its picture removed too.
        asks.send(Ask::Look(paths[1..].to_vec())).unwrap();
        assert!(pictured(next(&told)).is_empty());
        let mut want = vec![file_name(EMPTY), upper, "notes.txt".to_owned()];
        want.sort();
        assert_eq!(names(&folder), want);
    }

    #[test]
    fn a_want_is_told_whether_its_key_has_a_picture_and_a_keep_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        // A folder not made yet.
        let folder = dir.path().join("app-data").join(FOLDER_NAME);
        let document = dir.path().join("a.silico");
        std::fs::write(&document, b"abc").unwrap();
        let (asks, told) = kept(&folder);
        let session = SessionId::parse("s1").unwrap();
        let revision = Revision::from_number(3).unwrap();
        let want = |key: &str| Ask::Want {
            session,
            revision,
            key: key.to_owned(),
        };
        asks.send(want(ABC)).unwrap();
        assert!(matches!(
            next(&told),
            Told::Missing { key, .. } if key == ABC
        ));
        // Kept before any look: written, nothing removed or told.
        let png = plain_png(4, 4, [5, 6, 7]);
        asks.send(Ask::Keep {
            key: ABC.to_owned(),
            png: png.clone(),
        })
        .unwrap();
        asks.send(want(ABC)).unwrap();
        assert!(matches!(next(&told), Told::Have { .. }));
        assert_eq!(std::fs::read(folder.join(file_name(ABC))).unwrap(), png);
        // Listed, then the document saved as other bytes, and those drawn: the old picture
        // removed, the new one shown.
        let path = document.to_str().unwrap().to_owned();
        asks.send(Ask::Look(vec![path.clone()])).unwrap();
        assert_eq!(pictured(next(&told)), std::slice::from_ref(&path));
        std::fs::write(&document, b"").unwrap();
        asks.send(Ask::Keep {
            key: EMPTY.to_owned(),
            png: png.clone(),
        })
        .unwrap();
        assert_eq!(pictured(next(&told)), std::slice::from_ref(&path));
        assert_eq!(names(&folder), [file_name(EMPTY)]);
        // A picture of a document not listed yet (a Save As's new path) stays, kept.
        asks.send(Ask::Keep {
            key: ABC.to_owned(),
            png,
        })
        .unwrap();
        assert_eq!(pictured(next(&told)), [path]);
        let mut both = vec![file_name(ABC), file_name(EMPTY)];
        both.sort();
        assert_eq!(names(&folder), both);
        // A key whose file isn't a picture: missing, so drawn again over it.
        std::fs::write(folder.join(file_name(EMPTY)), b"PNG 260x160").unwrap();
        asks.send(want(EMPTY)).unwrap();
        assert!(matches!(next(&told), Told::Missing { .. }));
    }
}
