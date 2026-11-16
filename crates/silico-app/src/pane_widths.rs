//! The side panes' widths (the browser's column and the property panel's), as the person
//! last dragged their borders (`crate::columns`), and whether the browser is collapsed to its
//! strip, kept in a file of the app's own in the platform's app-data folder, beside the
//! recent documents ([`KeptWidths::default_file`]), so the window opens laid out as it was
//! left.
//!
//! The file is read on a thread of its own, which writes it at the end of each drag and as
//! the browser is collapsed or shown, so the UI thread never waits on it (invariant 4): the
//! window asks the thread ([`KeptWidths::save`]) and takes what it read
//! ([`KeptWidths::poll`]). Its form is a header line, then the browser's width and the
//! property panel's, each on a line of its own in whole points, then, only while the browser
//! is collapsed, `browser collapsed` (`text`); a file that isn't one (`parse`), or can't be
//! read, is the defaults, never a failure; and a width out of range is clamped into it
//! ([`Widths::clamped`]). At most [`MAX_FILE_BYTES`] of it is read.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

pub use crate::app_data::Read;
use crate::wake::Wake;

/// The file's name in the app-data folder.
pub const FILE_NAME: &str = "pane-widths";

/// The file's first line: its form, and the form's version.
pub(crate) const HEADER: &str = "silico pane-widths 1";

/// The most of the file read: its header and two widths take a few dozen bytes. Whatever
/// comes after it, and the line it cuts, is left unread.
pub const MAX_FILE_BYTES: usize = 4096;

/// The side panes' widths at first, as the prototype's `#main` lays them out (styles.css:
/// `grid-template-columns: var(--browser-w) 1fr var(--right-w)`, `--browser-w: 248px`,
/// `--right-w: 296px`), each column with its 1 px border inside it (`#browser`'s
/// `border-right`, `#right`'s `border-left`, `box-sizing: border-box`).
pub(crate) const BROWSER_W: f32 = 248.0;
pub(crate) const PROPERTIES_W: f32 = 296.0;

/// The browser's column collapsed to its strip (`#main.browser-collapsed`:
/// `grid-template-columns: 28px 1fr var(--right-w)`), its border inside it as the open
/// column's is; whatever width it was dragged to is kept for when it's shown again.
pub(crate) const BROWSER_STRIP_W: f32 = 28.0;

/// The browser's least column (its border in it), from what its deepest row holds (`.row`,
/// `crate::panes`' `row`): a body's or a sketch's, two levels in, 6 of padding and two
/// indents of 18, then its caret's place (14), its eye's (18), and its icon (14), each 4
/// before the next part, 100 in all; a sketch's prefix, `(–)` in the monospace type (19.8
/// in the app's fonts), and 4; at its right a health mark (14) 4 after its name, and 6 of
/// padding; so a sketch's default name with two digits, `Sketch12`, whole, which a pane 199
/// wide holds exactly (egui fits a name by its last glyph's ink, 51.0, not its advance,
/// 52.2) and one a point narrower cuts short (`panes`' test `at_the_least_browser_a_deep
/// _rows_parts_and_a_default_name_fit` draws both); with its border, 200, which is on the
/// 8-point grid the prototype's columns are on (248, 296). A longer name is cut short with
/// an ellipsis, as at any width.
pub(crate) const BROWSER_MIN_W: f32 = 200.0;

/// The property panel's least column (its border in it), from its rows (`.prop`: `--pad`
/// 12 at each side, the key's column 96, 8 to the value) with a value's column as wide as
/// the key's, so a dialog's field holds a value and its unit, 224; with its border, 225. A
/// dialog's footer (`.dlg-footer`) then gives Cancel and OK (224 less its padding, 12 at
/// each side, and the 6 between them) 97 each, room for Cancel with its key cap, 95.6 in
/// the app's fonts (`properties`' test `at_the_least_property_panel_a_rows_columns_and_the
/// _footers_buttons_fit` measures them); rounded up to the 8-point grid, 232.
pub(crate) const PROPERTIES_MIN_W: f32 = 232.0;

/// The widest a file may make a column: wider than any display is in points (an 8K one at
/// its native scale is 7,680). A wider one is clamped to it; a drag never makes one wider
/// than its window leaves it.
pub(crate) const WIDEST: f32 = 8192.0;

/// The side panes' columns, each its border included, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    /// The browser's, at the window's left.
    pub browser: f32,
    /// The property panel's, at the window's right.
    pub properties: f32,
}

impl Default for Widths {
    fn default() -> Self {
        Widths {
            browser: BROWSER_W,
            properties: PROPERTIES_W,
        }
    }
}

impl Widths {
    /// Each in its range: no narrower than its least ([`BROWSER_MIN_W`],
    /// [`PROPERTIES_MIN_W`]), no wider than [`WIDEST`]; one that isn't a number, its least.
    pub fn clamped(self) -> Self {
        Widths {
            browser: within(self.browser, BROWSER_MIN_W),
            properties: within(self.properties, PROPERTIES_MIN_W),
        }
    }
}

/// `width` from `min` to [`WIDEST`]; `min` if it isn't a number (`clamp` keeps a NaN).
fn within(width: f32, min: f32) -> f32 {
    if width.is_nan() {
        min
    } else {
        width.clamp(min, WIDEST)
    }
}

/// What the file keeps: the side panes' widths, and whether the browser is collapsed to its
/// strip ([`BROWSER_STRIP_W`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Kept {
    pub widths: Widths,
    pub browser_collapsed: bool,
}

/// The line that says the browser is collapsed, after the widths.
const COLLAPSED: &str = "browser collapsed";

/// What the file `bytes` keeps, its widths clamped ([`Widths::clamped`]): the defaults (the
/// browser shown) unless it's UTF-8, its first line [`HEADER`], and the two lines after it
/// `browser N` and `properties N`, in that order, each `N` a whole number of points in
/// decimal digits, then either nothing, the browser shown (as every file was written before
/// the browser collapsed), or `browser collapsed` and nothing after it (the last line's break
/// optional).
pub(crate) fn parse(bytes: &[u8]) -> Kept {
    let parsed = || {
        let text = std::str::from_utf8(bytes).ok()?;
        let body = text.strip_prefix(HEADER)?.strip_prefix('\n')?;
        let mut lines = body.split_terminator('\n');
        let browser = points(lines.next()?.strip_prefix("browser ")?)?;
        let properties = points(lines.next()?.strip_prefix("properties ")?)?;
        let browser_collapsed = match lines.next() {
            None => false,
            Some(COLLAPSED) => true,
            Some(_) => return None,
        };
        lines.next().is_none().then_some(Kept {
            widths: Widths {
                browser,
                properties,
            }
            .clamped(),
            browser_collapsed,
        })
    };
    parsed().unwrap_or_default()
}

/// `text` as a width, if it's decimal digits alone: a number past `u32`'s range is past
/// [`WIDEST`], and clamped to it as any wider one is.
fn points(text: &str) -> Option<f32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(text.parse::<u32>().map_or(WIDEST, |points| points as f32))
}

/// The file's text for `kept`: [`HEADER`], then each width, clamped, to the nearest whole
/// point, on a line of its own, then `browser collapsed` on one of its own if it is.
pub(crate) fn text(kept: Kept) -> String {
    let widths = kept.widths.clamped();
    let collapsed = if kept.browser_collapsed {
        format!("{COLLAPSED}\n")
    } else {
        String::new()
    };
    format!(
        "{HEADER}\nbrowser {}\nproperties {}\n{collapsed}",
        widths.browser.round(),
        widths.properties.round()
    )
}

/// The panes' widths, and whether the browser is collapsed, as the file holds them, as the
/// window last heard them from the thread that keeps them.
pub struct KeptWidths {
    /// Whether the file has been read (or found unreadable) yet.
    read: bool,
    /// What the file keeps, once read, until the window takes it.
    kept: Option<Kept>,
    asks: Option<Sender<Kept>>,
    told: Option<Receiver<Kept>>,
    wake: Wake,
}

impl KeptWidths {
    /// The widths kept in `file`, read on a thread of its own (`app_data::read_capped`: a
    /// regular file, never a pipe that could hang it, and at most [`MAX_FILE_BYTES`] of it).
    pub fn at(file: PathBuf) -> Self {
        Self::reading(
            file,
            Box::new(|file| crate::app_data::read_capped(file, MAX_FILE_BYTES)),
        )
    }

    /// The widths kept in `file`, read first by `read` on a thread of its own.
    pub fn reading(file: PathBuf, read: Read) -> Self {
        let (asks, asked) = mpsc::channel();
        let (tell, told) = mpsc::channel();
        let wake = Wake::default();
        let waking = wake.clone();
        let started = std::thread::Builder::new()
            .name("silico-pane-widths".to_owned())
            .spawn(move || keep(&file, read, &asked, &tell, &waking));
        match started {
            Ok(_) => KeptWidths {
                read: false,
                kept: None,
                asks: Some(asks),
                told: Some(told),
                wake,
            },
            Err(e) => {
                tracing::warn!("the panes' widths can't be kept: {e}");
                Self::unkept()
            }
        }
    }

    /// Widths that aren't kept, their thread not started: read, the defaults, and no drag's
    /// written anywhere.
    pub(crate) fn unkept() -> Self {
        KeptWidths {
            read: true,
            kept: None,
            asks: None,
            told: None,
            wake: Wake::default(),
        }
    }

    /// Where the widths are kept: `pane-widths` in the platform's app-data folder
    /// (`app_data::folder`), beside `recent-documents`; none if the platform names no home
    /// folder.
    pub fn default_file() -> Option<PathBuf> {
        crate::app_data::folder().map(|folder| folder.join(FILE_NAME))
    }

    /// Whether the file has been read (or found unreadable) yet.
    pub fn is_read(&self) -> bool {
        self.read
    }

    /// Whether the widths are kept: false if their thread couldn't be started.
    pub fn is_kept(&self) -> bool {
        self.asks.is_some()
    }

    /// What was read from the file, once, the first poll after the thread tells it, never
    /// waited for.
    pub(crate) fn poll(&mut self) -> Option<Kept> {
        if let Some(told) = &self.told {
            for kept in told.try_iter() {
                self.kept = Some(kept);
                self.read = true;
            }
        }
        self.kept.take()
    }

    /// `kept` kept: written to the file by the thread.
    pub(crate) fn save(&self, kept: Kept) {
        if let Some(asks) = &self.asks {
            // A thread that has ended keeps nothing more; the window keeps its layout.
            let _ = asks.send(kept);
        }
    }

    /// The thread asks `ctx` for a frame when it tells the widths it read.
    pub(crate) fn waking(&self, ctx: &eframe::egui::Context) {
        self.wake.set(ctx);
    }
}

/// The thread that keeps the widths: `file` read once and what it keeps told (the defaults,
/// if it can't be read); then each ask taken as it comes, the newest of those already
/// waiting, and written, even what the file was last read or written with: another instance
/// of the app may have written it since, and the last drag (or collapse) to end is what the
/// file keeps.
fn keep(file: &Path, read: Read, asked: &Receiver<Kept>, tell: &Sender<Kept>, wake: &Wake) {
    let _span = tracing::info_span!("pane widths", file = %file.display()).entered();
    let kept = read(file).map_or_else(Kept::default, |bytes| parse(&bytes));
    if tell.send(kept).is_err() {
        return;
    }
    wake.wake();
    while let Ok(first) = asked.recv() {
        let newest = asked.try_iter().last().unwrap_or(first);
        if let Err(e) = crate::app_data::write(file, &text(newest)) {
            tracing::warn!("the panes' widths couldn't be written: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `widths` with the browser shown.
    fn shown(widths: Widths) -> Kept {
        Kept {
            widths,
            browser_collapsed: false,
        }
    }

    #[test]
    fn widths_are_read_as_written() {
        let widths = Widths {
            browser: 300.0,
            properties: 260.0,
        };
        assert_eq!(
            text(shown(widths)),
            "silico pane-widths 1\nbrowser 300\nproperties 260\n"
        );
        assert_eq!(parse(text(shown(widths)).as_bytes()), shown(widths));
        // The last line's break is optional; leading zeros are the number's.
        assert_eq!(
            parse(b"silico pane-widths 1\nbrowser 0300\nproperties 260"),
            shown(widths)
        );
        // Written to the nearest whole point.
        let halves = Widths {
            browser: 300.5,
            properties: 260.4,
        };
        assert_eq!(
            text(shown(halves)),
            "silico pane-widths 1\nbrowser 301\nproperties 260\n"
        );
        // The defaults, as the prototype's columns: 248 and 296.
        assert_eq!(
            text(Kept::default()),
            "silico pane-widths 1\nbrowser 248\nproperties 296\n"
        );
    }

    #[test]
    fn anything_else_is_the_defaults() {
        let defaults = Widths {
            browser: 248.0,
            properties: 296.0,
        };
        let defaults = shown(defaults);
        for bytes in [
            &b""[..],
            b"\xff\xfe",
            b"silico pane-widths 1",
            b"silico pane-widths 1\n",
            b"silico pane-widths 2\nbrowser 300\nproperties 260\n",
            b"silico recent-documents 1\nbrowser 300\nproperties 260\n",
            b"browser 300\nproperties 260\n",
            b" silico pane-widths 1\nbrowser 300\nproperties 260\n",
            b"silico pane-widths 1\r\nbrowser 300\nproperties 260\n",
            // A line missing, out of order, twice, or one more.
            b"silico pane-widths 1\nbrowser 300\n",
            b"silico pane-widths 1\nproperties 260\nbrowser 300\n",
            b"silico pane-widths 1\nbrowser 300\nbrowser 300\n",
            b"silico pane-widths 1\nbrowser 300\nproperties 260\nbrowser 300\n",
            b"silico pane-widths 1\nbrowser 300\nproperties 260\n\n",
            // A number that isn't whole decimal digits.
            b"silico pane-widths 1\nbrowser 300.5\nproperties 260\n",
            b"silico pane-widths 1\nbrowser -300\nproperties 260\n",
            b"silico pane-widths 1\nbrowser +300\nproperties 260\n",
            b"silico pane-widths 1\nbrowser \nproperties 260\n",
            b"silico pane-widths 1\nbrowser 3e2\nproperties 260\n",
            b"silico pane-widths 1\nbrowser 0x12c\nproperties 260\n",
            b"silico pane-widths 1\nbrowser NaN\nproperties 260\n",
            b"silico pane-widths 1\nbrowser 300\r\nproperties 260\n",
            b"silico pane-widths 1\nbrowser  300\nproperties 260\n",
            b"silico pane-widths 1\nbrowser 300 \nproperties 260\n",
            b"silico pane-widths 1\nbrowser\t300\nproperties 260\n",
            b"silico pane-widths 1\nBrowser 300\nproperties 260\n",
            b"silico pane-widths 1\nbrowser \xd9\xa3\nproperties 260\n",
        ] {
            assert_eq!(
                parse(bytes),
                defaults,
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn a_collapsed_browser_is_a_line_of_its_own_after_the_widths() {
        let collapsed = Kept {
            widths: Widths {
                browser: 320.0,
                properties: 300.0,
            },
            browser_collapsed: true,
        };
        let written = "silico pane-widths 1\nbrowser 320\nproperties 300\nbrowser collapsed\n";
        assert_eq!(text(collapsed), written);
        assert_eq!(parse(written.as_bytes()), collapsed);
        // Its break optional, as the last line's is.
        assert_eq!(
            parse(b"silico pane-widths 1\nbrowser 320\nproperties 300\nbrowser collapsed"),
            collapsed
        );
        // The width it was dragged to kept under it, clamped as any is.
        assert_eq!(
            parse(b"silico pane-widths 1\nbrowser 10\nproperties 300\nbrowser collapsed\n"),
            Kept {
                widths: Widths {
                    browser: 200.0,
                    properties: 300.0,
                },
                browser_collapsed: true,
            }
        );
        // Anything else after the widths, or after it, is no file of this form: the
        // defaults, shown.
        for bytes in [
            &b"silico pane-widths 1\nbrowser 320\nproperties 300\nbrowser shown\n"[..],
            b"silico pane-widths 1\nbrowser 320\nproperties 300\nbrowser collapsed \n",
            b"silico pane-widths 1\nbrowser 320\nproperties 300\nBrowser collapsed\n",
            b"silico pane-widths 1\nbrowser 320\nproperties 300\nbrowser collapsed\r\n",
            b"silico pane-widths 1\nbrowser 320\nproperties 300\nbrowser collapsed\n\n",
            b"silico pane-widths 1\nbrowser 320\nproperties 300\nbrowser collapsed\nbrowser collapsed\n",
            b"silico pane-widths 1\nbrowser collapsed\nbrowser 320\nproperties 300\n",
            b"silico pane-widths 1\nbrowser 320\nbrowser collapsed\nproperties 300\n",
        ] {
            assert_eq!(
                parse(bytes),
                Kept::default(),
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn a_width_out_of_range_is_clamped_into_it() {
        // By hand: the browser's least 200, the property panel's 232, the widest 8192.
        let read = |browser: &str, properties: &str| {
            parse(
                format!("silico pane-widths 1\nbrowser {browser}\nproperties {properties}\n")
                    .as_bytes(),
            )
            .widths
        };
        let widths = |browser, properties| Widths {
            browser,
            properties,
        };
        assert_eq!(read("0", "0"), widths(200.0, 232.0));
        assert_eq!(read("199", "231"), widths(200.0, 232.0));
        assert_eq!(read("200", "232"), widths(200.0, 232.0));
        assert_eq!(read("201", "233"), widths(201.0, 233.0));
        assert_eq!(read("8192", "8191"), widths(8192.0, 8191.0));
        assert_eq!(read("8193", "100000"), widths(8192.0, 8192.0));
        // Past u32's 4,294,967,295, and a thousand digits: the widest.
        assert_eq!(read("4294967296", "4294967295"), widths(8192.0, 8192.0));
        assert_eq!(read(&"9".repeat(1000), "300"), widths(8192.0, 300.0));
        // And clamped when written.
        assert_eq!(
            text(shown(widths(10.0, f32::NAN))),
            "silico pane-widths 1\nbrowser 200\nproperties 232\n"
        );
        assert_eq!(
            text(shown(widths(f32::INFINITY, -5.0))),
            "silico pane-widths 1\nbrowser 8192\nproperties 232\n"
        );
    }

    /// The thread's word polled until it's read.
    fn polled_until_read(widths: &mut KeptWidths) -> Kept {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            if let Some(read) = widths.poll() {
                assert!(widths.is_read());
                return read;
            }
            assert!(std::time::Instant::now() < deadline, "never told");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// `file`'s text once it reads `want`, within a minute.
    fn written(file: &Path, want: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::fs::read_to_string(file).ok().as_deref() != Some(want) {
            assert!(
                std::time::Instant::now() < deadline,
                "never written: {want}"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn the_thread_reads_the_file_once_and_writes_the_newest_widths_asked() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deep").join(FILE_NAME);
        // No file: the defaults, told once.
        let mut kept = KeptWidths::at(file.clone());
        assert_eq!(polled_until_read(&mut kept), Kept::default());
        assert_eq!(kept.poll(), None);
        assert!(!file.exists());
        // Asked twice: the file holds the newest.
        kept.save(shown(Widths {
            browser: 500.0,
            properties: 500.0,
        }));
        let wider = Widths {
            browser: 320.0,
            properties: 300.0,
        };
        kept.save(shown(wider));
        written(&file, "silico pane-widths 1\nbrowser 320\nproperties 300\n");
        // Another instance on the same file reads them.
        let mut again = KeptWidths::at(file.clone());
        assert_eq!(polled_until_read(&mut again), shown(wider));
        // No file but the widths' is left in its folder.
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE_NAME]);
    }

    #[test]
    fn the_thread_asks_the_window_for_a_frame_when_it_tells_the_widths() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let ctx = eframe::egui::Context::default();
        let asked = std::sync::Arc::new(AtomicU64::new(0));
        let counted = std::sync::Arc::clone(&asked);
        ctx.set_request_repaint_callback(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let dir = tempfile::tempdir().unwrap();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let read: Read = Box::new(move |_| {
            released.recv().ok()?;
            Some(b"silico pane-widths 1\nbrowser 300\nproperties 260\n".to_vec())
        });
        let mut kept = KeptWidths::reading(dir.path().join(FILE_NAME), read);
        kept.waking(&ctx);
        // Held: nothing told, and the window not asked.
        assert_eq!(kept.poll(), None);
        assert!(!kept.is_read());
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while asked.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline, "no frame asked for");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            polled_until_read(&mut kept),
            shown(Widths {
                browser: 300.0,
                properties: 260.0,
            })
        );
    }

    #[test]
    fn two_instances_on_one_file_keep_the_drag_that_ended_last() {
        // One instance writes 320 and 300, another 400 and 260, and the first 320 and 300
        // again: what the file was last written with by the first, written again, as the
        // last drag to end, never skipped as what it already wrote.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        let mut first = KeptWidths::at(file.clone());
        let mut second = KeptWidths::at(file.clone());
        polled_until_read(&mut first);
        polled_until_read(&mut second);
        let (one, other) = (
            Widths {
                browser: 320.0,
                properties: 300.0,
            },
            Widths {
                browser: 400.0,
                properties: 260.0,
            },
        );
        first.save(shown(one));
        written(&file, "silico pane-widths 1\nbrowser 320\nproperties 300\n");
        second.save(shown(other));
        written(&file, "silico pane-widths 1\nbrowser 400\nproperties 260\n");
        first.save(shown(one));
        written(&file, "silico pane-widths 1\nbrowser 320\nproperties 300\n");
    }

    #[test]
    fn widths_not_kept_are_read_at_once_as_the_defaults_and_written_nowhere() {
        let mut unkept = KeptWidths::unkept();
        assert!(unkept.is_read());
        assert!(!unkept.is_kept());
        assert_eq!(unkept.poll(), None);
        // Nothing to write to: asked, nothing happens.
        unkept.save(Kept::default());
        let dir = tempfile::tempdir().unwrap();
        assert!(KeptWidths::at(dir.path().join(FILE_NAME)).is_kept());
    }

    #[test]
    fn the_widths_are_kept_beside_the_recent_documents() {
        let recents = crate::recent::Recents::default_file();
        let widths = KeptWidths::default_file();
        assert_eq!(recents.is_some(), widths.is_some());
        if let (Some(recents), Some(widths)) = (recents, widths) {
            assert_eq!(recents.parent(), widths.parent());
            assert_eq!(widths.file_name().unwrap(), "pane-widths");
        }
    }
}
