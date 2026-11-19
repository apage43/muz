use std::{fmt, time::Instant};

use thiserror::Error;

use crate::{
    model,
    reconcile::{ReconcileError, ReconcileOperation, ReconcilePlan, plan_reconciliation},
};

use super::{
    AudioConfig, AudioEngine, DeviceError, EngineError, TransportSnapshot,
    engine::{
        DeviceRetention, EngineFade, FadeDirection, PlaybackAudibility, ProcessorPool, StructuralTransactionApplyError,
        TrackRetention,
    },
};
#[cfg(feature = "desktop")]
use super::observation::PreparedObservation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceSlot {
    BusInsert { bus: usize, insert: usize },
    TrackInstrument { track: usize },
    TrackInsert { track: usize, insert: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PreparedValueOperation {
    SetParameter {
        slot: DeviceSlot,
        expected_device_id: model::Id,
        name: String,
        value: f32,
    },
    UpdateTransport {
        transport: model::Transport,
    },
}

#[derive(Debug)]
pub struct PreparedValueTransaction {
    pub(crate) current_signature: u64,
    pub(crate) candidate_signature: u64,
    pub(crate) schedules: Vec<(usize, model::Id, super::engine::TrackSchedule)>,
    pub(crate) mix: Option<super::engine::PreparedMix>,
    pub(crate) config: Option<AudioConfig>,
    pub(crate) transport: Option<super::transport::RuntimeTransport>,
    pub(crate) runtime_loop_policy: Option<Option<(u64, u64)>>,
    #[cfg(feature = "desktop")]
    pub(crate) runtime_observation: Option<Option<PreparedObservation>>,
    revision: u64,
    observed_generation: u64,
    event_started: Option<Instant>,
    operations: Vec<PreparedValueOperation>,
    applied: bool,
}

impl PreparedValueTransaction {
    pub fn prepare(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: impl Into<Option<Instant>>,
    ) -> Result<Self, ValueTransactionPrepareError> {
        Self::prepare_inner(
            current,
            candidate,
            plan,
            observed_generation,
            event_started.into(),
            None,
            None,
        )
    }
    pub fn prepare_with_config(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: impl Into<Option<Instant>>,
        config: AudioConfig,
    ) -> Result<Self, ValueTransactionPrepareError> {
        Self::prepare_inner(
            current,
            candidate,
            plan,
            observed_generation,
            event_started.into(),
            Some(config),
            None,
        )
    }
    /// `current_signature`, when the caller has already computed it, saves
    /// hashing the current description again.
    fn prepare_inner(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: Option<Instant>,
        config: Option<AudioConfig>,
        current_signature: Option<u64>,
    ) -> Result<Self, ValueTransactionPrepareError> {
        if requires_structural(current, candidate, plan) {
            return Err(ValueTransactionPrepareError::Validation(
                "revision requires structural preparation".into(),
            ));
        }
        let candidate_signature = crate::snapshot::signature(candidate)
            .map_err(|e| ValueTransactionPrepareError::Validation(e.to_string()))?;
        candidate
            .validate_sample_coverage()
            .map_err(ValueTransactionPrepareError::SampleCoverage)?;
        let schedule_changed = current.transport != candidate.transport
            || current
                .tracks
                .iter()
                .zip(&candidate.tracks)
                .any(|(a, b)| a.source != b.source);
        let transport = if schedule_changed {
            let config = config.ok_or(ValueTransactionPrepareError::ConfigurationRequired)?;
            Some(
                super::transport::RuntimeTransport::from_session(
                    f64::from(config.sample_rate),
                    candidate,
                )
                .map_err(|s| ValueTransactionPrepareError::Validation(s.into()))?,
            )
        } else {
            None
        };
        let mut schedules = Vec::new();
        if schedule_changed {
            let config = config.ok_or(ValueTransactionPrepareError::ConfigurationRequired)?;
            for (index, track) in candidate.tracks.iter().enumerate() {
                schedules.push((
                    index,
                    track.id.clone(),
                    super::engine::TrackSchedule::prepare(
                        track,
                        transport.as_ref().expect("prepared timeline").timeline(),
                        config,
                    )
                    .map_err(|e| ValueTransactionPrepareError::Validation(e.to_string()))?,
                ));
            }
        }
        // A caller that supplies the current signature planned from these
        // descriptions itself; anyone else's plan is checked against them.
        if current_signature.is_none()
            && plan_reconciliation(plan.base_revision, current, candidate)? != *plan
        {
            return Err(ValueTransactionPrepareError::PlanDoesNotMatchSessions);
        }
        let mix_changed = current.extras.automation != candidate.extras.automation
            || plan
                .operations
                .iter()
                .any(|o| matches!(o, ReconcileOperation::UpdateRoute { .. }));
        let mix = if mix_changed {
            validate_lane_values(candidate)?;
            Some(
                super::engine::PreparedMix::prepare(candidate)
                    .map_err(|e| ValueTransactionPrepareError::Validation(e.to_string()))?,
            )
        } else {
            None
        };

        let revision = plan
            .base_revision
            .checked_add(1)
            .ok_or(ValueTransactionPrepareError::RevisionOverflow)?;
        let mut operations = Vec::with_capacity(plan.operations.len());
        for (operation_index, operation) in plan.operations.iter().enumerate() {
            match operation {
                ReconcileOperation::SetParameters { device_id, deltas } => {
                    let (slot, _, current_device) =
                        find_device(current, device_id).ok_or_else(|| {
                            ValueTransactionPrepareError::MissingDevice(device_id.clone())
                        })?;
                    let (candidate_slot, _, candidate_device) = find_device(candidate, device_id)
                        .ok_or_else(|| {
                        ValueTransactionPrepareError::MissingDevice(device_id.clone())
                    })?;
                    if slot != candidate_slot
                        || !current_device.same_structural_identity(candidate_device)
                    {
                        return Err(ValueTransactionPrepareError::DeviceIdentityChanged(
                            device_id.clone(),
                        ));
                    }
                    let candidate_values = candidate_device.control_values();
                    if !crate::description::static_controls(candidate_device.kind) {
                        return Err(ValueTransactionPrepareError::Validation(
                            "control requires structural preparation".into(),
                        ));
                    }
                    for (name, delta) in deltas {
                        let Some(value) = delta else {
                            return Err(ValueTransactionPrepareError::ParameterRemoval {
                                device_id: device_id.clone(),
                                parameter: name.clone(),
                            });
                        };
                        let Some(candidate_value) = candidate_values.get(name) else {
                            return Err(ValueTransactionPrepareError::MissingCandidateParameter {
                                device_id: device_id.clone(),
                                parameter: name.clone(),
                            });
                        };
                        if candidate_value != value {
                            return Err(ValueTransactionPrepareError::PlanDoesNotMatchSessions);
                        }
                        crate::description::validate_control(candidate_device, name, *value)
                            .map_err(|s| ValueTransactionPrepareError::Validation(s.into()))?;
                        push_operation(
                            &mut operations,
                            PreparedValueOperation::SetParameter {
                                slot,
                                expected_device_id: device_id.clone(),
                                name: name.clone(),
                                value: *value,
                            },
                        )?;
                    }
                }
                ReconcileOperation::UpdateTransport { transport } => {
                    if transport != &candidate.transport {
                        return Err(ValueTransactionPrepareError::PlanDoesNotMatchSessions);
                    }
                    push_operation(
                        &mut operations,
                        PreparedValueOperation::UpdateTransport {
                            transport: transport.clone(),
                        },
                    )?;
                }
                ReconcileOperation::Rename { .. }
                | ReconcileOperation::UpdateExtras
                | ReconcileOperation::UpdateRoute { .. }
                | ReconcileOperation::ReplaceTrackSource { .. } => {}
                _ => {
                    return Err(ValueTransactionPrepareError::StructuralOperation {
                        operation_index,
                    });
                }
            }
        }

        Ok(Self {
            schedules,
            mix,
            current_signature: match current_signature {
                Some(signature) => signature,
                None => crate::snapshot::signature(current)
                    .map_err(|e| ValueTransactionPrepareError::Validation(e.to_string()))?,
            },
            candidate_signature,
            config,
            transport,
            runtime_loop_policy: None,
            #[cfg(feature = "desktop")]
            runtime_observation: None,
            revision,
            observed_generation,
            event_started,
            operations,
            applied: false,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn observed_generation(&self) -> u64 {
        self.observed_generation
    }

    pub fn event_started(&self) -> Option<Instant> {
        self.event_started
    }

    pub fn operations(&self) -> &[PreparedValueOperation] {
        &self.operations
    }

    pub fn is_applied(&self) -> bool {
        self.applied
    }

    pub(crate) fn operations_mut(
        &mut self,
    ) -> Result<&mut [PreparedValueOperation], ValueTransactionApplyError> {
        if self.applied {
            Err(ValueTransactionApplyError::AlreadyApplied)
        } else {
            Ok(&mut self.operations)
        }
    }

    pub(crate) fn mark_applied(&mut self) {
        self.applied = true;
    }
}

pub struct PreparedStructuralTransaction {
    current_signature: u64,
    revision: u64,
    observed_generation: u64,
    event_started: Option<Instant>,
    current_session: Box<model::Session>,
    candidate_session: Box<model::Session>,
    candidate_engine: AudioEngine,
    device_retentions: Vec<DeviceRetention>,
    track_retentions: Vec<TrackRetention>,
    old_fade: EngineFade,
    new_fade: EngineFade,
    applied: bool,
}

impl fmt::Debug for PreparedStructuralTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedStructuralTransaction")
            .field("revision", &self.revision)
            .field("observed_generation", &self.observed_generation)
            .field("device_retentions", &self.device_retentions.len())
            .field("track_retentions", &self.track_retentions.len())
            .field("old_fade", &self.old_fade)
            .field("new_fade", &self.new_fade)
            .field("applied", &self.applied)
            .finish_non_exhaustive()
    }
}

impl PreparedStructuralTransaction {
    pub fn prepare(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: impl Into<Option<Instant>>,
        config: AudioConfig,
    ) -> Result<Self, StructuralTransactionPrepareError> {
        Self::prepare_with_pool(
            current,
            candidate,
            plan,
            observed_generation,
            event_started,
            config,
            &mut ProcessorPool::new(),
        )
    }

    /// Like [`Self::prepare`], placing pooled processors at the devices this
    /// transaction keeps instead of preparing them again.
    pub fn prepare_with_pool(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: impl Into<Option<Instant>>,
        config: AudioConfig,
        pool: &mut ProcessorPool,
    ) -> Result<Self, StructuralTransactionPrepareError> {
        Self::prepare_inner(
            current,
            candidate,
            plan,
            observed_generation,
            event_started.into(),
            config,
            pool,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_inner(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: Option<Instant>,
        config: AudioConfig,
        pool: &mut ProcessorPool,
        current_signature: Option<u64>,
    ) -> Result<Self, StructuralTransactionPrepareError> {
        if current_signature.is_none()
            && plan_reconciliation(plan.base_revision, current, candidate)? != *plan
        {
            return Err(StructuralTransactionPrepareError::PlanDoesNotMatchSessions);
        }
        if !requires_structural(current, candidate, plan) {
            return Err(StructuralTransactionPrepareError::PlanHasNoStructuralOperations);
        }
        let revision = plan
            .base_revision
            .checked_add(1)
            .ok_or(StructuralTransactionPrepareError::RevisionOverflow)?;
        let device_retentions = prepare_device_retentions(current, candidate);
        let mut reuse = super::engine::Reused::new();
        for retention in &device_retentions {
            if let Some((_, _, device)) = find_device(candidate, &retention.expected_id)
                && let Some(processor) = pool.take(device, config)
            {
                reuse.insert(device.id.clone(), processor);
            }
        }
        let candidate_engine = AudioEngine::new_reusing(candidate, config, &mut reuse)?;
        let current_signature = match current_signature {
            Some(signature) => signature,
            None => crate::snapshot::signature(current)
                .map_err(|e| EngineError::Preflight(e.to_string()))?,
        };
        let track_retentions = prepare_track_retentions(current, candidate);
        let (old_fade, new_fade) = prepare_fades(current, candidate);

        Ok(Self {
            revision,
            current_signature,
            observed_generation,
            event_started,
            current_session: Box::new(current.clone()),
            candidate_session: Box::new(candidate.clone()),
            candidate_engine,
            device_retentions,
            track_retentions,
            old_fade,
            new_fade,
            applied: false,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn observed_generation(&self) -> u64 {
        self.observed_generation
    }

    pub fn event_started(&self) -> Option<Instant> {
        self.event_started
    }

    pub fn current_session(&self) -> &model::Session {
        &self.current_session
    }

    pub fn candidate_session(&self) -> &model::Session {
        &self.candidate_session
    }

    pub fn is_applied(&self) -> bool {
        self.applied
    }

    pub fn retired_engine(&self) -> Option<&AudioEngine> {
        self.applied.then_some(&self.candidate_engine)
    }

    pub fn needs_fade(&self) -> bool {
        self.old_fade.tracks != 0
            || self.old_fade.buses != 0
            || self.old_fade.master
            || self.new_fade.tracks != 0
            || self.new_fade.buses != 0
            || self.new_fade.master
    }

    pub(crate) fn fade_out(&self) -> EngineFade {
        self.old_fade
    }

    pub(crate) fn fade_in(&self) -> EngineFade {
        self.new_fade
    }

    pub fn apply(
        &mut self,
        engine: &mut AudioEngine,
    ) -> Result<(), StructuralTransactionApplyError> {
        if self.applied {
            return Err(StructuralTransactionApplyError::AlreadyApplied);
        }
        if engine.revision().checked_add(1) != Some(self.revision)
            || engine.description_signature != self.current_signature
        {
            return Err(StructuralTransactionApplyError::RuntimeMismatch);
        }
        engine.swap_structural(
            &mut self.candidate_engine,
            &self.device_retentions,
            &self.track_retentions,
            &self.candidate_session.transport,
        )?;
        engine.accept_revision(self.revision);
        self.applied = true;
        Ok(())
    }
}

#[derive(Debug)]
pub enum PreparedTransaction {
    Value(Box<PreparedValueTransaction>),
    Structural(Box<PreparedStructuralTransaction>),
}

impl From<PreparedValueTransaction> for PreparedTransaction {
    fn from(transaction: PreparedValueTransaction) -> Self {
        Self::Value(Box::new(transaction))
    }
}

impl From<PreparedStructuralTransaction> for PreparedTransaction {
    fn from(transaction: PreparedStructuralTransaction) -> Self {
        Self::Structural(Box::new(transaction))
    }
}

impl PreparedTransaction {
    pub fn prepare(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: impl Into<Option<Instant>> + Copy,
        config: AudioConfig,
    ) -> Result<Self, TransactionPrepareError> {
        Self::prepare_with_pool(
            current,
            candidate,
            plan,
            observed_generation,
            event_started,
            config,
            &mut ProcessorPool::new(),
        )
    }

    /// Like [`Self::prepare`]; a structural revision places `pool`'s
    /// processors at the devices it keeps (see [`ProcessorPool`]).
    pub fn prepare_with_pool(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: impl Into<Option<Instant>> + Copy,
        config: AudioConfig,
        pool: &mut ProcessorPool,
    ) -> Result<Self, TransactionPrepareError> {
        Self::prepare_signed(
            current,
            candidate,
            plan,
            observed_generation,
            event_started.into(),
            config,
            pool,
            None,
        )
    }

    /// [`Self::prepare_with_pool`] for a caller that has already computed the
    /// current description's signature. Supplying it also asserts that `plan`
    /// was planned from exactly these descriptions, which is then not checked
    /// again.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_signed(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: Option<Instant>,
        config: AudioConfig,
        pool: &mut ProcessorPool,
        current_signature: Option<u64>,
    ) -> Result<Self, TransactionPrepareError> {
        if requires_structural(current, candidate, plan) {
            PreparedStructuralTransaction::prepare_inner(
                current,
                candidate,
                plan,
                observed_generation,
                event_started,
                config,
                pool,
                current_signature,
            )
            .map(|transaction| Self::Structural(Box::new(transaction)))
            .map_err(TransactionPrepareError::Structural)
        } else {
            PreparedValueTransaction::prepare_inner(
                current,
                candidate,
                plan,
                observed_generation,
                event_started,
                Some(config),
                current_signature,
            )
            .map(|transaction| Self::Value(Box::new(transaction)))
            .map_err(TransactionPrepareError::Value)
        }
    }

    /// The prepared candidate description's signature.
    pub(crate) fn candidate_signature(&self) -> u64 {
        match self {
            Self::Value(transaction) => transaction.candidate_signature,
            Self::Structural(transaction) => transaction.candidate_engine.description_signature,
        }
    }

    /// Prepare host listening and loop policy outside the audio callback.
    ///
    /// `current` and `candidate` must be the descriptions used to prepare this
    /// transaction. A loop beyond the candidate's shortened timeline is cleared;
    /// a valid SFZ loop is replayed under the caller's installed host context.
    /// SFZ replay replaces the complete graph, so the checkpoint, sounding
    /// voices and schedules share the same history. A value revision that
    /// replaces no schedule (controls, levels, automation, presentation) keeps
    /// the running checkpoint instead: it changes no SFZ history.
    /// An explicit listening mask survives by physical ID intersection; new
    /// tracks remain silent. `None` retains the initial all-tracks policy.
    ///
    /// Call this before installing prepared observation state. The existing
    /// offline preparation/apply APIs do not require runtime policy preparation.
    pub fn prepare_runtime_policy(
        &mut self,
        current: &model::Session,
        candidate: &model::Session,
        loop_range: Option<(u64, u64)>,
        max_replay_frames: u64,
        audibility: Option<&[String]>,
    ) -> Result<(), EngineError> {
        self.prepare_runtime_policy_inner(
            current,
            candidate,
            loop_range,
            max_replay_frames,
            audibility.map(|ids| PlaybackAudibility::Tracks(ids.to_vec())).as_ref(),
            true,
        )
    }

    /// [`Self::prepare_runtime_policy`] for the caller that prepared this
    /// transaction from `current` and `candidate` itself: with `verify` false
    /// it does not hash them again to check that they match.
    pub(crate) fn prepare_runtime_policy_inner(
        &mut self,
        current: &model::Session,
        candidate: &model::Session,
        loop_range: Option<(u64, u64)>,
        max_replay_frames: u64,
        audibility: Option<&PlaybackAudibility>,
        verify: bool,
    ) -> Result<(), EngineError> {
        let (current_signature, candidate_signature, config, applied) = match self {
            Self::Value(transaction) => (
                transaction.current_signature,
                transaction.candidate_signature,
                transaction.config.ok_or(EngineError::InvalidConfig(
                    "runtime policy preparation requires an audio configuration",
                ))?,
                transaction.applied,
            ),
            Self::Structural(transaction) => (
                transaction.current_signature,
                transaction.candidate_engine.description_signature,
                transaction.candidate_engine.config(),
                transaction.applied,
            ),
        };
        if applied {
            return Err(EngineError::InvalidConfig(
                "runtime policy cannot be prepared after transaction application",
            ));
        }
        if verify
            && (crate::snapshot::signature(current)
                .map_err(|error| EngineError::Preflight(error.to_string()))?
                != current_signature
                || crate::snapshot::signature(candidate)
                    .map_err(|error| EngineError::Preflight(error.to_string()))?
                    != candidate_signature)
        {
            return Err(EngineError::InvalidConfig(
                "runtime policy descriptions do not match the prepared transaction",
            ));
        }
        if loop_range.is_some_and(|(start, end)| start >= end) {
            return Err(EngineError::InvalidConfig("loop end must follow start"));
        }
        let timeline = super::transport::TempoTimeline::compile(
            f64::from(config.sample_rate),
            &candidate.transport,
            &candidate.tracks,
        )
        .map_err(EngineError::InvalidGraph)?;
        let loop_range = loop_range.filter(|(_, end)| *end <= timeline.end_tick());
        // A value revision that replaces no schedule leaves every SFZ
        // instrument's history, and so the running loop checkpoint, intact.
        let keeps_history = matches!(
            self,
            Self::Value(t) if t.schedules.is_empty() && t.transport.is_none()
        );
        let replay =
            loop_range.filter(|_| !keeps_history && AudioEngine::session_uses_sfz(candidate));
        if let Some((start, end)) = replay {
            let checked = crate::description::ValidatedSession::new(candidate)
                .map_err(|error| EngineError::Preflight(error.to_string()))?;
            let mut candidate_engine =
                AudioEngine::prepare_loop_with_audibility(&checked, config, start, end, max_replay_frames, audibility)?;
            candidate_engine.install_runtime_loop_policy(loop_range);
            let old_fade = EngineFade {
                tracks: 0,
                buses: 0,
                master: true,
                direction: FadeDirection::Out,
            };
            let new_fade = EngineFade {
                direction: FadeDirection::In,
                ..old_fade
            };
            match self {
                Self::Value(transaction) => {
                    *self = Self::Structural(Box::new(PreparedStructuralTransaction {
                        current_signature,
                        revision: transaction.revision,
                        observed_generation: transaction.observed_generation,
                        event_started: transaction.event_started,
                        current_session: Box::new(current.clone()),
                        candidate_session: Box::new(candidate.clone()),
                        candidate_engine,
                        device_retentions: Vec::new(),
                        track_retentions: Vec::new(),
                        old_fade,
                        new_fade,
                        applied: false,
                    }));
                }
                Self::Structural(transaction) => {
                    transaction.candidate_engine = candidate_engine;
                    transaction.device_retentions.clear();
                    transaction.track_retentions.clear();
                    transaction.old_fade = old_fade;
                    transaction.new_fade = new_fade;
                }
            }
        } else {
            match self {
                Self::Value(transaction) => {
                    transaction.runtime_loop_policy = Some(loop_range);
                }
                Self::Structural(transaction) => {
                    transaction.candidate_engine.prepare_runtime_audibility(audibility)?;
                    transaction
                        .candidate_engine
                        .install_runtime_loop_policy(loop_range);
                }
            }
        }
        Ok(())
    }

    /// Stage capture replacement, retaining displaced state for coordinator
    /// retirement. Even value revisions replace capture storage so frames from
    /// the previous revision cannot be mislabeled with the new track mapping.
    #[cfg(feature = "desktop")]
    pub(crate) fn replace_observation(
        &mut self,
        prepared: Option<PreparedObservation>,
    ) -> Option<PreparedObservation> {
        match self {
            Self::Value(transaction) => transaction
                .runtime_observation
                .replace(prepared)
                .flatten(),
            Self::Structural(transaction) => {
                transaction.candidate_engine.replace_observation(prepared)
            }
        }
    }

    /// After a structural transaction is applied, move the processors it
    /// prepared for kept devices (never run: apply swapped the running ones
    /// in) from the retired engine into `pool`, and drop pooled processors of
    /// devices the accepted session no longer has. Call off the callback, with
    /// the transaction returned from the output.
    pub fn recycle_into(&mut self, pool: &mut ProcessorPool) {
        let Self::Structural(transaction) = self else {
            return;
        };
        if !transaction.applied {
            return;
        }
        let config = transaction.candidate_engine.config();
        for retention in &transaction.device_retentions {
            let Some((_, _, device)) =
                find_device(&transaction.candidate_session, &retention.expected_id)
            else {
                continue;
            };
            if let Some(processor) = transaction.candidate_engine.take_processor(retention.current)
            {
                pool.put(device, config, processor);
            }
        }
        pool.retain_devices_of(&transaction.candidate_session);
    }

    pub fn revision(&self) -> u64 {
        match self {
            Self::Value(transaction) => transaction.revision(),
            Self::Structural(transaction) => transaction.revision(),
        }
    }

    pub fn observed_generation(&self) -> u64 {
        match self {
            Self::Value(transaction) => transaction.observed_generation(),
            Self::Structural(transaction) => transaction.observed_generation(),
        }
    }

    pub fn event_started(&self) -> Option<Instant> {
        match self {
            Self::Value(transaction) => transaction.event_started(),
            Self::Structural(transaction) => transaction.event_started(),
        }
    }

    pub fn is_structural(&self) -> bool {
        matches!(self, Self::Structural(_))
    }

    pub fn needs_fade(&self) -> bool {
        match self {
            Self::Value(_) => false,
            Self::Structural(transaction) => transaction.needs_fade(),
        }
    }

    pub fn fade_out(&self) -> Option<EngineFade> {
        match self {
            Self::Structural(transaction) if transaction.needs_fade() => {
                Some(transaction.fade_out())
            }
            _ => None,
        }
    }

    pub fn fade_in(&self) -> Option<EngineFade> {
        match self {
            Self::Structural(transaction) if transaction.needs_fade() => {
                Some(transaction.fade_in())
            }
            _ => None,
        }
    }

    pub fn fade_recovery(&self) -> Option<EngineFade> {
        match self {
            Self::Structural(transaction) if transaction.needs_fade() => {
                let mut fade = transaction.fade_out();
                fade.direction = FadeDirection::In;
                Some(fade)
            }
            _ => None,
        }
    }

    /// Reject a stale or incompatible transaction before starting a callback fade.
    pub(crate) fn matches_runtime(&self, engine: &AudioEngine) -> bool {
        match self {
            Self::Value(transaction) => {
                !transaction.applied
                    && engine.revision().checked_add(1) == Some(transaction.revision)
                    && engine.description_signature == transaction.current_signature
                    && transaction.config.is_none_or(|config| config == engine.config())
            }
            Self::Structural(transaction) => {
                !transaction.applied
                    && engine.revision().checked_add(1) == Some(transaction.revision)
                    && engine.description_signature == transaction.current_signature
                    && engine.config() == transaction.candidate_engine.config()
                    && engine.graph_budget() == transaction.candidate_engine.graph_budget()
            }
        }
    }

    pub fn apply(&mut self, engine: &mut AudioEngine) -> Result<(), TransactionApplyError> {
        match self {
            Self::Value(transaction) => engine
                .apply_value_transaction(transaction)
                .map_err(TransactionApplyError::Value),
            Self::Structural(transaction) => transaction
                .apply(engine)
                .map_err(TransactionApplyError::Structural),
        }
    }
}


fn is_structural_operation(operation: &ReconcileOperation) -> bool {
    !matches!(
        operation,
        ReconcileOperation::SetParameters { .. } | ReconcileOperation::UpdateTransport { .. }
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum PreparationEffect {
    Presentation,
    Controls,
    Schedule,
    Structural,
}
pub fn preparation_effect(
    current: &model::Session,
    candidate: &model::Session,
    plan: &ReconcilePlan,
) -> PreparationEffect {
    if requires_structural(current, candidate, plan) {
        PreparationEffect::Structural
    } else if current.extras.automation != candidate.extras.automation
        || plan.operations.iter().any(|o| {
            matches!(
                o,
                ReconcileOperation::ReplaceTrackSource { .. }
                    | ReconcileOperation::UpdateTransport { .. }
            )
        })
    {
        PreparationEffect::Schedule
    } else if plan.operations.iter().any(|o| {
        matches!(
            o,
            ReconcileOperation::SetParameters { .. } | ReconcileOperation::UpdateRoute { .. }
        )
    }) {
        PreparationEffect::Controls
    } else {
        PreparationEffect::Presentation
    }
}
fn requires_structural(
    current: &model::Session,
    candidate: &model::Session,
    plan: &ReconcilePlan,
) -> bool {
    plan.operations.iter().any(|op| match op {
        ReconcileOperation::Rename { .. } => false,
        // The tail only bounds offline renders; live playback never reads it.
        ReconcileOperation::UpdateExtras => !lanes_retargetable(current, candidate),
        ReconcileOperation::SetParameters { device_id, .. } => find_device(candidate, device_id)
            .is_none_or(|(_, _, d)| !crate::description::static_controls(d.kind)),
        ReconcileOperation::UpdateRoute { route } => {
            !route.gain_db.is_finite()
                || find_route(current, route.id.as_str())
                    .is_none_or(|old| old.pre != route.pre || old.to != route.to)
        }
        ReconcileOperation::ReplaceTrackSource { track_id, .. } => {
            let a = current.tracks.iter().find(|t| t.id == *track_id);
            let b = candidate.tracks.iter().find(|t| t.id == *track_id);
            match (a, b) {
                (Some(a), Some(b)) => {
                    !a.source.same_identity(&b.source) || !expression_already_accepted(a, b)
                }
                _ => true,
            }
        }
        _ => is_structural_operation(op),
    })
}

/// Whether the running instrument has already accepted every note-expression
/// check the candidate source needs.
///
/// Preparation asks the instrument itself which expression slots it accepts
/// and, for a plugin or rack, what range each takes. A value revision has no
/// new instrument to ask. With the instrument unchanged, a slot the current
/// source already uses was accepted. A native instrument checks values against
/// the standard slot ranges, which need no instance; a plugin or rack must
/// already have accepted the exact value. A voice patch is checked from its
/// description when its schedule is prepared.
fn expression_already_accepted(current: &model::Track, candidate: &model::Track) -> bool {
    use std::collections::BTreeSet;
    fn checks(track: &model::Track) -> impl Iterator<Item = (u8, f64)> + '_ {
        let model::TrackSource::Midi(m) = &track.source;
        m.imported
            .notes
            .iter()
            .filter_map(|n| n.performance.as_ref())
            .flat_map(super::engine::note_expression_checks)
    }
    let instrument = &candidate.instrument;
    if instrument.kind == model::DeviceKind::VoicePatch || checks(candidate).next().is_none() {
        return true;
    }
    if !current.instrument.same_structural_identity(instrument) {
        return false;
    }
    let instance_ranges = matches!(
        instrument.kind,
        model::DeviceKind::Vst3 | model::DeviceKind::Clap | model::DeviceKind::Rack
    );
    let accepted: BTreeSet<(u8, u64)> = checks(current)
        .map(|(kind, value)| (kind, if instance_ranges { value.to_bits() } else { 0 }))
        .collect();
    checks(candidate).all(|(kind, value)| {
        if instance_ranges {
            accepted.contains(&(kind, value.to_bits()))
        } else {
            let (lo, hi) = crate::expression::default_range(kind);
            accepted.contains(&(kind, 0)) && (lo..=hi).contains(&value)
        }
    })
}

/// Whether changed automation can be installed into the running routes and
/// devices: the same targets (so each still names the same route or control),
/// with device lanes only on controls a value revision may set.
fn lanes_retargetable(current: &model::Session, candidate: &model::Session) -> bool {
    let (old, new) = (&current.extras.automation, &candidate.extras.automation);
    if old == new {
        return true;
    }
    fn targets(lanes: &[crate::description::Automation]) -> std::collections::BTreeSet<&str> {
        lanes.iter().map(|a| a.target.as_str()).collect()
    }
    old.len() == new.len()
        && targets(old) == targets(new)
        && new.iter().all(|lane| {
            find_route(candidate, &lane.target).is_some()
                || lane_device(candidate, &lane.target)
                    .is_some_and(|(d, _)| crate::description::static_controls(d.kind))
        })
}

/// The device and control a device lane's target names.
fn lane_device<'a, 't>(
    session: &'a model::Session,
    target: &'t str,
) -> Option<(&'a model::Device, &'t str)> {
    all_devices(session).into_iter().find_map(|(_, _, d)| {
        target
            .strip_prefix(d.id.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
            .map(|name| (d, name))
    })
}

/// Check changed lanes' values without a processor, as preparation would
/// with one: route lanes are finite levels, device lanes valid control values.
fn validate_lane_values(candidate: &model::Session) -> Result<(), ValueTransactionPrepareError> {
    for lane in &candidate.extras.automation {
        if find_route(candidate, &lane.target).is_some() {
            if let Some(p) = lane.points.iter().find(|p| !p.value.is_finite()) {
                return Err(ValueTransactionPrepareError::Validation(format!(
                    "automation target '{}' has a non-finite level {}",
                    lane.target, p.value
                )));
            }
            continue;
        }
        let Some((device, name)) = lane_device(candidate, &lane.target) else {
            return Err(ValueTransactionPrepareError::Validation(format!(
                "automation target '{}' is unknown",
                lane.target
            )));
        };
        for p in &lane.points {
            crate::description::validate_control(device, name, p.value).map_err(|e| {
                ValueTransactionPrepareError::Validation(format!(
                    "automation target '{}': {e}",
                    lane.target
                ))
            })?;
        }
    }
    Ok(())
}

fn find_route<'a>(session: &'a model::Session, id: &str) -> Option<&'a model::Route> {
    session
        .tracks
        .iter()
        .flat_map(|t| std::iter::once(&t.output).chain(&t.sends))
        .chain(
            std::iter::once(&session.master)
                .chain(&session.buses)
                .flat_map(|b| b.output.iter().chain(&b.sends)),
        )
        .find(|r| r.id.as_str() == id)
}

fn push_operation(
    operations: &mut Vec<PreparedValueOperation>,
    operation: PreparedValueOperation,
) -> Result<(), ValueTransactionPrepareError> {
    if operations.len() == model::MAX_TRANSACTION_OPS {
        return Err(ValueTransactionPrepareError::TooManyPreparedOperations {
            maximum: model::MAX_TRANSACTION_OPS,
        });
    }
    operations.push(operation);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortSignature {
    Instrument,
    Effect,
}

fn find_device<'a>(
    session: &'a model::Session,
    id: &model::Id,
) -> Option<(DeviceSlot, PortSignature, &'a model::Device)> {
    for (insert, device) in session.master.inserts.iter().enumerate() {
        if device.id == *id {
            return Some((
                DeviceSlot::BusInsert { bus: 0, insert },
                PortSignature::Effect,
                device,
            ));
        }
    }
    for (bus_index, bus) in session.buses.iter().enumerate() {
        for (insert, device) in bus.inserts.iter().enumerate() {
            if device.id == *id {
                return Some((
                    DeviceSlot::BusInsert {
                        bus: bus_index + 1,
                        insert,
                    },
                    PortSignature::Effect,
                    device,
                ));
            }
        }
    }
    for (track, track_source) in session.tracks.iter().enumerate() {
        if track_source.instrument.id == *id {
            return Some((
                DeviceSlot::TrackInstrument { track },
                PortSignature::Instrument,
                &track_source.instrument,
            ));
        }
        for (insert, device) in track_source.inserts.iter().enumerate() {
            if device.id == *id {
                return Some((
                    DeviceSlot::TrackInsert { track, insert },
                    PortSignature::Effect,
                    device,
                ));
            }
        }
    }
    None
}

fn all_devices(session: &model::Session) -> Vec<(DeviceSlot, PortSignature, &model::Device)> {
    let mut devices = Vec::new();
    for (insert, device) in session.master.inserts.iter().enumerate() {
        devices.push((
            DeviceSlot::BusInsert { bus: 0, insert },
            PortSignature::Effect,
            device,
        ));
    }
    for (bus, source) in session.buses.iter().enumerate() {
        for (insert, device) in source.inserts.iter().enumerate() {
            devices.push((
                DeviceSlot::BusInsert {
                    bus: bus + 1,
                    insert,
                },
                PortSignature::Effect,
                device,
            ));
        }
    }
    for (track, source) in session.tracks.iter().enumerate() {
        devices.push((
            DeviceSlot::TrackInstrument { track },
            PortSignature::Instrument,
            &source.instrument,
        ));
        for (insert, device) in source.inserts.iter().enumerate() {
            devices.push((
                DeviceSlot::TrackInsert { track, insert },
                PortSignature::Effect,
                device,
            ));
        }
    }
    devices
}

fn prepare_device_retentions(
    current: &model::Session,
    candidate: &model::Session,
) -> Vec<DeviceRetention> {
    let candidate_devices = all_devices(candidate);
    let mut retentions = Vec::new();
    for (current_slot, current_ports, current_device) in all_devices(current) {
        let Some((candidate_slot, candidate_ports, candidate_device)) = candidate_devices
            .iter()
            .find(|(_, _, device)| device.id == current_device.id)
        else {
            continue;
        };
        if !current_device.same_structural_identity(candidate_device)
            || current_ports != *candidate_ports
            || (!crate::description::static_controls(candidate_device.kind)
                && current_device.control_values() != candidate_device.control_values())
        {
            continue;
        }
        retentions.push(DeviceRetention {
            current: current_slot,
            candidate: *candidate_slot,
            expected_id: current_device.id.clone(),
            expected_kind: current_device.kind,
            parameters: candidate_device
                .control_values()
                .iter()
                .filter(|(name, value)| current_device.control_values().get(*name) != Some(*value))
                .map(|(name, value)| (name.clone(), *value))
                .collect(),
        });
    }
    retentions
}

/// Describe processor lifetime effects using the same retention decisions as apply.
pub fn try_visit_processor_consequences<E>(
    current: &model::Session,
    candidate: &model::Session,
    mut visitor: impl FnMut(&model::Id, &'static str, &'static str) -> Result<(), E>,
) -> Result<(), E> {
    let retained: std::collections::BTreeSet<_> = prepare_device_retentions(current, candidate)
        .into_iter()
        .map(|r| r.expected_id)
        .collect();
    let old: std::collections::BTreeMap<_, _> = all_devices(current)
        .into_iter()
        .map(|(_, _, d)| (d.id.clone(), d))
        .collect();
    let new: std::collections::BTreeMap<_, _> = all_devices(candidate)
        .into_iter()
        .map(|(_, _, d)| (d.id.clone(), d))
        .collect();
    for id in old
        .keys()
        .chain(new.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
    {
        let (effect, reason) = match (old.get(&id), new.get(&id)) {
            (Some(_), None) => ("removed", "device removed"),
            (None, Some(_)) => ("added", "device added"),
            (Some(_), Some(_)) if retained.contains(&id) => {
                ("retained", "structural identity and ports are compatible")
            }
            (Some(_), Some(_)) => ("replaced", "structural identity or ports changed"),
            (None, None) => unreachable!(),
        };
        visitor(&id, effect, reason)?;
    }
    Ok(())
}

pub fn processor_consequences(
    current: &model::Session,
    candidate: &model::Session,
) -> Vec<serde_json::Value> {
    let mut rows = Vec::new();
    let result: Result<(), std::convert::Infallible> =
        try_visit_processor_consequences(current, candidate, |id, effect, reason| {
            rows.push(serde_json::json!({"device_id":id.as_str(),"effect":effect,"reason":reason}));
            Ok(())
        });
    match result {
        Ok(()) => {}
        Err(x) => match x {},
    }
    rows
}

fn prepare_track_retentions(
    current: &model::Session,
    candidate: &model::Session,
) -> Vec<TrackRetention> {
    let mut retentions = Vec::with_capacity(current.tracks.len());
    for (current_index, current_track) in current.tracks.iter().enumerate() {
        if let Some((candidate_index, _)) =
            candidate.tracks.iter().enumerate().find(|(_, track)| {
                track.id == current_track.id
                    && match (&current_track.source, &track.source) {
                        (
                            model::TrackSource::Midi(current),
                            model::TrackSource::Midi(candidate),
                        ) => {
                            current.id == candidate.id
                                && current.asset == candidate.asset
                                && current.channel == candidate.channel
                                && current.all_channels == candidate.all_channels
                        }
                    }
            })
        {
            retentions.push(TrackRetention {
                current: current_index,
                candidate: candidate_index,
                expected_id: current_track.id.clone(),
            });
        }
    }
    retentions
}

fn prepare_fades(current: &model::Session, candidate: &model::Session) -> (EngineFade, EngineFade) {
    let mut old_tracks = 0_u32;
    let mut new_tracks = 0_u32;
    for (index, track) in current.tracks.iter().enumerate() {
        match candidate
            .tracks
            .iter()
            .find(|candidate| candidate.id == track.id)
        {
            Some(candidate) if same_track_structure(track, candidate) => {}
            _ => old_tracks |= 1_u32 << index,
        }
    }
    for (index, track) in candidate.tracks.iter().enumerate() {
        match current.tracks.iter().find(|current| current.id == track.id) {
            Some(current) if same_track_structure(current, track) => {}
            _ => new_tracks |= 1_u32 << index,
        }
    }

    let mut old_buses = 0_u16;
    let mut new_buses = 0_u16;
    for (index, bus) in current.buses.iter().enumerate() {
        match candidate
            .buses
            .iter()
            .find(|candidate| candidate.id == bus.id)
        {
            Some(candidate) if same_bus_structure(bus, candidate) => {}
            _ => old_buses |= 1_u16 << (index + 1),
        }
    }
    for (index, bus) in candidate.buses.iter().enumerate() {
        match current.buses.iter().find(|current| current.id == bus.id) {
            Some(current) if same_bus_structure(current, bus) => {}
            _ => new_buses |= 1_u16 << (index + 1),
        }
    }

    let master_changed = current.master.id != candidate.master.id
        || !same_device_chain(&current.master.inserts, &candidate.master.inserts);
    (
        EngineFade {
            tracks: old_tracks,
            buses: old_buses,
            master: master_changed,
            direction: FadeDirection::Out,
        },
        EngineFade {
            tracks: new_tracks,
            buses: new_buses,
            master: master_changed,
            direction: FadeDirection::In,
        },
    )
}

fn same_track_structure(current: &model::Track, candidate: &model::Track) -> bool {
    current.instrument.id == candidate.instrument.id
        && current
            .instrument
            .same_structural_identity(&candidate.instrument)
        && current.source.same_identity(&candidate.source)
        && same_device_chain(&current.inserts, &candidate.inserts)
        && same_route(&current.output, &candidate.output)
        && same_routes(&current.sends, &candidate.sends)
}

fn same_bus_structure(current: &model::Bus, candidate: &model::Bus) -> bool {
    same_device_chain(&current.inserts, &candidate.inserts)
        && match (&current.output, &candidate.output) {
            (Some(a), Some(b)) => same_route(a, b),
            (None, None) => true,
            _ => false,
        }
        && same_routes(&current.sends, &candidate.sends)
}

/// The same connection: the candidate engine's own route carries its level, so
/// a level change alone needs no fade.
fn same_route(current: &model::Route, candidate: &model::Route) -> bool {
    current.id == candidate.id && current.pre == candidate.pre && current.to == candidate.to
}

fn same_device_chain(current: &[model::Device], candidate: &[model::Device]) -> bool {
    current.len() == candidate.len()
        && current.iter().zip(candidate).all(|(current, candidate)| {
            current.id == candidate.id && current.same_structural_identity(candidate)
        })
}

fn same_routes(current: &[model::Route], candidate: &[model::Route]) -> bool {
    current.len() == candidate.len()
        && current.iter().all(|route| {
            candidate
                .iter()
                .any(|candidate| same_route(route, candidate))
        })
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ValueTransactionPrepareError {
    #[error("this revision needs an explicit audio configuration")]
    ConfigurationRequired,
    #[error("invalid revision: {0}")]
    Validation(String),
    #[error("invalid sampler coverage: {0}")]
    SampleCoverage(String),
    #[error(transparent)]
    Reconcile(#[from] ReconcileError),
    #[error("reconcile plan does not describe the supplied current and candidate sessions")]
    PlanDoesNotMatchSessions,
    #[error("value transaction revision overflowed")]
    RevisionOverflow,
    #[error(
        "reconcile operation {operation_index} is structural and cannot enter a value transaction"
    )]
    StructuralOperation { operation_index: usize },
    #[error("value transaction contains more than {maximum} prepared operations")]
    TooManyPreparedOperations { maximum: usize },
    #[error("device `{0}` is missing from a value-only transaction")]
    MissingDevice(model::Id),
    #[error("device `{0}` changed identity in a value-only transaction")]
    DeviceIdentityChanged(model::Id),
    #[error("parameter `{parameter}` was removed from device `{device_id}`")]
    ParameterRemoval {
        device_id: model::Id,
        parameter: String,
    },
    #[error("candidate device `{device_id}` has no parameter `{parameter}`")]
    MissingCandidateParameter {
        device_id: model::Id,
        parameter: String,
    },
}

#[derive(Debug, Error)]
pub enum StructuralTransactionPrepareError {
    #[error(transparent)]
    Reconcile(#[from] ReconcileError),
    #[error("reconcile plan does not describe the supplied current and candidate sessions")]
    PlanDoesNotMatchSessions,
    #[error("structural transaction revision overflowed")]
    RevisionOverflow,
    #[error("reconcile plan has no structural operation")]
    PlanHasNoStructuralOperations,
    #[error(transparent)]
    Engine(#[from] EngineError),
}

#[derive(Debug, Error)]
pub enum TransactionPrepareError {
    #[error(transparent)]
    Device(DeviceError),
    #[error(transparent)]
    Value(ValueTransactionPrepareError),
    #[error(transparent)]
    Structural(StructuralTransactionPrepareError),
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ValueTransactionApplyError {
    #[error("value transaction was already applied")]
    AlreadyApplied,
    #[error("prepared value transaction no longer matches the runtime graph")]
    RuntimeMismatch,
    #[error(transparent)]
    Device(#[from] DeviceError),
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TransactionApplyError {
    #[error(transparent)]
    Value(ValueTransactionApplyError),
    #[error(transparent)]
    Structural(StructuralTransactionApplyError),
}

pub struct TransactionReceipt {
    pub revision: u64,
    pub observed_generation: u64,
    pub committed_at: Instant,
    pub sample_position: u64,
    pub transport: TransportSnapshot,
    pub callback_count: u64,
    pub render_faults: u64,
    pub stream_errors: u64,
    pub transaction_commits: u64,
    pub transaction_faults: u64,
    pub structural: bool,
    pub faded: bool,
    pub committed: bool,
    pub failure: Option<TransactionApplyError>,
    pub transaction: Box<PreparedTransaction>,
}

impl fmt::Debug for TransactionReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransactionReceipt")
            .field("revision", &self.revision)
            .field("observed_generation", &self.observed_generation)
            .field("committed_at", &self.committed_at)
            .field("sample_position", &self.sample_position)
            .field("transport", &self.transport)
            .field("callback_count", &self.callback_count)
            .field("render_faults", &self.render_faults)
            .field("stream_errors", &self.stream_errors)
            .field("transaction_commits", &self.transaction_commits)
            .field("transaction_faults", &self.transaction_faults)
            .field("structural", &self.structural)
            .field("faded", &self.faded)
            .field("committed", &self.committed)
            .field("failure", &self.failure)
            .field("transaction", &self.transaction)
            .finish()
    }
}

pub type ValueTransactionReceipt = TransactionReceipt;

#[cfg(test)]
mod runtime_policy_tests {
    use super::*;

    fn compile(source: &str) -> model::Session {
        let value = crate::lang::Evaluator::new().source(source).unwrap();
        crate::compile::lower(
            value.get("__result").unwrap().clone(),
            std::path::Path::new("runtime-policy.muz"),
            vec![],
        )
        .unwrap()
        .session
    }

    fn config() -> AudioConfig {
        AudioConfig {
            sample_rate: 48_000.0,
            max_frames: 64,
            offline: true,
        }
    }

    fn prepare(current: &model::Session, candidate: &model::Session) -> PreparedTransaction {
        let plan = plan_reconciliation(0, current, candidate).unwrap();
        PreparedTransaction::prepare(current, candidate, &plan, 7, None, config()).unwrap()
    }

    #[test]
    fn value_schedule_revision_preserves_valid_loop_and_paused_position() {
        let current = compile(
            r#"song({tracks:[track("x",note(60,4b),synth("pad"))],tail:0})"#,
        );
        let candidate = compile(
            r#"song({tracks:[track("x",note(60,3b),synth("pad"))],tail:0})"#,
        );
        let range = Some((96_000, 192_000));
        let mut engine = AudioEngine::new(&current, config()).unwrap();
        engine.set_running(false);
        engine.set_loop(range);
        engine.seek_ticks(144_000);
        let before = engine.status().transport;
        let mut transaction = prepare(&current, &candidate);
        assert!(!transaction.is_structural());
        transaction
            .prepare_runtime_policy(&current, &candidate, range, 0, None)
            .unwrap();
        transaction.apply(&mut engine).unwrap();
        assert_eq!(engine.loop_range(), range);
        assert_eq!(engine.status().transport.current_tick, before.current_tick);
        assert_eq!(engine.status().transport.running, before.running);
        assert_eq!(engine.revision(), 1);
    }

    #[test]
    fn shortened_value_revision_clears_loop_instead_of_reenabling_old_extent() {
        let current = compile(
            r#"song({tracks:[track("x",note(60,4b),synth("pad"))],tail:0})"#,
        );
        let candidate = compile(
            r#"song({tracks:[track("x",note(60,1b),synth("pad"))],tail:0})"#,
        );
        let range = Some((960_000, 1_920_000));
        let mut engine = AudioEngine::new(&current, config()).unwrap();
        engine.set_loop(range);
        let mut transaction = prepare(&current, &candidate);
        assert!(!transaction.is_structural());
        transaction
            .prepare_runtime_policy(&current, &candidate, range, 0, None)
            .unwrap();
        transaction.apply(&mut engine).unwrap();
        assert_eq!(engine.loop_range(), None);
        assert!(engine.status().current_tick <= engine.status().end_tick as f64);
    }

    #[test]
    fn structural_mask_intersects_ids_and_keeps_new_tracks_silent() {
        let current = compile(
            r#"song({tracks:[track("keep",note(60,4b),voice_patch("p",{nodes:[{id:"level",op:"param",value:0.2,min:0,max:1}],output:"level"})),track("removed",note(60,4b),synth("pad"))],tail:0})"#,
        );
        let candidate = compile(
            r#"song({tracks:[track("keep",note(60,4b),voice_patch("p",{nodes:[{id:"level",op:"param",value:0.2,min:0,max:1}],output:"level"})),track("new",note(60,4b),voice_patch("p",{nodes:[{id:"level",op:"param",value:0.4,min:0,max:1}],output:"level"}))],tail:0})"#,
        );
        let audible = ["keep".to_owned(), "removed".to_owned()];
        let mut transaction = prepare(&current, &candidate);
        assert!(transaction.is_structural());
        transaction
            .prepare_runtime_policy(&current, &candidate, None, 0, Some(&audible))
            .unwrap();
        let mut engine = AudioEngine::new(&current, config()).unwrap();
        engine.set_running(true);
        transaction.apply(&mut engine).unwrap();
        let mut expected = AudioEngine::new(&candidate, config()).unwrap();
        expected
            .set_track_audibility(&["keep".to_owned()], false)
            .unwrap();
        expected.set_running(true);
        let mut actual_pcm = [0.0; 128];
        let mut expected_pcm = [0.0; 128];
        engine.render_interleaved(&mut actual_pcm, 2).unwrap();
        expected.render_interleaved(&mut expected_pcm, 2).unwrap();
        assert!(expected_pcm.iter().any(|sample| *sample != 0.0));
        assert_eq!(actual_pcm, expected_pcm);
    }

    #[test]
    fn structural_empty_mask_silences_output() {
        let current = compile(
            r#"song({tracks:[track("x",note(60,4b),voice_patch("p",{nodes:[{id:"level",op:"param",value:0.2,min:0,max:1}],output:"level"}))],tail:0})"#,
        );
        let mut candidate = current.clone();
        candidate.tracks[0].instrument.generation += 1;
        let mut transaction = prepare(&current, &candidate);
        transaction
            .prepare_runtime_policy(&current, &candidate, None, 0, Some(&[]))
            .unwrap();
        let mut engine = AudioEngine::new(&current, config()).unwrap();
        engine.set_running(true);
        transaction.apply(&mut engine).unwrap();
        let mut pcm = [0.0; 128];
        engine.render_interleaved(&mut pcm, 2).unwrap();
        assert_eq!(pcm, [0.0; 128]);
    }

    #[test]
    fn preparation_rejects_mismatched_descriptions_without_changing_transaction() {
        let current = compile(
            r#"song({tracks:[track("x",note(60,4b),synth("pad"))],tail:0})"#,
        );
        let mut candidate = current.clone();
        candidate.extras.title = "accepted candidate".into();
        let mut wrong = candidate.clone();
        wrong.extras.title = "different candidate".into();
        let mut transaction = prepare(&current, &candidate);
        assert!(
            transaction
                .prepare_runtime_policy(&current, &wrong, None, 0, None)
                .is_err()
        );
        let mut engine = AudioEngine::new(&current, config()).unwrap();
        assert!(transaction.matches_runtime(&engine));
        transaction.apply(&mut engine).unwrap();
        assert_eq!(engine.description_signature, crate::snapshot::signature(&candidate).unwrap());
    }

    #[test]
    fn prefade_guard_rejects_wrong_signature_configuration_and_already_applied() {
        let current = compile(
            r#"song({tracks:[track("x",note(60,4b),synth("pad"))],tail:0})"#,
        );
        let mut candidate = current.clone();
        candidate.tracks[0].instrument.generation += 1;
        let mut transaction = prepare(&current, &candidate);
        let mut wrong = current.clone();
        wrong.extras.title = "different baseline".into();
        assert!(!transaction.matches_runtime(&AudioEngine::new(&wrong, config()).unwrap()));
        let mut wrong_config = config();
        wrong_config.sample_rate = 44_100.0;
        assert!(!transaction.matches_runtime(
            &AudioEngine::new(&current, wrong_config).unwrap()
        ));
        let mut engine = AudioEngine::new(&current, config()).unwrap();
        assert!(transaction.matches_runtime(&engine));
        transaction.apply(&mut engine).unwrap();
        assert!(!transaction.matches_runtime(&engine));
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn observation_cutover_keeps_retired_capture_in_transaction() {
        let current = compile(
            r#"song({tracks:[track("x",note(60,4b),synth("pad"))],tail:0})"#,
        );
        let selection = crate::native::ObservationSelection {
            meter_tracks: vec!["x".into()],
            master_meter: true,
            analysis: None,
            meter_hz: 20,
            analysis_hz: 0,
        };
        for structural in [false, true] {
            for enabled in [false, true] {
                let mut candidate = current.clone();
                if structural {
                    candidate.tracks[0].instrument.generation += 1;
                } else {
                    candidate.extras.title = "new title".into();
                }
                let mut engine = AudioEngine::new(&current, config()).unwrap();
                let (previous, _previous_consumer) =
                    PreparedObservation::new(&current, config(), &selection).unwrap();
                assert!(engine.replace_observation(Some(previous)).is_none());
                let mut transaction = prepare(&current, &candidate);
                transaction
                    .prepare_runtime_policy(&current, &candidate, None, 0, None)
                    .unwrap();
                let replacement = if enabled {
                    let (prepared, consumer) =
                        PreparedObservation::new(&candidate, config(), &selection).unwrap();
                    Some((prepared, consumer))
                } else {
                    None
                };
                let (prepared, _consumer) = match replacement {
                    Some((prepared, consumer)) => (Some(prepared), Some(consumer)),
                    None => (None, None),
                };
                assert!(transaction.replace_observation(prepared).is_none());
                transaction.apply(&mut engine).unwrap();
                assert_eq!(engine.replace_observation(None).is_some(), enabled);
                match &mut transaction {
                    PreparedTransaction::Value(transaction) => {
                        assert!(transaction.runtime_observation.as_ref().unwrap().is_some());
                    }
                    PreparedTransaction::Structural(transaction) => {
                        assert!(
                            transaction.candidate_engine.replace_observation(None).is_some()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn structural_loop_policy_keeps_valid_range_and_clears_shortened_range() {
        let current = compile(
            r#"song({tracks:[track("x",note(60,4b),synth("pad"))],tail:0})"#,
        );
        let mut candidate = compile(
            r#"song({tracks:[track("x",note(60,1b),synth("pad"))],tail:0})"#,
        );
        candidate.tracks[0].instrument.generation += 1;
        for (range, expected) in [
            (Some((96_000, 192_000)), Some((96_000, 192_000))),
            (Some((960_000, 1_920_000)), None),
        ] {
            let mut engine = AudioEngine::new(&current, config()).unwrap();
            engine.set_running(false);
            engine.set_loop(range);
            let mut transaction = prepare(&current, &candidate);
            assert!(transaction.is_structural());
            transaction
                .prepare_runtime_policy(&current, &candidate, range, 0, None)
                .unwrap();
            transaction.apply(&mut engine).unwrap();
            assert_eq!(engine.loop_range(), expected);
            assert!(!engine.status().transport.running);
        }
    }
}
