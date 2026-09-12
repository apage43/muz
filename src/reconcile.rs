use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{
    self, Bus, Device, Id, MAX_TRANSACTION_OPS, Pattern, Route, Session, Track, TrackSource,
    Transport,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntityType {
    Track,
    Bus,
    Device,
    Pattern,
    Route,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntityCategory {
    Track,
    Master,
    Bus,
    Pattern,
    Instrument,
    Insert,
    Output,
    Send,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Entity {
    Track(Track),
    Bus(Bus),
    Device(Device),
    Pattern(Pattern),
    Route(Route),
}

impl Entity {
    pub fn id(&self) -> &Id {
        match self {
            Self::Track(value) => &value.id,
            Self::Bus(value) => &value.id,
            Self::Device(value) => &value.id,
            Self::Pattern(value) => &value.id,
            Self::Route(value) => &value.id,
        }
    }

    pub fn entity_type(&self) -> EntityType {
        match self {
            Self::Track(_) => EntityType::Track,
            Self::Bus(_) => EntityType::Bus,
            Self::Device(_) => EntityType::Device,
            Self::Pattern(_) => EntityType::Pattern,
            Self::Route(_) => EntityType::Route,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRef {
    pub id: Id,
    pub entity_type: EntityType,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocatedEntity {
    pub entity: Entity,
    pub parent: Option<Id>,
    pub category: EntityCategory,
    pub order: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReconcileOperation {
    UpdateExtras,
    Add {
        entity: LocatedEntity,
    },
    Remove {
        entity: LocatedEntity,
    },
    Replace {
        entity: LocatedEntity,
    },
    Rename {
        entity: EntityRef,
        name: String,
    },
    Move {
        entity: EntityRef,
        order: u32,
    },
    Reparent {
        entity: EntityRef,
        parent: Option<Id>,
        category: EntityCategory,
        order: u32,
    },
    SetParameters {
        device_id: Id,
        deltas: BTreeMap<String, Option<f32>>,
    },
    ReplacePattern {
        pattern: Pattern,
    },
    ReplaceTrackSource {
        track_id: Id,
        source: TrackSource,
    },
    UpdateRoute {
        route: Route,
    },
    UpdateTransport {
        transport: Transport,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReconcilePlan {
    pub base_revision: u64,
    pub operations: Vec<ReconcileOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ReconcileError {
    #[error("duplicate entity id `{id}` in {session} session")]
    DuplicateEntityId { session: &'static str, id: Id },
    #[error("reconciliation needs {actual} operations, exceeding the limit of {maximum}")]
    TooManyOperations { actual: usize, maximum: usize },
}

#[derive(Clone, Copy)]
struct FlatEntity<'a> {
    entity: FlatEntityValue<'a>,
    parent: Option<&'a Id>,
    category: EntityCategory,
    order: u32,
}

#[derive(Clone, Copy)]
enum FlatEntityValue<'a> {
    Track(&'a Track),
    Bus(&'a Bus),
    Device(&'a Device),
    Pattern(&'a Pattern),
    Route(&'a Route),
}

impl<'a> FlatEntityValue<'a> {
    fn id(self) -> &'a Id {
        match self {
            Self::Track(value) => &value.id,
            Self::Bus(value) => &value.id,
            Self::Device(value) => &value.id,
            Self::Pattern(value) => &value.id,
            Self::Route(value) => &value.id,
        }
    }

    fn entity_type(self) -> EntityType {
        match self {
            Self::Track(_) => EntityType::Track,
            Self::Bus(_) => EntityType::Bus,
            Self::Device(_) => EntityType::Device,
            Self::Pattern(_) => EntityType::Pattern,
            Self::Route(_) => EntityType::Route,
        }
    }

    fn to_owned(self) -> Entity {
        match self {
            Self::Track(value) => Entity::Track(value.clone()),
            Self::Bus(value) => Entity::Bus(value.clone()),
            Self::Device(value) => Entity::Device(value.clone()),
            Self::Pattern(value) => Entity::Pattern(value.clone()),
            Self::Route(value) => Entity::Route(value.clone()),
        }
    }
}

impl FlatEntity<'_> {
    fn entity_ref(self) -> EntityRef {
        EntityRef {
            id: self.entity.id().clone(),
            entity_type: self.entity.entity_type(),
        }
    }

    fn to_owned(self) -> LocatedEntity {
        LocatedEntity {
            entity: self.entity.to_owned(),
            parent: self.parent.cloned(),
            category: self.category,
            order: self.order,
        }
    }
}

fn push_bus<'a>(
    entities: &mut Vec<FlatEntity<'a>>,
    bus: &'a Bus,
    category: EntityCategory,
    order: u32,
) {
    entities.push(FlatEntity {
        entity: FlatEntityValue::Bus(bus),
        parent: None,
        category,
        order,
    });
    for (index, device) in bus.inserts.iter().enumerate() {
        entities.push(FlatEntity {
            entity: FlatEntityValue::Device(device),
            parent: Some(&bus.id),
            category: EntityCategory::Insert,
            order: index as u32,
        });
    }
    if let Some(route) = &bus.output {
        entities.push(FlatEntity {
            entity: FlatEntityValue::Route(route),
            parent: Some(&bus.id),
            category: EntityCategory::Output,
            order: 0,
        });
    }
    for (index, route) in bus.sends.iter().enumerate() {
        entities.push(FlatEntity {
            entity: FlatEntityValue::Route(route),
            parent: Some(&bus.id),
            category: EntityCategory::Send,
            order: index as u32,
        });
    }
}

fn flatten(session: &Session) -> Vec<FlatEntity<'_>> {
    let mut entities = Vec::new();
    push_bus(&mut entities, &session.master, EntityCategory::Master, 0);
    for (index, bus) in session.buses.iter().enumerate() {
        push_bus(&mut entities, bus, EntityCategory::Bus, index as u32);
    }
    for (track_index, track) in session.tracks.iter().enumerate() {
        entities.push(FlatEntity {
            entity: FlatEntityValue::Track(track),
            parent: None,
            category: EntityCategory::Track,
            order: track_index as u32,
        });
        if let TrackSource::Pattern(pattern) = &track.source {
            entities.push(FlatEntity {
                entity: FlatEntityValue::Pattern(pattern),
                parent: Some(&track.id),
                category: EntityCategory::Pattern,
                order: 0,
            });
        }
        entities.push(FlatEntity {
            entity: FlatEntityValue::Device(&track.instrument),
            parent: Some(&track.id),
            category: EntityCategory::Instrument,
            order: 0,
        });
        for (index, device) in track.inserts.iter().enumerate() {
            entities.push(FlatEntity {
                entity: FlatEntityValue::Device(device),
                parent: Some(&track.id),
                category: EntityCategory::Insert,
                order: index as u32,
            });
        }
        entities.push(FlatEntity {
            entity: FlatEntityValue::Route(&track.output),
            parent: Some(&track.id),
            category: EntityCategory::Output,
            order: 0,
        });
        for (index, route) in track.sends.iter().enumerate() {
            entities.push(FlatEntity {
                entity: FlatEntityValue::Route(route),
                parent: Some(&track.id),
                category: EntityCategory::Send,
                order: index as u32,
            });
        }
    }
    entities
}

fn index_by_id<'a>(
    entities: &[FlatEntity<'a>],
    session: &'static str,
) -> Result<BTreeMap<Id, usize>, ReconcileError> {
    let mut index = BTreeMap::new();
    for (position, entity) in entities.iter().enumerate() {
        let id = entity.entity.id().clone();
        if index.insert(id.clone(), position).is_some() {
            return Err(ReconcileError::DuplicateEntityId { session, id });
        }
    }
    Ok(index)
}

fn needs_replacement(old: FlatEntity<'_>, new: FlatEntity<'_>) -> bool {
    match (old.entity, new.entity) {
        (FlatEntityValue::Track(old), FlatEntityValue::Track(new)) => {
            !old.source.same_identity(&new.source)
        }
        (FlatEntityValue::Device(old), FlatEntityValue::Device(new)) => {
            !old.same_structural_identity(new)
                || old.kind.port_signature() != new.kind.port_signature()
        }
        (old, new) => old.entity_type() != new.entity_type(),
    }
}

fn parameter_deltas(
    old: &BTreeMap<String, f32>,
    new: &BTreeMap<String, f32>,
) -> BTreeMap<String, Option<f32>> {
    let names: BTreeSet<_> = old.keys().chain(new.keys()).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let old_value = old.get(name);
            let new_value = new.get(name);
            (old_value != new_value).then(|| (name.clone(), new_value.copied()))
        })
        .collect()
}

pub fn plan_reconciliation(
    base_revision: u64,
    old: &model::Session,
    new: &model::Session,
) -> Result<ReconcilePlan, ReconcileError> {
    let old_entities = flatten(old);
    let new_entities = flatten(new);
    let old_index = index_by_id(&old_entities, "old")?;
    let new_index = index_by_id(&new_entities, "new")?;
    let mut operations = Vec::new();

    // Children are declared after their parents, so reverse traversal removes them first.
    for old_entity in old_entities.iter().rev().copied() {
        if !new_index.contains_key(old_entity.entity.id()) {
            operations.push(ReconcileOperation::Remove {
                entity: old_entity.to_owned(),
            });
        }
    }

    for new_entity in new_entities.iter().copied() {
        let Some(&old_position) = old_index.get(new_entity.entity.id()) else {
            continue;
        };
        let old_entity = old_entities[old_position];
        if needs_replacement(old_entity, new_entity) {
            operations.push(ReconcileOperation::Replace {
                entity: new_entity.to_owned(),
            });
        }
    }

    // Parents are declared before their children, so forward traversal adds them first.
    for new_entity in new_entities.iter().copied() {
        if !old_index.contains_key(new_entity.entity.id()) {
            operations.push(ReconcileOperation::Add {
                entity: new_entity.to_owned(),
            });
        }
    }

    for new_entity in new_entities.iter().copied() {
        let Some(&old_position) = old_index.get(new_entity.entity.id()) else {
            continue;
        };
        let old_entity = old_entities[old_position];
        if needs_replacement(old_entity, new_entity) {
            continue;
        }

        if old_entity.parent != new_entity.parent || old_entity.category != new_entity.category {
            operations.push(ReconcileOperation::Reparent {
                entity: new_entity.entity_ref(),
                parent: new_entity.parent.cloned(),
                category: new_entity.category,
                order: new_entity.order,
            });
        } else if old_entity.order != new_entity.order {
            operations.push(ReconcileOperation::Move {
                entity: new_entity.entity_ref(),
                order: new_entity.order,
            });
        }

        match (old_entity.entity, new_entity.entity) {
            (FlatEntityValue::Track(old), FlatEntityValue::Track(new)) => {
                if old.name != new.name {
                    operations.push(ReconcileOperation::Rename {
                        entity: new_entity.entity_ref(),
                        name: new.name.clone(),
                    });
                }
                if matches!(
                    (&old.source, &new.source),
                    (TrackSource::Midi(_), TrackSource::Midi(_))
                ) && old.source != new.source
                {
                    operations.push(ReconcileOperation::ReplaceTrackSource {
                        track_id: new.id.clone(),
                        source: new.source.clone(),
                    });
                }
            }
            (FlatEntityValue::Bus(old), FlatEntityValue::Bus(new)) => {
                if old.name != new.name {
                    operations.push(ReconcileOperation::Rename {
                        entity: new_entity.entity_ref(),
                        name: new.name.clone(),
                    });
                }
            }
            (FlatEntityValue::Device(old), FlatEntityValue::Device(new)) => {
                let deltas = parameter_deltas(&old.control_values(), &new.control_values());
                if !deltas.is_empty() {
                    operations.push(ReconcileOperation::SetParameters {
                        device_id: new.id.clone(),
                        deltas,
                    });
                }
            }
            (FlatEntityValue::Pattern(old), FlatEntityValue::Pattern(new)) => {
                if old.notes != new.notes {
                    operations.push(ReconcileOperation::ReplacePattern {
                        pattern: new.clone(),
                    });
                }
            }
            (FlatEntityValue::Route(old), FlatEntityValue::Route(new)) => {
                if old.to != new.to || old.gain_db != new.gain_db {
                    operations.push(ReconcileOperation::UpdateRoute { route: new.clone() });
                }
            }
            _ => unreachable!("entity type changes are handled as replacements"),
        }
    }

    if old.extras != new.extras {
        operations.push(ReconcileOperation::UpdateExtras);
    }
    if old.transport != new.transport {
        operations.push(ReconcileOperation::UpdateTransport {
            transport: new.transport.clone(),
        });
    }

    if operations.len() > MAX_TRANSACTION_OPS {
        return Err(ReconcileError::TooManyOperations {
            actual: operations.len(),
            maximum: MAX_TRANSACTION_OPS,
        });
    }

    Ok(ReconcilePlan {
        base_revision,
        operations,
    })
}
