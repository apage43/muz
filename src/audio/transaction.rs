use std::{fmt, time::Instant};

use thiserror::Error;

use crate::{
    model,
    reconcile::{ReconcileError, ReconcileOperation, ReconcilePlan, plan_reconciliation},
};

use super::{
    AudioConfig, AudioEngine, DeviceError, EngineError, TransportSnapshot,
    engine::{
        DeviceRetention, EngineFade, FadeDirection, StructuralTransactionApplyError, TrackRetention,
    },
};

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
    ReplacePattern {
        track: usize,
        expected_pattern_id: model::Id,
        pattern: model::Pattern,
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
    pub(crate) config: Option<AudioConfig>,
    pub(crate) transport: Option<super::transport::RuntimeTransport>,
    revision: u64,
    observed_generation: u64,
    event_started: Instant,
    operations: Vec<PreparedValueOperation>,
    applied: bool,
}

impl PreparedValueTransaction {
    pub fn prepare(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: Instant,
    ) -> Result<Self, ValueTransactionPrepareError> {
        Self::prepare_inner(
            current,
            candidate,
            plan,
            observed_generation,
            event_started,
            None,
        )
    }
    pub fn prepare_with_config(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: Instant,
        config: AudioConfig,
    ) -> Result<Self, ValueTransactionPrepareError> {
        Self::prepare_inner(
            current,
            candidate,
            plan,
            observed_generation,
            event_started,
            Some(config),
        )
    }
    fn prepare_inner(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: Instant,
        config: Option<AudioConfig>,
    ) -> Result<Self, ValueTransactionPrepareError> {
        if requires_structural(current, candidate, plan) {
            return Err(ValueTransactionPrepareError::Validation(
                "revision requires structural preparation".into(),
            ));
        }
        crate::snapshot::validate_events(candidate)
            .map_err(|e| ValueTransactionPrepareError::Validation(e.to_string()))?;
        let schedule_changed = current.transport != candidate.transport
            || current.tracks.iter().zip(&candidate.tracks).any(|(a, b)| {
                matches!(b.source, model::TrackSource::Midi(_)) && a.source != b.source
            });
        let mut schedules = Vec::new();
        if schedule_changed {
            let config = config.ok_or(ValueTransactionPrepareError::ConfigurationRequired)?;
            for (index, track) in candidate.tracks.iter().enumerate() {
                schedules.push((
                    index,
                    track.id.clone(),
                    super::engine::TrackSchedule::prepare(track, candidate, config)
                        .map_err(|e| ValueTransactionPrepareError::Validation(e.to_string()))?,
                ));
            }
        }
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
        candidate
            .validate_sample_coverage()
            .map_err(ValueTransactionPrepareError::SampleCoverage)?;
        let expected = plan_reconciliation(plan.base_revision, current, candidate)?;
        if expected != *plan {
            return Err(ValueTransactionPrepareError::PlanDoesNotMatchSessions);
        }

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
                ReconcileOperation::ReplacePattern { pattern } => {
                    let (track, current_pattern) =
                        find_pattern(current, &pattern.id).ok_or_else(|| {
                            ValueTransactionPrepareError::MissingPattern(pattern.id.clone())
                        })?;
                    let (candidate_track, candidate_pattern) = find_pattern(candidate, &pattern.id)
                        .ok_or_else(|| {
                            ValueTransactionPrepareError::MissingPattern(pattern.id.clone())
                        })?;
                    if track != candidate_track || candidate_pattern != pattern {
                        return Err(ValueTransactionPrepareError::PlanDoesNotMatchSessions);
                    }
                    push_operation(
                        &mut operations,
                        PreparedValueOperation::ReplacePattern {
                            track,
                            expected_pattern_id: current_pattern.id.clone(),
                            pattern: pattern.clone(),
                        },
                    )?;
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
            current_signature: crate::snapshot::signature(current)
                .map_err(|e| ValueTransactionPrepareError::Validation(e.to_string()))?,
            candidate_signature: crate::snapshot::signature(candidate)
                .map_err(|e| ValueTransactionPrepareError::Validation(e.to_string()))?,
            config,
            transport,
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

    pub fn event_started(&self) -> Instant {
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
    event_started: Instant,
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
        event_started: Instant,
        config: AudioConfig,
    ) -> Result<Self, StructuralTransactionPrepareError> {
        let expected = plan_reconciliation(plan.base_revision, current, candidate)?;
        if expected != *plan {
            return Err(StructuralTransactionPrepareError::PlanDoesNotMatchSessions);
        }
        if !requires_structural(current, candidate, plan) {
            return Err(StructuralTransactionPrepareError::PlanHasNoStructuralOperations);
        }
        let revision = plan
            .base_revision
            .checked_add(1)
            .ok_or(StructuralTransactionPrepareError::RevisionOverflow)?;
        let candidate_engine = AudioEngine::new(candidate, config)?;
        let current_signature = crate::snapshot::signature(current)
            .map_err(|e| EngineError::Preflight(e.to_string()))?;
        let device_retentions = prepare_device_retentions(current, candidate);
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

    pub fn event_started(&self) -> Instant {
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
    Value(PreparedValueTransaction),
    Structural(PreparedStructuralTransaction),
}

impl From<PreparedValueTransaction> for PreparedTransaction {
    fn from(transaction: PreparedValueTransaction) -> Self {
        Self::Value(transaction)
    }
}

impl From<PreparedStructuralTransaction> for PreparedTransaction {
    fn from(transaction: PreparedStructuralTransaction) -> Self {
        Self::Structural(transaction)
    }
}

impl PreparedTransaction {
    pub fn prepare(
        current: &model::Session,
        candidate: &model::Session,
        plan: &ReconcilePlan,
        observed_generation: u64,
        event_started: Instant,
        config: AudioConfig,
    ) -> Result<Self, TransactionPrepareError> {
        if requires_structural(current, candidate, plan) {
            PreparedStructuralTransaction::prepare(
                current,
                candidate,
                plan,
                observed_generation,
                event_started,
                config,
            )
            .map(Self::Structural)
            .map_err(TransactionPrepareError::Structural)
        } else {
            PreparedValueTransaction::prepare_with_config(
                current,
                candidate,
                plan,
                observed_generation,
                event_started,
                config,
            )
            .map(Self::Value)
            .map_err(TransactionPrepareError::Value)
        }
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

    pub fn event_started(&self) -> Instant {
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

    pub(crate) fn fade_out(&self) -> Option<EngineFade> {
        match self {
            Self::Structural(transaction) if transaction.needs_fade() => {
                Some(transaction.fade_out())
            }
            _ => None,
        }
    }

    pub(crate) fn fade_in(&self) -> Option<EngineFade> {
        match self {
            Self::Structural(transaction) if transaction.needs_fade() => {
                Some(transaction.fade_in())
            }
            _ => None,
        }
    }

    pub(crate) fn fade_recovery(&self) -> Option<EngineFade> {
        match self {
            Self::Structural(transaction) if transaction.needs_fade() => {
                let mut fade = transaction.fade_out();
                fade.direction = FadeDirection::In;
                Some(fade)
            }
            _ => None,
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
        ReconcileOperation::SetParameters { .. }
            | ReconcileOperation::ReplacePattern { .. }
            | ReconcileOperation::UpdateTransport { .. }
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
    } else if plan.operations.iter().any(|o| {
        matches!(
            o,
            ReconcileOperation::ReplacePattern { .. }
                | ReconcileOperation::ReplaceTrackSource { .. }
                | ReconcileOperation::UpdateTransport { .. }
        )
    }) {
        PreparationEffect::Schedule
    } else if plan
        .operations
        .iter()
        .any(|o| matches!(o, ReconcileOperation::SetParameters { .. }))
    {
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
        ReconcileOperation::UpdateExtras => {
            current.extras.automation != candidate.extras.automation
                || current.extras.tail != candidate.extras.tail
        }
        ReconcileOperation::SetParameters { device_id, .. } => find_device(candidate, device_id)
            .is_none_or(|(_, _, d)| !crate::description::static_controls(d.kind)),
        ReconcileOperation::ReplaceTrackSource { track_id, .. } => {
            let a = current.tracks.iter().find(|t| t.id == *track_id);
            let b = candidate.tracks.iter().find(|t| t.id == *track_id);
            match (a, b) {
                (Some(a), Some(b)) => {
                    !a.source.same_identity(&b.source)
                        || matches!(
                            b.instrument.kind,
                            model::DeviceKind::Vst3
                                | model::DeviceKind::Clap
                                | model::DeviceKind::Rack
                        )
                        || match &b.source {
                            model::TrackSource::Midi(m) => {
                                b.instrument.kind != model::DeviceKind::VoicePatch
                                    && m.imported.notes.iter().any(|n| {
                                        n.performance.is_some_and(|p| p.expression.len > 0)
                                    })
                            }
                            _ => false,
                        }
                }
                _ => true,
            }
        }
        ReconcileOperation::UpdateTransport { .. } => {
            current.transport.mode() != candidate.transport.mode()
        }
        _ => is_structural_operation(op),
    })
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

fn find_pattern<'a>(
    session: &'a model::Session,
    id: &model::Id,
) -> Option<(usize, &'a model::Pattern)> {
    session
        .tracks
        .iter()
        .enumerate()
        .find_map(|(track, source)| {
            source
                .source
                .pattern()
                .filter(|pattern| pattern.id == *id)
                .map(|pattern| (track, pattern))
        })
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
                            model::TrackSource::Pattern(current),
                            model::TrackSource::Pattern(candidate),
                        ) => current.id == candidate.id,
                        (
                            model::TrackSource::Midi(current),
                            model::TrackSource::Midi(candidate),
                        ) => {
                            current.id == candidate.id
                                && current.asset == candidate.asset
                                && current.channel == candidate.channel
                                && current.all_channels == candidate.all_channels
                        }
                        _ => false,
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
        && current.output == candidate.output
        && same_routes(&current.sends, &candidate.sends)
}

fn same_bus_structure(current: &model::Bus, candidate: &model::Bus) -> bool {
    same_device_chain(&current.inserts, &candidate.inserts)
        && current.output == candidate.output
        && same_routes(&current.sends, &candidate.sends)
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
                .any(|candidate| candidate.id == route.id && candidate == route)
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
    #[error("pattern `{0}` is missing from a value-only transaction")]
    MissingPattern(model::Id),
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
