//! The sketch sweeps' reruns (#541): FreeCAD 1.1.3 doesn't answer every case the same run to
//! run (its unknowns are ordered by memory address, docs/ports/sketcher.md:51, so its solves
//! and drags land by the process's state), and a generated case it varies on is no oracle for
//! what varies. So a sweep's case that differs is run through FreeCAD [`RERUNS`] times more,
//! each alone in a FreeCAD of its own, and judged again against every run
//! ([`rejudge`]). Where FreeCAD's runs agree (their numbers within [`REPRODUCED`] of the first
//! run's), the case is compared as it was. Where they disagree, a difference is still reported
//! wherever every run gives it, read without FreeCAD's side of it (where it put a point, its
//! degrees of freedom, how it worded a refusal): a difference is left uncompared only where
//! some run of FreeCAD agrees with Silico there, within the comparison's own tolerance, and
//! it's listed with what of FreeCAD's answer varied. A difference of Silico's in what FreeCAD
//! reproduced is never left out, nor one from where every run of FreeCAD is.
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
    if full
        .iter()
        .zip(&keyed)
        .any(|(f, k)| f.how.len() != k.how.len() || f.solved != k.solved)
    {
        return unjudged(
            "the harness's own fault: a run's differences read otherwise without FreeCAD's details"
                .to_owned(),
        );
    }
    let made: Vec<usize> = (0..snapped.len())
        .filter(|&i| made_as_silico(p, &snapped[i]))
        .collect();
    let compared: Vec<usize> = if !made.is_empty() {
        made
    } else {
        (0..snapped.len()).collect()
    };
    // Where Silico's solve succeeded, the compared runs whose solve did too, if not all did.
    let silico_solved = matches!(&p.silico, SilicoSide::Edited(s) if s.ours.outcome.is_ok());
    let solved: Vec<usize> = compared
        .iter()
        .copied()
        .filter(|&i| matches!(&snapped[i], Theirs::Edited(fc) if fc.after.code == 0))
        .collect();
    let solved = if silico_solved && solved.len() < compared.len() {
        solved
    } else {
        Vec::new()
    };
    let kept = common(&compared, |i| keyed.get(i).map(|k| &k.how[..]));
    let kept_solved = common(&solved, |i| keyed.get(i).map(solved_lines));
    // Each run's differences, each kept (a difference every compared run gives, or a place
    // every run whose solve succeeded gives) or not, and whether it compares where the solves
    // put things.
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
                let keep = take(&mut left, key) || (place && take(&mut left_solved, key));
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
