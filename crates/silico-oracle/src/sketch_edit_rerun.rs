//! The sketch sweeps' reruns (#541): FreeCAD 1.1.3 doesn't answer every case the same run to
//! run (its unknowns are ordered by memory address, docs/ports/sketcher.md:51, so its solves
//! and drags land by the process's state), and a generated case it varies on is no oracle for
//! what varies. So a sweep's case that differs is run through FreeCAD [`RERUNS`] times more,
//! each alone in a FreeCAD of its own, and judged again against every run
//! ([`rejudge`]). Where FreeCAD's runs agree (their numbers within [`REPRODUCED`] of the first
//! run's), the case is compared as it was. Where they disagree, a difference is still reported
//! wherever every run reproduces it: only the differences some run of FreeCAD doesn't give
//! are left uncompared, and they're listed with what of FreeCAD's answer varied. A difference
//! of Silico's in what FreeCAD reproduced is never left out.
//!
//! The hand cases and the gate's tests don't rerun: [`run_rerunning`] is the sweeps' alone.

use std::collections::BTreeMap;

use super::{
    DRAG_BAND, EditCase, FcGeometry, FreecadEdited, Plan, Prepared, SilicoSide, Theirs, Verdict,
    both_sides, freecad_outcomes, judged, merge_refusal, rule, silico_refused, verdict,
};
use crate::sketch::Details;

/// How many more times FreeCAD runs a sweep's case that differs, each alone in a FreeCAD of
/// its own: with the sweep's run, three answers.
pub const RERUNS: usize = 2;

/// The most cases of a sweep that are run again, in the sweep's order: past it, a case that
/// differs is reported as it is. A night's sweep has had at most a few differing cases; this
/// bounds a broken build's, which differs everywhere, to about a hundred more FreeCAD runs.
pub const MOST_RERUN: usize = 50;

/// How far apart two of FreeCAD's runs may put a number (a place, a radius, a constraint's
/// value) and still give the same answer: the drag's band, the loosest the comparison holds
/// FreeCAD's places to, so a run's noise inside any of the comparison's tolerances is never
/// taken for an answer that varies (which would leave Silico's difference there uncompared).
pub const REPRODUCED: f64 = DRAG_BAND;

/// A rerun case: its name, how many runs of FreeCAD it had, and what they gave.
#[derive(Debug, Clone, PartialEq)]
pub struct Rerun {
    pub case: String,
    pub runs: usize,
    pub reran: Reran,
}

/// What a differing case's runs of FreeCAD gave.
#[derive(Debug, Clone, PartialEq)]
pub enum Reran {
    /// The same answer every run: the case is compared in full.
    Alike,
    /// Answers that differ: what of FreeCAD's answer varied, and each difference left
    /// uncompared for it, with the runs (from 1, the sweep's) that gave it.
    Varied {
        varied: Vec<String>,
        uncompared: Vec<String>,
    },
    /// Not judged by its reruns, so compared as the sweep's run has it: why.
    Unjudged(String),
}

impl std::fmt::Display for Rerun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Rerun { case, runs, reran } = self;
        match reran {
            Reran::Alike => write!(
                f,
                "rerun: {case}: FreeCAD gave the same answer in all {runs} runs, so the case is compared in full"
            ),
            Reran::Varied { varied, uncompared } if uncompared.is_empty() => write!(
                f,
                "varies: {case}: FreeCAD's {runs} runs differ in {}, but every difference is one each run gives, so the case is compared in full",
                varied.join("; ")
            ),
            Reran::Varied { varied, uncompared } => write!(
                f,
                "varies: {case}: FreeCAD's {runs} runs differ in {}, so what some run doesn't give isn't compared: {}",
                varied.join("; "),
                uncompared.join("; ")
            ),
            Reran::Unjudged(why) => write!(f, "rerun: {case}: {why}"),
        }
    }
}

/// Runs `cases` as [`super::run`] does, then each that differs (up to [`MOST_RERUN`] of them,
/// in order) through FreeCAD [`RERUNS`] times more, each alone in a FreeCAD of its own, and
/// gives each case's verdict as [`rejudge`] gives it, in order, and what each rerun case's
/// runs gave.
pub fn run_rerunning(cases: &[EditCase]) -> Result<(Vec<Verdict>, Vec<Rerun>), String> {
    let (prepared, theirs) = both_sides(cases)?;
    let mut verdicts = Vec::with_capacity(prepared.len());
    let mut reruns = Vec::new();
    for (p, first) in prepared.iter().zip(theirs) {
        let v = verdict(p, first.as_ref());
        if v.how.is_empty() || !matches!(p.plan, Plan::Run { .. }) {
            verdicts.push(v);
            continue;
        }
        if reruns.len() == MOST_RERUN {
            reruns.push(Rerun {
                case: p.name.clone(),
                runs: 1,
                reran: Reran::Unjudged(format!(
                    "not run again: the sweep runs at most {MOST_RERUN} of its differing cases again"
                )),
            });
            verdicts.push(v);
            continue;
        }
        let _span = tracing::info_span!("sketch_edit_rerun", case = %p.name).entered();
        let mut runs = vec![first];
        for _ in 0..RERUNS {
            runs.extend(freecad_outcomes(std::slice::from_ref(p))?);
        }
        let (v, reran) = rejudge(p, &runs);
        reruns.push(Rerun {
            case: p.name.clone(),
            runs: runs.len(),
            reran,
        });
        verdicts.push(v);
    }
    Ok((verdicts, reruns))
}

/// `p`'s verdict against FreeCAD's `runs` of it (the sweep's first), and what they gave. Each
/// run after the first is taken as the first's wherever its numbers are within [`REPRODUCED`]
/// of it ([`snapped`]). Runs that agree: the first run's verdict, [`Reran::Alike`]. Runs that
/// don't: a difference is kept where each run compared gives it, read without what FreeCAD
/// says beside what's compared ([`Details::Keyed`]), as often as the run giving it least
/// often does; the runs compared are those where FreeCAD made the edit as Silico did, if two
/// or more did (its answer there reproduced among them, whatever it made of the case in
/// another run), and every run otherwise. The verdict is the first compared run's, with those
/// differences. A refusal of Silico's by no rule of the note's is a difference whatever
/// FreeCAD answers, so it's kept whole; and a run the harness failed on, or FreeCAD wasn't
/// run for, leaves the first run's verdict as it is ([`Reran::Unjudged`]).
pub fn rejudge(p: &Prepared, runs: &[Option<Theirs>]) -> (Verdict, Reran) {
    let Some((Some(first), rest)) = runs.split_first() else {
        return (
            verdict(p, None),
            Reran::Unjudged("FreeCAD wasn't run on it".to_owned()),
        );
    };
    let unjudged = |why: String| (verdict(p, Some(first)), Reran::Unjudged(why));
    let mut snapped = vec![first.clone()];
    for (n, run) in rest.iter().enumerate() {
        match run {
            None => return unjudged(format!("FreeCAD's run {} gave nothing", n + 2)),
            Some(Theirs::Harness(why)) => {
                return unjudged(format!(
                    "the harness's FreeCAD side failed in run {}, not FreeCAD's edit, so its runs aren't compared: {why}",
                    n + 2
                ));
            }
            Some(run) => snapped.push(self::snapped(first, run)),
        }
    }
    if let Theirs::Harness(why) = first {
        return unjudged(format!(
            "the harness's FreeCAD side failed in run 1, not FreeCAD's edit, so its runs aren't compared: {why}"
        ));
    }
    let varied = varied(&snapped);
    if varied.is_empty() {
        return (verdict(p, Some(first)), Reran::Alike);
    }
    // Silico's refusal by no rule of the note's: a difference whatever FreeCAD does.
    if silico_refused(p).is_some() && rule(p).is_none() {
        let reran = Reran::Varied {
            varied,
            uncompared: Vec::new(),
        };
        return (verdict(p, Some(first)), reran);
    }
    let full: Vec<Verdict> = snapped
        .iter()
        .map(|t| judged(p, Some(t), Details::Full))
        .collect();
    let keyed: Vec<Verdict> = snapped
        .iter()
        .map(|t| judged(p, Some(t), Details::Keyed))
        .collect();
    if full
        .iter()
        .zip(&keyed)
        .any(|(f, k)| f.how.len() != k.how.len())
    {
        return unjudged(
            "the harness's own fault: a run's differences read otherwise without FreeCAD's details"
                .to_owned(),
        );
    }
    let made: Vec<usize> = (0..snapped.len())
        .filter(|&i| made_as_silico(p, &snapped[i]))
        .collect();
    let compared: Vec<usize> = if made.len() >= 2 {
        made
    } else {
        (0..snapped.len()).collect()
    };
    let count = |v: &Verdict| {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for line in &v.how {
            *counts.entry(line.clone()).or_default() += 1;
        }
        counts
    };
    // Each keyed difference, as often as the compared run giving it least often gives it.
    let mut kept = compared
        .first()
        .and_then(|&i| keyed.get(i))
        .map(count)
        .unwrap_or_default();
    for &i in compared.iter().skip(1) {
        let Some(counts) = keyed.get(i).map(count) else {
            continue;
        };
        for (line, n) in &mut kept {
            *n = (*n).min(counts.get(line).copied().unwrap_or(0));
        }
    }
    let base = compared.first().copied().unwrap_or(0);
    let (Some(based), Some(base_keys)) = (full.get(base), keyed.get(base)) else {
        return unjudged("the harness's own fault: no run to compare".to_owned());
    };
    let mut v = based.clone();
    let mut left = kept.clone();
    v.how = based
        .how
        .iter()
        .zip(&base_keys.how)
        .filter(|(_, key)| take(&mut left, key))
        .map(|(line, _)| line.clone())
        .collect();
    // What each run gave that isn't kept, with the runs that gave it, in the runs' order.
    let mut uncompared: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, (f, k)) in full.iter().zip(&keyed).enumerate() {
        let mut left = kept.clone();
        for (line, key) in f.how.iter().zip(&k.how) {
            if compared.contains(&i) && take(&mut left, key) {
                continue;
            }
            match uncompared.iter_mut().find(|(l, _)| l == line) {
                Some((_, runs)) if !runs.contains(&(i + 1)) => runs.push(i + 1),
                Some(_) => {}
                None => uncompared.push((line.clone(), vec![i + 1])),
            }
        }
    }
    let uncompared = uncompared
        .into_iter()
        .map(|(line, runs)| {
            let runs: Vec<String> = runs.iter().map(ToString::to_string).collect();
            format!("{line} (run {})", runs.join(", "))
        })
        .collect();
    (v, Reran::Varied { varied, uncompared })
}

/// Takes one of `key` from `left`, if there's one left.
fn take(left: &mut BTreeMap<String, usize>, key: &str) -> bool {
    match left.get_mut(key) {
        Some(n) if *n > 0 => {
            *n -= 1;
            true
        }
        _ => false,
    }
}

/// Whether FreeCAD's run `t` made `p`'s edit as Silico did: both edited, neither refusing
/// (a merge's Coincident command not rolling FreeCAD's back).
fn made_as_silico(p: &Prepared, t: &Theirs) -> bool {
    matches!(p.silico, SilicoSide::Edited(_))
        && silico_refused(p).is_none()
        && matches!(t, Theirs::Edited(fc) if merge_refusal(p, fc).is_none())
}

/// `run` taken as `first` wherever it gives the same answer: each place, radius, and
/// constraint value of its geometry and constraints matched with `first`'s (the same index,
/// ID, and kind; the same constraint kind and references) and within [`REPRODUCED`] of it is
/// `first`'s, exactly; and a crash or hang ended the same way (its first line) is `first`'s,
/// what FreeCAD last wrote before it aside. Anything else is `run`'s own.
pub fn snapped(first: &Theirs, run: &Theirs) -> Theirs {
    match (first, run) {
        (Theirs::Lost(a), Theirs::Lost(b)) if a.lines().next() == b.lines().next() => first.clone(),
        (Theirs::Edited(a), Theirs::Edited(b)) => {
            let mut out: FreecadEdited = (**b).clone();
            snap_geometry(&a.edited, &mut out.edited);
            snap_geometry(&a.geometry, &mut out.geometry);
            if a.constraints.len() == out.constraints.len() {
                for (want, got) in a.constraints.iter().zip(&mut out.constraints) {
                    if want.kind == got.kind && want.refs == got.refs {
                        snap(want.value, &mut got.value);
                    }
                }
            }
            if let (Some(want), Some(got)) = (&a.placed, &mut out.placed) {
                snap_geometry(&want.geometry, &mut got.geometry);
            }
            Theirs::Edited(Box::new(out))
        }
        _ => run.clone(),
    }
}

fn snap_geometry(first: &[FcGeometry], run: &mut [FcGeometry]) {
    if first.len() != run.len() {
        return;
    }
    for (want, got) in first.iter().zip(run) {
        if want.id != got.id || want.kind != got.kind {
            continue;
        }
        for (pos, at) in &mut got.points {
            if let Some(w) = want.points.get(pos) {
                snap(w[0], &mut at[0]);
                snap(w[1], &mut at[1]);
            }
        }
        if let (Some(w), Some(r)) = (want.radius, got.radius.as_mut()) {
            snap(w, r);
        }
    }
}

fn snap(want: f64, got: &mut f64) {
    if (want - *got).abs() <= REPRODUCED {
        *got = want;
    }
}

/// What of FreeCAD's answer differs between `runs` (each after the first [`snapped`] to it),
/// for a reader: empty where they're the same.
pub fn varied(runs: &[Theirs]) -> Vec<String> {
    let Some((first, rest)) = runs.split_first() else {
        return Vec::new();
    };
    if rest.iter().all(|r| r == first) {
        return Vec::new();
    }
    let outcome = |t: &Theirs| match t {
        Theirs::Edited(_) => "made the edit".to_owned(),
        Theirs::Failed(why) => format!("raised \"{why}\""),
        Theirs::Lost(why) => format!(
            "crashed or hung ({})",
            why.lines().next().unwrap_or_default()
        ),
        Theirs::Harness(_) => "failed in the harness".to_owned(),
    };
    let edited: Vec<&FreecadEdited> = runs
        .iter()
        .filter_map(|t| match t {
            Theirs::Edited(fc) => Some(&**fc),
            _ => None,
        })
        .collect();
    if edited.len() != runs.len() {
        let each: Vec<String> = runs
            .iter()
            .enumerate()
            .map(|(i, t)| format!("run {} {}", i + 1, outcome(t)))
            .collect();
        return vec![format!("its outcome ({})", each.join(", "))];
    }
    let Some(&one) = edited.first() else {
        return Vec::new();
    };
    let mut varied = Vec::new();
    let differs =
        |part: &dyn Fn(&FreecadEdited) -> String| edited.iter().any(|fc| part(fc) != part(one));
    let each = |part: &dyn Fn(&FreecadEdited) -> String| {
        let each: Vec<String> = edited
            .iter()
            .enumerate()
            .map(|(i, fc)| format!("run {} {}", i + 1, part(fc)))
            .collect();
        each.join(", ")
    };
    let diagnosis = |d: &super::Diagnosis| {
        format!(
            "code {} with {} degrees of freedom, conflicting {:?}, redundant {:?}, partially redundant {:?}, malformed {:?}",
            d.code, d.dof, d.conflicting, d.redundant, d.partially_redundant, d.malformed
        )
    };
    let before = |fc: &FreecadEdited| diagnosis(&fc.before);
    if differs(&before) {
        varied.push(format!("its solve before the edit ({})", each(&before)));
    }
    if edited.iter().any(|fc| fc.ids_before != one.ids_before) {
        varied.push("its geometry's IDs before the edit".to_owned());
    }
    for (what, part) in [
        (
            "its places after the edit, before the solve",
            (|fc: &FreecadEdited| fc.edited.clone()) as fn(&FreecadEdited) -> Vec<FcGeometry>,
        ),
        ("its places after the solve", |fc| fc.geometry.clone()),
        ("its places from Silico's start, after the solve", |fc| {
            fc.placed
                .as_ref()
                .map(|p| p.geometry.clone())
                .unwrap_or_default()
        }),
    ] {
        if let Some(said) = places_varied(&edited, part) {
            varied.push(format!("{what} ({said})"));
        }
    }
    if edited.iter().any(|fc| fc.constraints != one.constraints) {
        varied.push("its constraints".to_owned());
    }
    let after = |fc: &FreecadEdited| diagnosis(&fc.after);
    if differs(&after) {
        varied.push(format!("its solve ({})", each(&after)));
    }
    let placed = |fc: &FreecadEdited| {
        fc.placed
            .as_ref()
            .map_or_else(|| "none".to_owned(), |p| diagnosis(&p.solve))
    };
    if differs(&placed) {
        varied.push(format!("its solve from Silico's start ({})", each(&placed)));
    }
    if edited.iter().any(|fc| fc.applied != one.applied) {
        varied.push("the note's forms it was brought to".to_owned());
    }
    varied
}

/// How the geometry `part` takes of each of `runs` varies, if it does: the places (and radii)
/// that differ, how many, and the most any is apart; or that its curves aren't the same.
fn places_varied(
    runs: &[&FreecadEdited],
    part: fn(&FreecadEdited) -> Vec<FcGeometry>,
) -> Option<String> {
    let parts: Vec<Vec<FcGeometry>> = runs.iter().map(|fc| part(fc)).collect();
    let (first, rest) = parts.split_first()?;
    if rest.iter().all(|r| r == first) {
        return None;
    }
    let same_curves = rest.iter().all(|r| {
        r.len() == first.len()
            && r.iter().zip(first).all(|(a, b)| {
                a.id == b.id
                    && a.kind == b.kind
                    && a.construction == b.construction
                    && a.points.keys().eq(b.points.keys())
                    && a.radius.is_some() == b.radius.is_some()
            })
    });
    if !same_curves {
        return Some("its curves aren't the same".to_owned());
    }
    let (mut places, mut farthest) = (0usize, 0.0f64);
    for (i, g) in first.iter().enumerate() {
        for (pos, at) in &g.points {
            let apart = rest
                .iter()
                .filter_map(|r| r.get(i).and_then(|o| o.points.get(pos)))
                .map(|o| (o[0] - at[0]).abs().max((o[1] - at[1]).abs()))
                .fold(0.0, f64::max);
            if apart > 0.0 {
                places += 1;
                farthest = farthest.max(apart);
            }
        }
        if let Some(r) = g.radius {
            let apart = rest
                .iter()
                .filter_map(|o| o.get(i).and_then(|o| o.radius))
                .map(|o| (o - r).abs())
                .fold(0.0, f64::max);
            if apart > 0.0 {
                places += 1;
                farthest = farthest.max(apart);
            }
        }
    }
    Some(format!(
        "{places} places or radii, up to {farthest:e} mm apart"
    ))
}
