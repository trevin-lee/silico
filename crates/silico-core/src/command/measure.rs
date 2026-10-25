//! M4.5's measures (docs/design/m4.1-command-api.md, section 7; docs/ports/measure.md):
//! `mass_properties`, a solid's volume, area, and centre of mass as M1's `measure` gives
//! them (OCCT's integration, asked for a relative 1e-9: docs/ports/measure.md lists the
//! shapes it's known to miss that on), read from its rebuild's summary, so it asks the
//! kernel nothing: volume and area as the feature's rebuilt fingerprint holds them, as
//! `format` writes it, to a millionth of a mm³ and mm² (a stored fingerprint that differs
//! is `check`'s drift); the centre, which no fingerprint holds, rounded to the nanometre,
//! as a fingerprint's box is.

use super::elements::summarized;
use super::{ModelChoice, Problems, Running, SessionId, Sessions};
use crate::document::{FeatureId, Fingerprint, Length, Measure};
use crate::kernel::Clients;
use crate::recompute::Model;
use crate::{Error, ErrorCode};

impl<P: Clients + 'static> Sessions<P> {
    /// `mass_properties` (M4.5): the mass properties of the feature `feature`'s solid, or of
    /// every body, of the model `model` names (decision 33) ([`mass_properties`]).
    pub fn mass_properties(
        &self,
        session: &SessionId,
        model: ModelChoice,
        feature: Option<FeatureId>,
    ) -> Running<Bodies> {
        self.read_model(session, "mass_properties", model, move |read| {
            mass_properties(read.model(), feature.as_ref())
        })
    }
}

/// A solid's mass properties: its volume (mm³) and area (mm²), to 6 decimals, as its
/// fingerprint holds them, and its centre of mass, to the nanometre.
#[derive(Debug, Clone, PartialEq)]
pub struct MassProperties {
    /// The feature whose solid it is.
    pub feature: FeatureId,
    pub volume: Measure,
    pub area: Measure,
    pub center_of_mass: [Length; 3],
}

/// `mass_properties`' answer: each solid's, in timeline order.
#[derive(Debug, Clone, PartialEq)]
pub struct Bodies {
    pub bodies: Vec<MassProperties>,
}

/// `mass_properties` on `model`: the feature `feature`'s solid's, or, for none, each
/// body's (the solids no later feature consumed, in timeline order). Refused as `elements`
/// refuses a solid it can't read (naming `feature` if the request did), for each solid that
/// can't be read, every one said; and for a solid whose summary the kernel refused, its
/// refusal.
pub fn mass_properties(model: &Model, feature: Option<&FeatureId>) -> Result<Bodies, Problems> {
    let _span = tracing::debug_span!("mass_properties", ?feature).entered();
    let mut refused = Problems::none();
    let mut bodies = Vec::new();
    let asked: Vec<(&FeatureId, bool)> = match feature {
        Some(id) => vec![(id, true)],
        None => model.bodies().iter().map(|id| (id, false)).collect(),
    };
    for (id, asked) in asked {
        match properties(model, id, asked) {
            Ok(body) => bodies.push(body),
            Err(problems) => refused.absorb(problems),
        }
    }
    refused.into_result()?;
    Ok(Bodies { bodies })
}

/// The feature `id`'s solid's mass properties, or why it has none.
fn properties(model: &Model, id: &FeatureId, asked: bool) -> Result<MassProperties, Problems> {
    let summary = summarized(model, id, asked)?;
    // The fingerprint's own rounding, so the two never disagree.
    let fingerprint = Fingerprint::of(summary);
    let center = summary.center.map(Length::rounded);
    match (fingerprint, center) {
        (Some(fingerprint), [Some(x), Some(y), Some(z)]) => Ok(MassProperties {
            feature: id.clone(),
            volume: fingerprint.volume,
            area: fingerprint.area,
            center_of_mass: [x, y, z],
        }),
        // The core refuses a summary that isn't finite when it's had, and the gate bounds a
        // solid to a kilometre: a finite value past what these hold is the worker's, a
        // summary no admitted solid has (as `check` takes it, never Silico's bug).
        _ => Err(Error::new(
            ErrorCode::ProtocolError,
            format!(
                "the kernel worker measured {id}'s solid past what a measure or a length holds"
            ),
        )
        .with_feature(id.as_str())
        .with_suggestion("start a new kernel worker")
        .into()),
    }
}
