//! The sketch sweeps' reruns (#541): FreeCAD 1.1.3 doesn't answer every case the same run to
//! run (its unknowns are ordered by memory address, docs/ports/sketcher.md:51, so its solves
//! and drags land by the process's state), and a generated case it varies on is no oracle for
//! what varies. So a sweep's case that differs is run through FreeCAD [`RERUNS`] times more,
//! each alone in a FreeCAD of its own, and judged again against every run
//! ([`rejudge`]). Where FreeCAD's runs agree (their numbers within [`REPRODUCED`] of the first
//! run's), the case is compared as it was. Where they disagree, a difference is still reported
//! wherever every run gives it, read with Silico's side and the line's kind alone (FreeCAD's
//! content, its places, kinds, names, values, counts, and the kind of outcome it gave, left
//! out, [`Details::Keyed`]): a difference is left uncompared only where
//! some run of FreeCAD agrees with Silico there, within the comparison's own tolerance, and
//! it's listed with what of FreeCAD's answer varied. A difference of Silico's in what FreeCAD
//! reproduced is never left out, nor one from where every run of FreeCAD is.
//!
//! The hand cases and the gate's tests don't rerun: [`run_rerunning`] is the sweeps' alone.

use std::collections::{BTreeMap, BTreeSet};

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
    rerun_differing(&prepared, theirs, MOST_RERUN, |p| {
        Ok(freecad_outcomes(std::slice::from_ref(p))?
            .into_iter()
            .next()
            .flatten())
    })
}

/// Each of `prepared`'s verdicts against FreeCAD's `first` run of it, in order; and each that
/// differs, up to `most` of them in order, run [`RERUNS`] times more through `alone` (FreeCAD's
/// outcome of the one case, in a FreeCAD of its own) and judged by [`rejudge`]. A differing
/// case past `most` is compared as its first run has it, and said to be
/// ([`Reran::Unjudged`]).
pub fn rerun_differing(
    prepared: &[Prepared],
    first: Vec<Option<Theirs>>,
    most: usize,
    mut alone: impl FnMut(&Prepared) -> Result<Option<Theirs>, String>,
) -> Result<(Vec<Verdict>, Vec<Rerun>), String> {
    let mut verdicts = Vec::with_capacity(prepared.len());
    let mut reruns = Vec::new();
    let mut rerun = 0usize;
    for (p, first) in prepared.iter().zip(first) {
        let v = verdict(p, first.as_ref());
        if v.how.is_empty() || !matches!(p.plan, Plan::Run { .. }) {
            verdicts.push(v);
            continue;
        }
        if rerun >= most {
            tracing::warn!(case = %p.name, most, "a differing case not run again");
            reruns.push(Rerun {
                case: p.name.clone(),
                runs: 1,
                reran: Reran::Unjudged(format!(
                    "not run again: the sweep runs at most {most} of its differing cases again"
                )),
            });
            verdicts.push(v);
            continue;
        }
        rerun += 1;
        let _span = tracing::info_span!("sketch_edit_rerun", case = %p.name).entered();
        let mut runs = vec![first];
        for _ in 0..RERUNS {
            runs.push(alone(p)?);
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
/// don't: a difference is kept where each run compared gives it, read without FreeCAD's side
/// of it ([`Details::Keyed`]: where FreeCAD put a point, a radius, its degrees of freedom, its
/// values, how it worded a refusal or ended), as often as the run giving it least often does;
/// so a difference is left uncompared only where some run of FreeCAD agrees with Silico there,
/// within the comparison's own tolerance. The runs compared are those where FreeCAD made the
/// edit as Silico did, if any did, and every run otherwise: what FreeCAD's edit makes
/// (its constraints, its degrees of freedom and diagnostics, its places before the solve) is
/// its edit code's, compared wherever it made the edit, and only what kind of outcome it gave
/// in another run (a raise, a crash, a rollback) is left uncompared; and where Silico's solve
/// succeeded and some of those runs' solves failed, where the solves put things is compared
/// against the runs whose solve succeeded, one or more ([`Verdict::solved`]). The verdict is
/// the first compared run's, with those differences. A refusal of Silico's by no rule of the
/// note's is a difference whatever FreeCAD answers, so it's kept whole; and a run the harness
/// failed on, or FreeCAD wasn't run for, leaves the first run's verdict as it is
/// ([`Reran::Unjudged`]).
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
    let made: Vec<usize> = (0..snapped.len())
        .filter(|&i| made_as_silico(p, &snapped[i]))
        .collect();
    let compared: Vec<usize> = if !made.is_empty() {
        made
    } else {
        (0..snapped.len()).collect()
    };
    // The compared runs whose solve succeeded: only they give where the solves put things,
    // and only where Silico's succeeded too (none of a run's places otherwise).
    let solved: Vec<usize> = compared
        .iter()
        .copied()
        .filter(|&i| matches!(&snapped[i], Theirs::Edited(fc) if fc.after.code == 0))
        .collect();
    let kept = common(&compared, |i| keyed.get(i).map(|k| &k.how[..]));
    let kept_solved = common(&solved, |i| keyed.get(i).map(solved_lines));
    // Each run's differences, each kept (a difference every compared run gives, or a place
    // every run whose solve succeeded gives) or not, and whether it compares where the solves
    // put things.
    // A point Silico's solve didn't move, which a compared run gives as a difference where
    // FreeCAD's solve didn't move it either: kept unless some run has it at Silico's place. A
    // run whose solve moved it doesn't compare it, which isn't agreeing with Silico.
    let agreed: BTreeSet<&str> = full
        .iter()
        .flat_map(|f| f.unmoved.iter())
        .filter(|(_, agrees)| **agrees)
        .map(|(point, _)| point.as_str())
        .collect();
    let mut held: Vec<String> = Vec::new();
    for f in compared.iter().filter_map(|&i| full.get(i)) {
        for point in f.unmoved_lines.values() {
            if !agreed.contains(point.as_str()) && !held.contains(point) {
                held.push(point.clone());
            }
        }
    }
    let marked = |i: usize| -> Vec<(String, bool, bool)> {
        let (Some(f), Some(k)) = (full.get(i), keyed.get(i)) else {
            return Vec::new();
        };
        let mut left = if compared.contains(&i) {
            kept.clone()
        } else {
            BTreeMap::new()
        };
        let mut left_solved = if solved.contains(&i) {
            kept_solved.clone()
        } else {
            BTreeMap::new()
        };
        let first_solved = k.how.len().saturating_sub(k.solved);
        f.how
            .iter()
            .zip(&k.how)
            .enumerate()
            .map(|(n, (line, key))| {
                let place = n >= first_solved;
                let keep = take(&mut left, key)
                    || (place && take(&mut left_solved, key))
                    || (compared.contains(&i)
                        && f.unmoved_lines.get(line).is_some_and(|q| held.contains(q)));
                (line.clone(), keep, place)
            })
            .collect()
    };
    let base = compared.first().copied().unwrap_or(0);
    let Some(based) = full.get(base) else {
        return unjudged("the harness's own fault: no run to compare".to_owned());
    };
    let mut v = based.clone();
    let mut kept_lines = marked(base);
    // The places from the first run whose solve succeeded, where the base's failed.
    if let Some(&at) = solved.first()
        && !solved.contains(&base)
    {
        kept_lines.extend(marked(at).into_iter().filter(|(_, _, place)| *place));
    }
    // Each held point's difference, from the first compared run that gives it, where the
    // base doesn't.
    for point in &held {
        if based.unmoved_lines.values().any(|q| q == point) {
            continue;
        }
        let given = compared.iter().filter_map(|&i| full.get(i)).find_map(|f| {
            f.unmoved_lines
                .iter()
                .find(|(_, q)| *q == point)
                .map(|(line, _)| line)
        });
        if let Some(line) = given {
            kept_lines.push((line.clone(), true, true));
        }
    }
    kept_lines.retain(|(_, keep, _)| *keep);
    v.solved = kept_lines.iter().filter(|(_, _, place)| *place).count();
    v.how = kept_lines.into_iter().map(|(line, _, _)| line).collect();
    // What each run gave that isn't kept, with the runs that gave it, in the runs' order.
    let mut uncompared: Vec<(String, Vec<usize>)> = Vec::new();
    for i in 0..full.len() {
        for (line, keep, _) in marked(i) {
            if keep {
                continue;
            }
            match uncompared.iter_mut().find(|(l, _)| *l == line) {
                Some((_, runs)) if !runs.contains(&(i + 1)) => runs.push(i + 1),
                Some(_) => {}
                None => uncompared.push((line, vec![i + 1])),
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

/// A keyed verdict's last differences, those comparing where the solves put things.
fn solved_lines(v: &Verdict) -> &[String] {
    v.how
        .get(v.how.len().saturating_sub(v.solved)..)
        .unwrap_or(&[])
}

/// The differences every one of `runs` gives (each run's by `lines`), each as many times as
/// the run giving it least often does; none for no runs.
fn common<'a>(
    runs: &[usize],
    lines: impl Fn(usize) -> Option<&'a [String]>,
) -> BTreeMap<String, usize> {
    let count = |i: usize| {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for line in lines(i).unwrap_or(&[]) {
            *counts.entry(line.clone()).or_default() += 1;
        }
        counts
    };
    let Some((&first, rest)) = runs.split_first() else {
        return BTreeMap::new();
    };
    let mut kept = count(first);
    for &i in rest {
        let counts = count(i);
        for (line, n) in &mut kept {
            *n = (*n).min(counts.get(line).copied().unwrap_or(0));
        }
    }
    kept
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

/// `run` taken as `first` wherever it gives the same answer: each place and radius of a curve
/// `first` has too (the same ID and kind), and each value of a constraint it has too (the same
/// name, kind, and references), within [`REPRODUCED`] of `first`'s is `first`'s, exactly,
/// whatever else either has; and a crash or hang ended the same way (its first line) is
/// `first`'s, what FreeCAD last wrote before it aside. Anything else is `run`'s own.
pub fn snapped(first: &Theirs, run: &Theirs) -> Theirs {
    match (first, run) {
        (Theirs::Lost(a), Theirs::Lost(b)) if a.lines().next() == b.lines().next() => first.clone(),
        (Theirs::Edited(a), Theirs::Edited(b)) => {
            let mut out: FreecadEdited = (**b).clone();
            snap_geometry(&a.edited, &mut out.edited);
            snap_geometry(&a.geometry, &mut out.geometry);
            for got in &mut out.constraints {
                if let Some(want) = a
                    .constraints
                    .iter()
                    .find(|w| w.name == got.name && w.kind == got.kind && w.refs == got.refs)
                {
                    snap(want.value, &mut got.value);
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
    for got in run {
        let Some(want) = first.iter().find(|w| w.id == got.id && w.kind == got.kind) else {
            continue;
        };
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

#[cfg(test)]
mod tests {
    use super::super::Diagnosis;
    use super::*;

    fn solve() -> Diagnosis {
        Diagnosis {
            code: 0,
            dof: 0,
            conflicting: Vec::new(),
            redundant: Vec::new(),
            partially_redundant: Vec::new(),
            malformed: Vec::new(),
        }
    }

    /// A circle about `centre` of radius `radius`, its ID `id`.
    fn circle(id: i64, centre: [f64; 2], radius: f64) -> FcGeometry {
        FcGeometry {
            id,
            kind: "circle".to_owned(),
            points: BTreeMap::from([("3".to_owned(), centre)]),
            radius: Some(radius),
            construction: false,
        }
    }

    /// A line from `a` to `b`, its ID `id`.
    fn line(id: i64, a: [f64; 2], b: [f64; 2]) -> FcGeometry {
        FcGeometry {
            id,
            kind: "line".to_owned(),
            points: BTreeMap::from([("1".to_owned(), a), ("2".to_owned(), b)]),
            radius: None,
            construction: false,
        }
    }

    /// FreeCAD's edit with `geometry` after the solve and nothing else.
    fn edited(geometry: Vec<FcGeometry>) -> Theirs {
        Theirs::Edited(Box::new(FreecadEdited {
            before: solve(),
            ids_before: Vec::new(),
            edited: Vec::new(),
            geometry,
            constraints: Vec::new(),
            after: solve(),
            applied: Vec::new(),
            placed: None,
        }))
    }

    /// What varied between a run of `first` and two of `other`, after the solve.
    fn after_solve(first: Vec<FcGeometry>, other: Vec<FcGeometry>) -> Vec<String> {
        varied(&[edited(first), edited(other.clone()), edited(other)])
    }

    fn said(how: &str) -> Vec<String> {
        vec![format!("its places after the solve ({how})")]
    }

    #[test]
    fn places_varied_counts_each_place_and_radius_apart_and_the_farthest() {
        // The line's end 0.5 off in x and 2 off in y (2 the farthest, of a point), the
        // circle's radius 0.25 off and its centre where it was: 2 places or radii, up to 2.
        let first = vec![line(1, [0.0, 0.0], [4.0, 1.0]), circle(2, [9.0, 9.0], 2.0)];
        let other = vec![line(1, [0.0, 0.0], [4.5, 3.0]), circle(2, [9.0, 9.0], 2.25)];
        assert_eq!(
            after_solve(first.clone(), other),
            said("2 places or radii, up to 2e0 mm apart")
        );
        // The radius alone 0.25 off: 1, up to 0.25 (and not 2.25 + 2 or 2.25 / 2).
        let other = vec![line(1, [0.0, 0.0], [4.0, 1.0]), circle(2, [9.0, 9.0], 2.25)];
        assert_eq!(
            after_solve(first.clone(), other),
            said("1 places or radii, up to 2.5e-1 mm apart")
        );
        // The end alone 0.5 off in y, x where it was: 1, up to 0.5.
        let other = vec![line(1, [0.0, 0.0], [4.0, 1.5]), circle(2, [9.0, 9.0], 2.0)];
        assert_eq!(
            after_solve(first, other),
            said("1 places or radii, up to 5e-1 mm apart")
        );
    }

    #[test]
    fn places_varied_says_curves_otherwise_whichever_one_of_them_differs() {
        // Each run's curves the first's but in one respect: then they aren't the same, and
        // no place is counted.
        let base = || vec![line(1, [0.0, 0.0], [4.0, 1.0]), circle(2, [9.0, 9.0], 2.0)];
        let other_ways: Vec<(&str, Vec<FcGeometry>)> = vec![
            ("a curve more", {
                let mut g = base();
                g.push(line(3, [5.0, 5.0], [6.0, 6.0]));
                g
            }),
            ("another ID", {
                let mut g = base();
                g[0].id = 7;
                g
            }),
            ("another kind", {
                let mut g = base();
                g[0].kind = "arc".to_owned();
                g
            }),
            ("construction", {
                let mut g = base();
                g[0].construction = true;
                g
            }),
            ("other points", {
                let mut g = base();
                g[0].points.insert("3".to_owned(), [2.0, 0.5]);
                g
            }),
            ("a radius", {
                let mut g = base();
                g[0].radius = Some(1.0);
                g
            }),
        ];
        for (how, other) in other_ways {
            assert_eq!(
                after_solve(base(), other),
                said("its curves aren't the same"),
                "{how}"
            );
        }
    }

    #[test]
    fn a_rerun_case_is_said_as_its_runs_gave_it() {
        let rerun = |reran: Reran| Rerun {
            case: "edit_1_2".to_owned(),
            runs: 3,
            reran,
        };
        assert_eq!(
            rerun(Reran::Alike).to_string(),
            "rerun: edit_1_2: FreeCAD gave the same answer in all 3 runs, so the case is compared in full"
        );
        assert_eq!(
            rerun(Reran::Varied {
                varied: vec!["its constraints".to_owned(), "its solve (x)".to_owned()],
                uncompared: Vec::new(),
            })
            .to_string(),
            "varies: edit_1_2: FreeCAD's 3 runs differ in its constraints; its solve (x), but every difference is one each run gives, so the case is compared in full"
        );
        assert_eq!(
            rerun(Reran::Varied {
                varied: vec!["its constraints".to_owned()],
                uncompared: vec!["a (run 1)".to_owned(), "b (run 2, 3)".to_owned()],
            })
            .to_string(),
            "varies: edit_1_2: FreeCAD's 3 runs differ in its constraints, so what some run doesn't give isn't compared: a (run 1); b (run 2, 3)"
        );
        assert_eq!(
            rerun(Reran::Unjudged("why".to_owned())).to_string(),
            "rerun: edit_1_2: why"
        );
    }

    #[test]
    fn take_takes_each_key_as_many_times_as_it_has() {
        let mut left = BTreeMap::from([("a".to_owned(), 2), ("b".to_owned(), 0)]);
        assert_eq!(
            [
                take(&mut left, "a"),
                take(&mut left, "a"),
                take(&mut left, "a"),
                take(&mut left, "b"),
                take(&mut left, "c"),
            ],
            [true, true, false, false, false]
        );
        assert_eq!(
            left,
            BTreeMap::from([("a".to_owned(), 0), ("b".to_owned(), 0)])
        );
    }
}
