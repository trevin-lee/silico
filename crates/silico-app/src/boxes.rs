//! Each body's boxes for Select Other's cast (`crate::select_other`; M5.7,
//! docs/design/m5.0-app.md, section 9's *Selection*): its mesh's [`Pickable`], made off the
//! UI thread (invariant 4) on a thread of the tab's own, as the view takes each new mesh, as
//! the recent documents are kept (`crate::recent`): asked by the mesh, told back, and a frame
//! asked for ([`crate::wake::Wake`]). Making them takes about 17 ms for ten copies of CTC 02
//! (the picking bench's `boxes_ms`), and 70 ms at a million triangles, against the cast's
//! 2 ms on the UI thread.
//!
//! They're kept by their mesh (its `Arc`, the very one the view holds) while the view holds
//! it, through the tab hidden and its body hidden in the browser, and let go the frame the
//! view no longer holds that mesh. A mesh the view lets go before its boxes are made isn't
//! made (it's asked for weakly, so nothing here keeps it). A thread that can't be started, or
//! that ends, makes none again: what waits on it is told ([`Boxes::failed`]). Two tabs whose
//! views hold one mesh (one model open twice: the core gives its meshes to each session
//! that reads them) each make its boxes, on their own threads.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Weak};

use eframe::egui;
use silico_protocol::Mesh;
use silico_render::render::Pickable;

use crate::wake::Wake;

/// What makes a mesh's boxes: [`Pickable::new`], or a test's, to see where it runs and to
/// hold it.
pub type Build = Arc<dyn Fn(Arc<Mesh>) -> Pickable + Send + Sync>;

/// The boxes made the way the cast takes them: [`Pickable::new`].
pub fn pickable() -> Build {
    Arc::new(Pickable::new)
}

/// The thread that makes them, once started.
enum Maker {
    /// None asked for yet.
    Idle,
    /// Asked by the mesh, weakly; told back the boxes made.
    Running {
        asks: Sender<Weak<Mesh>>,
        told: Receiver<Pickable>,
    },
    /// It couldn't be started, or it ended: no box is made again.
    Failed,
}

/// A tab's boxes: those made, each of a mesh the view holds; the meshes asked for and not
/// made yet, weakly; what makes them; and its thread.
pub(crate) struct Boxes {
    made: Vec<Pickable>,
    asked: Vec<Weak<Mesh>>,
    build: Build,
    maker: Maker,
    wake: Wake,
}

impl Default for Boxes {
    fn default() -> Self {
        Boxes::new(pickable())
    }
}

impl std::fmt::Debug for Boxes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Boxes")
            .field("made", &self.made.len())
            .field("asked", &self.asked.len())
            .field("failed", &self.failed())
            .finish_non_exhaustive()
    }
}

/// Whether `weak` is a handle on `mesh`: the same allocation, which `weak` keeps from being
/// another mesh's while it's held.
fn same(weak: &Weak<Mesh>, mesh: &Arc<Mesh>) -> bool {
    std::ptr::eq(weak.as_ptr(), Arc::as_ptr(mesh))
}

impl Boxes {
    /// Boxes made by `build`, its thread started when the first is asked for.
    pub(crate) fn new(build: Build) -> Self {
        Boxes {
            made: Vec::new(),
            asked: Vec::new(),
            build,
            maker: Maker::Idle,
            wake: Wake::default(),
        }
    }

    /// The view holds `meshes` (every body's, those hidden too) this frame: what the thread
    /// has made since taken, those of a mesh not among them let go (and those asked for), and
    /// each of them neither made nor asked for asked for, `ctx` asked for a frame as each is
    /// made. Never waits.
    pub(crate) fn hold<'a>(
        &mut self,
        meshes: impl Iterator<Item = &'a Arc<Mesh>> + Clone,
        ctx: &egui::Context,
    ) {
        self.take_made();
        let held = |mesh: &Arc<Mesh>| meshes.clone().any(|kept| Arc::ptr_eq(kept, mesh));
        self.made.retain(|pickable| held(pickable.mesh()));
        self.asked
            .retain(|weak| meshes.clone().any(|kept| same(weak, kept)));
        for mesh in meshes {
            let known = self.made.iter().any(|made| Arc::ptr_eq(made.mesh(), mesh))
                || self.asked.iter().any(|weak| same(weak, mesh));
            if !known {
                self.ask(mesh, ctx);
            }
        }
    }

    /// The boxes the thread has told of, kept while they're still asked for; the thread
    /// found ended, if it has.
    fn take_made(&mut self) {
        let Maker::Running { told, .. } = &self.maker else {
            return;
        };
        let mut ended = false;
        loop {
            match told.try_recv() {
                Ok(pickable) => {
                    let at = self
                        .asked
                        .iter()
                        .position(|weak| same(weak, pickable.mesh()));
                    if let Some(at) = at {
                        self.asked.swap_remove(at);
                        self.made.push(pickable);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    ended = true;
                    break;
                }
            }
        }
        if ended {
            tracing::warn!("the thread making Select Other's boxes ended: no more are made");
            self.fail();
        }
    }

    /// `mesh`'s boxes asked of the thread, started if it isn't.
    fn ask(&mut self, mesh: &Arc<Mesh>, ctx: &egui::Context) {
        if matches!(self.maker, Maker::Idle) {
            self.start(ctx);
        }
        let Maker::Running { asks, .. } = &self.maker else {
            return;
        };
        if asks.send(Arc::downgrade(mesh)).is_ok() {
            self.asked.push(Arc::downgrade(mesh));
        } else {
            tracing::warn!("the thread making Select Other's boxes ended: no more are made");
            self.fail();
        }
    }

    /// The thread started, asking `ctx` for a frame as each box is made.
    fn start(&mut self, ctx: &egui::Context) {
        let (asks, asked) = mpsc::channel::<Weak<Mesh>>();
        let (tell, told) = mpsc::channel();
        self.wake.set(ctx);
        let wake = self.wake.clone();
        let build = Arc::clone(&self.build);
        let started = std::thread::Builder::new()
            .name("silico-select-other-boxes".to_owned())
            .spawn(move || make_each(&asked, &tell, &build, &wake));
        self.maker = match started {
            Ok(_) => Maker::Running { asks, told },
            Err(e) => {
                tracing::warn!("Select Other's boxes can't be made: {e}");
                Maker::Failed
            }
        };
    }

    /// No box made again: those asked for never will be.
    fn fail(&mut self) {
        self.maker = Maker::Failed;
        self.asked.clear();
    }

    /// `mesh`'s boxes, if they're made.
    pub(crate) fn made_of(&self, mesh: &Arc<Mesh>) -> Option<&Pickable> {
        self.made.iter().find(|made| Arc::ptr_eq(made.mesh(), mesh))
    }

    /// Whether no box can be made: its thread couldn't be started, or it ended.
    pub(crate) fn failed(&self) -> bool {
        matches!(self.maker, Maker::Failed)
    }

    /// The meshes whose boxes are made, in the order they were.
    pub(crate) fn meshes(&self) -> impl Iterator<Item = &Arc<Mesh>> {
        self.made.iter().map(Pickable::mesh)
    }
}

/// The thread's work: each mesh asked for, if something still holds it, made into its boxes
/// by `build` and told, and a frame asked for; until the tab lets go of its end.
fn make_each(asked: &Receiver<Weak<Mesh>>, tell: &Sender<Pickable>, build: &Build, wake: &Wake) {
    let _span = tracing::debug_span!("select_other_boxes").entered();
    while let Ok(mesh) = asked.recv() {
        // Let go before its turn came: not made.
        let Some(mesh) = mesh.upgrade() else {
            continue;
        };
        let made = build(mesh);
        if tell.send(made).is_err() {
            return;
        }
        wake.wake();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::thread::ThreadId;
    use std::time::{Duration, Instant};

    use super::*;

    const WAIT: Duration = Duration::from_secs(60);

    /// A box mesh whose least x is `at`: what tells the meshes apart in a build's record.
    fn mesh(at: f64) -> Arc<Mesh> {
        Arc::new(silico_render::bench::box_mesh([at, 0.0, 0.0], [1.0; 3]))
    }

    /// `mesh`'s least x.
    fn least_x(mesh: &Mesh) -> f64 {
        mesh.positions
            .iter()
            .map(|p| p[0])
            .fold(f64::INFINITY, f64::min)
    }

    /// `boxes` holding `meshes` frame after frame until `done`.
    fn held_until(
        boxes: &mut Boxes,
        meshes: &[&Arc<Mesh>],
        ctx: &egui::Context,
        done: impl Fn(&Boxes) -> bool,
    ) {
        let deadline = Instant::now() + WAIT;
        loop {
            boxes.hold(meshes.iter().copied(), ctx);
            if done(boxes) {
                return;
            }
            assert!(Instant::now() < deadline, "never done: {boxes:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// What a [`recorded`] build writes down: the thread each ran on, and its mesh's least x.
    type Record = Arc<Mutex<Vec<(ThreadId, f64)>>>;

    /// A build that writes down the thread it ran on and the mesh's least x.
    fn recorded() -> (Build, Record) {
        let made = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&made);
        let build: Build = Arc::new(move |mesh: Arc<Mesh>| {
            seen.lock()
                .unwrap()
                .push((std::thread::current().id(), least_x(&mesh)));
            Pickable::new(mesh)
        });
        (build, made)
    }

    #[test]
    fn boxes_are_made_off_the_holding_thread_once_kept_while_held_and_let_go_after() {
        let (build, made) = recorded();
        let mut boxes = Boxes::new(build);
        let ctx = egui::Context::default();
        let (a, b) = (mesh(0.0), mesh(5.0));
        // Nothing made at the hold that asks for them: they're made on a thread of their own.
        boxes.hold([&a, &b].into_iter(), &ctx);
        held_until(&mut boxes, &[&a, &b], &ctx, |boxes| {
            boxes.made_of(&a).is_some() && boxes.made_of(&b).is_some()
        });
        let here = std::thread::current().id();
        let on = made.lock().unwrap().clone();
        assert_eq!(on.len(), 2, "{on:?}");
        assert!(on.iter().all(|(thread, _)| *thread != here), "{on:?}");
        // Held again and again: kept, each its own mesh's, none made again.
        for _ in 0..5 {
            boxes.hold([&a, &b].into_iter(), &ctx);
        }
        assert_eq!(made.lock().unwrap().len(), 2);
        assert!(Arc::ptr_eq(boxes.made_of(&a).unwrap().mesh(), &a));
        assert!(Arc::ptr_eq(boxes.made_of(&b).unwrap().mesh(), &b));
        // `a` held no longer: its boxes let go, and nothing here holds it; `b`'s kept.
        boxes.hold([&b].into_iter(), &ctx);
        assert!(boxes.made_of(&a).is_none());
        assert_eq!(Arc::strong_count(&a), 1);
        assert!(boxes.made_of(&b).is_some());
        assert_eq!(boxes.meshes().count(), 1);
        // Another mesh of the same points is another mesh: made, once.
        let again = mesh(0.0);
        held_until(&mut boxes, &[&b, &again], &ctx, |boxes| {
            boxes.made_of(&again).is_some()
        });
        assert_eq!(made.lock().unwrap().len(), 3);
        assert!(!boxes.failed());
    }

    #[test]
    fn a_mesh_let_go_before_its_turn_is_never_made() {
        // The first mesh's making held until released; the second, asked behind it, let go
        // meanwhile (nothing but the boxes' handle on it, which is weak): never made.
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let made = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&made);
        let build: Build = Arc::new(move |mesh: Arc<Mesh>| {
            let at = least_x(&mesh);
            if at == 0.0 {
                released.lock().unwrap().recv_timeout(WAIT).unwrap();
            }
            seen.lock().unwrap().push(at);
            Pickable::new(mesh)
        });
        let mut boxes = Boxes::new(build);
        let ctx = egui::Context::default();
        let (a, b) = (mesh(0.0), mesh(5.0));
        boxes.hold([&a, &b].into_iter(), &ctx);
        boxes.hold([&a].into_iter(), &ctx);
        drop(b);
        release.send(()).unwrap();
        held_until(&mut boxes, &[&a], &ctx, |boxes| boxes.made_of(&a).is_some());
        // A third asked after: made once the thread has passed the second's turn.
        let c = mesh(9.0);
        held_until(&mut boxes, &[&a, &c], &ctx, |boxes| {
            boxes.made_of(&c).is_some()
        });
        assert_eq!(*made.lock().unwrap(), [0.0, 9.0]);
    }

    #[test]
    fn a_thread_that_ends_is_told_so_nothing_waits_on_it_and_none_is_asked_again() {
        let build: Build = Arc::new(|_| panic!("no boxes"));
        let mut boxes = Boxes::new(build);
        let ctx = egui::Context::default();
        let a = mesh(0.0);
        held_until(&mut boxes, &[&a], &ctx, Boxes::failed);
        assert!(boxes.made_of(&a).is_none());
        assert!(boxes.asked.is_empty());
        boxes.hold([&a].into_iter(), &ctx);
        assert!(boxes.asked.is_empty() && boxes.failed());
    }
}
