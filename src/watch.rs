use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;

const MAX_WATCH_TARGETS: usize = 512;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WatchBatch {
    /// When the first relevant event in this batch reached the notify callback.
    pub first_event: Instant,
    /// Number of relevant notify events coalesced into this batch.
    pub event_count: usize,
}

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("could not resolve the current directory for the source path: {0}")]
    CurrentDirectory(#[source] io::Error),
    #[error("source path has no parent directory: {0}")]
    MissingParent(PathBuf),
    #[error("could not create the source watcher: {0}")]
    Create(#[source] notify::Error),
    #[error("could not watch the source parent directory: {0}")]
    Watch(#[source] notify::Error),
    #[error("source watcher reported an error: {0}")]
    Event(#[source] notify::Error),
    #[error("cannot watch {actual} exact paths; maximum is {maximum}")]
    TargetCapacityExceeded { actual: usize, maximum: usize },
    #[error("source watcher event channel disconnected")]
    Disconnected,
}

struct TimedEvent {
    received_at: Instant,
    result: notify::Result<Event>,
}

#[derive(Clone, Copy)]
struct PendingBatch {
    first_event: Instant,
    last_event: Instant,
    event_count: usize,
}

/// Watches bounded exact target paths through their parent directories so replacing
/// or renaming a target inode does not invalidate the watch.
pub struct SourceWatcher {
    targets: Vec<PathBuf>,
    working_directory: PathBuf,
    parents: Vec<PathBuf>,
    quiet_interval: Duration,
    receiver: Receiver<TimedEvent>,
    pending: Option<PendingBatch>,
    completed: Option<WatchBatch>,
    watcher: RecommendedWatcher,
}

impl SourceWatcher {
    /// Starts a non-recursive watch of one source's parent directory.
    pub fn new(source: impl AsRef<Path>, quiet_interval: Duration) -> Result<Self, WatchError> {
        Self::new_many([source.as_ref()], quiet_interval)
    }

    /// Starts non-recursive watches for a bounded set of normalized exact paths.
    pub fn new_many<I, P>(targets: I, quiet_interval: Duration) -> Result<Self, WatchError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let working_directory = std::env::current_dir().map_err(WatchError::CurrentDirectory)?;
        let targets = normalize_targets(&working_directory, targets)?;
        let parents = target_parents(&targets)?;

        let (sender, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |result| {
            let _ = sender.send(TimedEvent {
                received_at: Instant::now(),
                result,
            });
        })
        .map_err(WatchError::Create)?;
        for parent in &parents {
            watcher
                .watch(parent, RecursiveMode::NonRecursive)
                .map_err(WatchError::Watch)?;
        }

        Ok(Self {
            targets,
            working_directory,
            parents,
            quiet_interval,
            receiver,
            pending: None,
            completed: None,
            watcher,
        })
    }

    pub fn source_path(&self) -> &Path {
        &self.targets[0]
    }

    pub fn watched_parents(&self) -> &[PathBuf] {
        &self.parents
    }

    pub fn watched_parent(&self) -> &Path {
        &self.parents[0]
    }

    /// Replaces the exact target set after a candidate has been accepted.
    pub fn replace_targets<I, P>(&mut self, targets: I) -> Result<(), WatchError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let targets = normalize_targets(&self.working_directory, targets)?;
        let parents = target_parents(&targets)?;

        let added: Vec<_> = parents
            .iter()
            .filter(|parent| !self.parents.contains(parent))
            .cloned()
            .collect();
        for (index, parent) in added.iter().enumerate() {
            if let Err(error) = self.watcher.watch(parent, RecursiveMode::NonRecursive) {
                for watched in &added[..index] {
                    let _ = self.watcher.unwatch(watched);
                }
                return Err(WatchError::Watch(error));
            }
        }
        for parent in self
            .parents
            .iter()
            .filter(|parent| !parents.contains(parent))
        {
            let _ = self.watcher.unwatch(parent);
        }
        self.targets = targets;
        self.parents = parents;
        Ok(())
    }
    /// Waits for a completed batch for at most `timeout`.
    ///
    /// A caller timeout does not discard an in-progress batch. A later call
    /// continues waiting for that batch's quiet interval to expire.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<WatchBatch>, WatchError> {
        let started = Instant::now();

        loop {
            self.drain_ready()?;

            if let Some(batch) = self.completed.take() {
                return Ok(Some(batch));
            }

            if self
                .pending
                .is_some_and(|pending| pending.last_event.elapsed() >= self.quiet_interval)
            {
                return Ok(self.take_batch());
            }

            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Ok(None);
            }

            let wait = self.pending.map_or(remaining, |pending| {
                remaining.min(
                    self.quiet_interval
                        .saturating_sub(pending.last_event.elapsed()),
                )
            });

            match self.receiver.recv_timeout(wait) {
                Ok(event) => self.accept(event)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(WatchError::Disconnected),
            }
        }
    }

    fn drain_ready(&mut self) -> Result<(), WatchError> {
        while self.completed.is_none() {
            match self.receiver.try_recv() {
                Ok(event) => self.accept(event)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(WatchError::Disconnected),
            }
        }
        Ok(())
    }

    fn accept(&mut self, timed: TimedEvent) -> Result<(), WatchError> {
        let event = timed.result.map_err(WatchError::Event)?;
        if !event_is_relevant(&event, &self.targets, &self.working_directory) {
            return Ok(());
        }

        if let Some(pending) = self.pending {
            if timed
                .received_at
                .saturating_duration_since(pending.last_event)
                >= self.quiet_interval
            {
                self.completed = Some(WatchBatch {
                    first_event: pending.first_event,
                    event_count: pending.event_count,
                });
                self.pending = None;
            }
        }

        match &mut self.pending {
            Some(pending) => {
                pending.last_event = pending.last_event.max(timed.received_at);
                pending.event_count = pending.event_count.saturating_add(1);
            }
            None => {
                self.pending = Some(PendingBatch {
                    first_event: timed.received_at,
                    last_event: timed.received_at,
                    event_count: 1,
                });
            }
        }
        Ok(())
    }

    fn take_batch(&mut self) -> Option<WatchBatch> {
        self.pending.take().map(|pending| WatchBatch {
            first_event: pending.first_event,
            event_count: pending.event_count,
        })
    }
}

fn event_is_relevant(event: &Event, targets: &[PathBuf], working_directory: &Path) -> bool {
    if !matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return false;
    }

    event.paths.iter().any(|path| {
        let normalized = normalize_from(working_directory, path);
        targets.contains(&normalized)
    })
}

fn normalize_targets<I, P>(working_directory: &Path, targets: I) -> Result<Vec<PathBuf>, WatchError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut normalized = Vec::new();
    for path in targets {
        let path = normalize_from(working_directory, path.as_ref());
        if !normalized.contains(&path) {
            if normalized.len() == MAX_WATCH_TARGETS {
                return Err(WatchError::TargetCapacityExceeded {
                    actual: normalized.len().saturating_add(1),
                    maximum: MAX_WATCH_TARGETS,
                });
            }
            normalized.push(path);
        }
    }
    if normalized.is_empty() {
        return Err(WatchError::TargetCapacityExceeded {
            actual: 0,
            maximum: MAX_WATCH_TARGETS,
        });
    }
    Ok(normalized)
}

fn target_parents(targets: &[PathBuf]) -> Result<Vec<PathBuf>, WatchError> {
    let mut parents = Vec::with_capacity(targets.len());
    for target in targets {
        let parent = target
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| WatchError::MissingParent(target.clone()))?
            .to_path_buf();
        if !parents.contains(&parent) {
            parents.push(parent);
        }
    }
    Ok(parents)
}

fn normalize_from(working_directory: &Path, path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    };
    let mut normalized = PathBuf::new();

    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    normalized
}
