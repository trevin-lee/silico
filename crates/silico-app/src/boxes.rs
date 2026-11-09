//! Each body's boxes for Select Other's cast (`crate::select_other`; M5.7,
//! docs/design/m5.0-app.md, section 9's *Selection*): its mesh's [`Pickable`], made off the
//! UI thread (invariant 4) on a thread of the tab's own, as the view takes each new mesh, as
//! the recent documents are kept (`crate::recent`): asked by the body and its mesh, told back,
//! and a frame asked for ([`crate::wake::Wake`]). Making them takes about 17 ms for ten copies of CTC 02
//! (the picking bench's `boxes_ms`), and 70 ms at a million triangles, against the cast's
//! 2 ms on the UI thread.
//!
//! They're kept by their body's number and its mesh (its `Arc`, the very one the view holds)
//! while the view holds that mesh for that body, through the tab hidden and its body hidden in
//! the browser, and let go the frame it doesn't. A mesh is asked for weakly, so nothing here
//! keeps it; and its ask is let go the frame the view lets go of it, so the thread, if it
//! hasn't begun its boxes, passes over it and makes the next, though the mesh lives on
//! elsewhere (the core's mesh cache keeps the very `Arc` the view had). Boxes already begun
//! are finished, and let go when they're told. A thread that can't be started, or that ends,
//! makes none again: what waits on it is told ([`Boxes::failed`]). Two tabs whose views hold
//! one mesh (one model open twice: the core gives its meshes to each session that reads them)
//! each make its boxes, on their own threads.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// A body's boxes asked of the thread: its mesh, weakly, and whether they're still wanted,
/// which the tab clears as the view lets go of the mesh, so the thread passes over it (that
/// flag, shared, is the ask's own: what's told back is matched to the ask by it).
#[derive(Clone)]
struct Ask {
    mesh: Weak<Mesh>,
    wanted: Arc<AtomicBool>,
}

impl Ask {
    /// `mesh`'s boxes, wanted.
    fn new(mesh: &Arc<Mesh>) -> Self {
        Ask {
            mesh: Arc::downgrade(mesh),
            wanted: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Not wanted any more: the thread, if it hasn't begun them, never will. (Relaxed: the
    /// flag orders nothing else, and the thread's read of it at worst makes boxes let go.)
    fn let_go(&self) {
        self.wanted.store(false, Ordering::Relaxed);
    }

    /// Whether they're still wanted.
    fn wanted(&self) -> bool {
        self.wanted.load(Ordering::Relaxed)
    }
}

/// What the thread tells back: a body's boxes, made for the ask whose flag `wanted` is.
struct Made {
    body: u32,
    wanted: Arc<AtomicBool>,
    pickable: Pickable,
}

/// The thread that makes them, once started.
enum Maker {
    /// None asked for yet.
    Idle,
    /// Asked by the body and its mesh, weakly; told back the boxes made.
    Running {
        asks: Sender<(u32, Ask)>,
        told: Receiver<Made>,
    },
    /// It couldn't be started, or it ended: no box is made again.
    Failed,
}

/// A tab's boxes, by body number: those made, each of the mesh the view holds for its body;
/// those asked for and not made yet, their meshes weakly; what makes them; and its thread.
pub(crate) struct Boxes {
    made: BTreeMap<u32, Pickable>,
    asked: BTreeMap<u32, Ask>,
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
            made: BTreeMap::new(),
            asked: BTreeMap::new(),
            build,
            maker: Maker::Idle,
            wake: Wake::default(),
        }
    }

    /// The view holds `meshes` (every body's, by its number, those hidden too) this frame:
    /// what the thread has made since taken, those of a body not held with the same mesh let
    /// go (and those asked for, so the thread passes over them), and each body neither made
    /// nor asked for asked for, `ctx` asked for a frame as each is made. Never waits; in
    /// `meshes`' count n and the kept's m, O((n + m) log n).
    pub(crate) fn hold<'a>(
        &mut self,
        meshes: impl Iterator<Item = (u32, &'a Arc<Mesh>)>,
        ctx: &egui::Context,
    ) {
        self.take_made();
        let held: BTreeMap<u32, &Arc<Mesh>> = meshes.collect();
        self.made.retain(|body, made| {
            held.get(body)
                .is_some_and(|mesh| Arc::ptr_eq(made.mesh(), mesh))
        });
        self.asked.retain(|body, ask| {
            let kept = held.get(body).is_some_and(|mesh| same(&ask.mesh, mesh));
            if !kept {
                ask.let_go();
            }
            kept
        });
        for (body, mesh) in held {
            if !self.made.contains_key(&body) && !self.asked.contains_key(&body) {
                self.ask(body, mesh, ctx);
            }
        }
    }

    /// The boxes the thread has told of, kept while they're still asked for (by the ask
    /// they were made for); the thread found ended, if it has.
    fn take_made(&mut self) {
        let Maker::Running { told, .. } = &self.maker else {
            return;
        };
        let mut ended = false;
        loop {
            match told.try_recv() {
                Ok(made) => {
                    let asked = self
                        .asked
                        .get(&made.body)
                        .is_some_and(|ask| Arc::ptr_eq(&ask.wanted, &made.wanted));
                    if asked {
                        self.asked.remove(&made.body);
                        self.made.insert(made.body, made.pickable);
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

    /// Body `body`'s boxes, of `mesh`, asked of the thread, started if it isn't.
    fn ask(&mut self, body: u32, mesh: &Arc<Mesh>, ctx: &egui::Context) {
        if matches!(self.maker, Maker::Idle) {
            self.start(ctx);
        }
        let Maker::Running { asks, .. } = &self.maker else {
            return;
        };
        let ask = Ask::new(mesh);
        if asks.send((body, ask.clone())).is_ok() {
            self.asked.insert(body, ask);
        } else {
            tracing::warn!("the thread making Select Other's boxes ended: no more are made");
            self.fail();
        }
    }

    /// The thread started, asking `ctx` for a frame as each box is made.
    fn start(&mut self, ctx: &egui::Context) {
        let (asks, asked) = mpsc::channel::<(u32, Ask)>();
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

    /// Body `body`'s boxes, if they're made of `mesh`.
    pub(crate) fn made_of(&self, body: u32, mesh: &Arc<Mesh>) -> Option<&Pickable> {
        self.made
            .get(&body)
            .filter(|made| Arc::ptr_eq(made.mesh(), mesh))
    }

    /// Whether no box can be made: its thread couldn't be started, or it ended.
    pub(crate) fn failed(&self) -> bool {
        matches!(self.maker, Maker::Failed)
    }

    /// The meshes whose boxes are made, by their bodies' numbers.
    pub(crate) fn meshes(&self) -> impl Iterator<Item = &Arc<Mesh>> {
        self.made.values().map(Pickable::mesh)
    }
}

/// The thread's work: each body asked for, if its boxes are still wanted and something still
/// holds its mesh, made into its boxes by `build` and told, and a frame asked for; until the
/// tab lets go of its end.
fn make_each(asked: &Receiver<(u32, Ask)>, tell: &Sender<Made>, build: &Build, wake: &Wake) {
    let _span = tracing::debug_span!("select_other_boxes").entered();
    while let Ok((body, ask)) = asked.recv() {
        // Let go before its turn came, by the view (though the core's cache may hold its mesh
        // still) or by everything: not made.
        if !ask.wanted() {
            continue;
        }
        let Some(mesh) = ask.mesh.upgrade() else {
            continue;
        };
        let made = Made {
            body,
            pickable: build(mesh),
            wanted: ask.wanted,
        };
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

    /// `boxes` holding `meshes`, each by its body's number, frame after frame until `done`.
    fn held_until(
        boxes: &mut Boxes,
        meshes: &[(u32, &Arc<Mesh>)],
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

    /// A build that writes down each mesh's least x, the first's (least x 0) held until
    /// `release` is sent.
    fn first_held() -> (Build, mpsc::Sender<()>, Arc<Mutex<Vec<f64>>>) {
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
        (build, release, made)
    }

    #[test]
    fn boxes_are_made_off_the_holding_thread_once_kept_while_held_and_let_go_after() {
        let (build, made) = recorded();
        let mut boxes = Boxes::new(build);
        let ctx = egui::Context::default();
        let (a, b) = (mesh(0.0), mesh(5.0));
        // Nothing made at the hold that asks for them: they're made on a thread of their own.
        boxes.hold([(1, &a), (2, &b)].into_iter(), &ctx);
        held_until(&mut boxes, &[(1, &a), (2, &b)], &ctx, |boxes| {
            boxes.made_of(1, &a).is_some() && boxes.made_of(2, &b).is_some()
        });
        let here = std::thread::current().id();
        let on = made.lock().unwrap().clone();
        assert_eq!(on.len(), 2, "{on:?}");
        assert!(on.iter().all(|(thread, _)| *thread != here), "{on:?}");
        // Held again and again: kept, each its own body's mesh's, none made again; and only
        // of the mesh and the body it was made for.
        for _ in 0..5 {
            boxes.hold([(1, &a), (2, &b)].into_iter(), &ctx);
        }
        assert_eq!(made.lock().unwrap().len(), 2);
        assert!(Arc::ptr_eq(boxes.made_of(1, &a).unwrap().mesh(), &a));
        assert!(Arc::ptr_eq(boxes.made_of(2, &b).unwrap().mesh(), &b));
        assert!(boxes.made_of(1, &b).is_none() && boxes.made_of(2, &a).is_none());
        // `a` held no longer: its boxes let go, and nothing here holds it; `b`'s kept.
        boxes.hold([(2, &b)].into_iter(), &ctx);
        assert!(boxes.made_of(1, &a).is_none());
        assert_eq!(Arc::strong_count(&a), 1);
        assert!(boxes.made_of(2, &b).is_some());
        assert_eq!(boxes.meshes().count(), 1);
        // Another mesh of the same points is another mesh: made, once.
        let again = mesh(0.0);
        held_until(&mut boxes, &[(2, &b), (1, &again)], &ctx, |boxes| {
            boxes.made_of(1, &again).is_some()
        });
        assert_eq!(made.lock().unwrap().len(), 3);
        // `b`'s body given another mesh: `b`'s boxes let go, the new one's made.
        let other = mesh(7.0);
        held_until(&mut boxes, &[(2, &other), (1, &again)], &ctx, |boxes| {
            boxes.made_of(2, &other).is_some()
        });
        assert!(boxes.made_of(2, &b).is_none());
        assert_eq!(Arc::strong_count(&b), 1);
        let kept: Vec<f64> = boxes.meshes().map(|mesh| least_x(mesh)).collect();
        assert_eq!(kept, [0.0, 7.0]);
        assert_eq!(made.lock().unwrap().len(), 4);
        assert!(!boxes.failed());
    }

    #[test]
    fn a_mesh_let_go_before_its_turn_is_never_made() {
        // The first mesh's making held until released; the second, asked behind it, let go
        // meanwhile (nothing but the boxes' handle on it, which is weak): never made.
        let (build, release, made) = first_held();
        let mut boxes = Boxes::new(build);
        let ctx = egui::Context::default();
        let (a, b) = (mesh(0.0), mesh(5.0));
        boxes.hold([(1, &a), (2, &b)].into_iter(), &ctx);
        boxes.hold([(1, &a)].into_iter(), &ctx);
        drop(b);
        release.send(()).unwrap();
        held_until(&mut boxes, &[(1, &a)], &ctx, |boxes| {
            boxes.made_of(1, &a).is_some()
        });
        // A third asked after: made once the thread has passed the second's turn.
        let c = mesh(9.0);
        held_until(&mut boxes, &[(1, &a), (3, &c)], &ctx, |boxes| {
            boxes.made_of(3, &c).is_some()
        });
        assert_eq!(*made.lock().unwrap(), [0.0, 9.0]);
    }

    #[test]
    fn a_mesh_the_view_lets_go_before_its_turn_is_never_made_though_another_holds_it() {
        // As the last, but the second mesh lives on (the core's mesh cache keeps the very
        // `Arc` the view had), its body given a new mesh: the second is passed over, the
        // thread making the new one next, so nothing waits behind boxes no one wants.
        let (build, release, made) = first_held();
        let mut boxes = Boxes::new(build);
        let ctx = egui::Context::default();
        let (a, b) = (mesh(0.0), mesh(5.0));
        let cached = Arc::clone(&b);
        boxes.hold([(1, &a), (2, &b)].into_iter(), &ctx);
        let new = mesh(7.0);
        boxes.hold([(1, &a), (2, &new)].into_iter(), &ctx);
        drop(b);
        release.send(()).unwrap();
        held_until(&mut boxes, &[(1, &a), (2, &new)], &ctx, |boxes| {
            boxes.made_of(1, &a).is_some() && boxes.made_of(2, &new).is_some()
        });
        assert_eq!(*made.lock().unwrap(), [0.0, 7.0]);
        assert!(boxes.made_of(2, &cached).is_none());
        // Let go of in a body, and asked again before its turn: made once, as asked again.
        let (build, release, made) = first_held();
        let mut boxes = Boxes::new(build);
        let d = mesh(3.0);
        boxes.hold([(1, &a), (2, &d)].into_iter(), &ctx);
        boxes.hold([(1, &a)].into_iter(), &ctx);
        boxes.hold([(1, &a), (2, &d)].into_iter(), &ctx);
        release.send(()).unwrap();
        held_until(&mut boxes, &[(1, &a), (2, &d)], &ctx, |boxes| {
            boxes.made_of(2, &d).is_some()
        });
        // A third behind them: once it's made, the thread has passed every ask before it.
        let c = mesh(9.0);
        held_until(&mut boxes, &[(1, &a), (2, &d), (3, &c)], &ctx, |boxes| {
            boxes.made_of(3, &c).is_some()
        });
        assert_eq!(*made.lock().unwrap(), [0.0, 3.0, 9.0]);
    }

    #[test]
    fn many_bodies_are_each_made_once_and_let_go_body_by_body() {
        // A thousand bodies, each its own mesh: each made once; then every other one given
        // a new mesh and every tenth gone: the rest kept, never made again, and the new made.
        let (build, made) = recorded();
        let mut boxes = Boxes::new(build);
        let ctx = egui::Context::default();
        let first: Vec<Arc<Mesh>> = (0..1000).map(|n| mesh(f64::from(n) + 1.0)).collect();
        let bodies = |meshes: &[Arc<Mesh>]| -> Vec<(u32, Arc<Mesh>)> {
            (1..).zip(meshes.iter().cloned()).collect()
        };
        let held = bodies(&first);
        let refs: Vec<(u32, &Arc<Mesh>)> = held.iter().map(|(n, mesh)| (*n, mesh)).collect();
        held_until(&mut boxes, &refs, &ctx, |boxes| {
            boxes.meshes().count() == 1000
        });
        assert_eq!(made.lock().unwrap().len(), 1000);
        let second: Vec<(u32, Arc<Mesh>)> = held
            .iter()
            .filter(|(n, _)| n % 10 != 0)
            .map(|(n, old)| {
                let mesh = if n % 2 == 0 {
                    mesh(least_x(old) + 2000.0)
                } else {
                    Arc::clone(old)
                };
                (*n, mesh)
            })
            .collect();
        let refs: Vec<(u32, &Arc<Mesh>)> = second.iter().map(|(n, mesh)| (*n, mesh)).collect();
        held_until(&mut boxes, &refs, &ctx, |boxes| {
            second
                .iter()
                .all(|(n, mesh)| boxes.made_of(*n, mesh).is_some())
        });
        // 1000 at first; then the 400 of new meshes (the even bodies not a tenth's).
        assert_eq!(made.lock().unwrap().len(), 1400);
        assert_eq!(boxes.meshes().count(), 900);
        for (n, old) in &held {
            let kept = boxes.made_of(*n, old).is_some();
            assert_eq!(kept, n % 2 == 1, "body {n}");
        }
    }

    #[test]
    fn a_thread_that_ends_is_told_so_nothing_waits_on_it_and_none_is_asked_again() {
        let build: Build = Arc::new(|_| panic!("no boxes"));
        let mut boxes = Boxes::new(build);
        let ctx = egui::Context::default();
        let a = mesh(0.0);
        held_until(&mut boxes, &[(1, &a)], &ctx, Boxes::failed);
        assert!(boxes.made_of(1, &a).is_none());
        assert!(boxes.asked.is_empty());
        boxes.hold([(1, &a)].into_iter(), &ctx);
        assert!(boxes.asked.is_empty() && boxes.failed());
    }
}
