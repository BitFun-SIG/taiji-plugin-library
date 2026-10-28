//! Seam: `event_bus` - Service Definition `RuntimeEventSink`.
//!
//! Contract reference: CONTRACT.md section 2.1 row `event_bus` and section 2.2
//! event layer seam (envelope + bounded fan-out + journal).
//!
//! - The envelope shape follows section 2.2 exactly.
//! - The fan-out channel is a bounded `mpsc` queue, which carries back pressure;
//!   `broadcast` is reserved for observation channels that tolerate frame loss
//!   and never carries truth-source changes.
//! - durable events are journaled; live events may stay in memory.
//! - The consumer name fixed by section 2.1 is `FractalRouter`.

use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use serde::{Deserialize, Serialize};

use crate::error::ContractError;

/// Error code: bounded queue full and back pressure timed out.
pub const E_EVENT_001: &str = "E-EVENT-001";
/// Error code: journal write failed.
pub const E_EVENT_002: &str = "E-EVENT-002";
/// Error code: event envelope field missing.
pub const E_EVENT_003: &str = "E-EVENT-003";

/// Domain tag used on event errors.
pub const DOMAIN_EVENT: &str = "EVENT";

/// Event type enumeration fixed by CONTRACT.md section 2.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RuntimeEventType {
    /// A turn started.
    TurnStarted,
    /// A turn completed.
    TurnCompleted,
    /// A turn failed.
    TurnFailed,
    /// A turn was cancelled.
    TurnCancelled,
    /// A task was assigned.
    TaskAssigned,
    /// A task is being retried.
    TaskRetry,
    /// A task completed.
    TaskCompleted,
    /// A session changed state.
    SessionStateChanged,
}

/// Persistence domain of an event, fixed by CONTRACT.md section 2.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventDomain {
    /// Session level durable fact, still present after reload.
    Durable,
    /// Live connection state.
    Live,
    /// Policy and adapter annotation.
    Capability,
}

impl RuntimeEventType {
    /// Persistence domain of this event type.
    pub fn domain(&self) -> EventDomain {
        match self {
            RuntimeEventType::TurnStarted | RuntimeEventType::TurnCompleted => EventDomain::Live,
            RuntimeEventType::TurnFailed | RuntimeEventType::TurnCancelled => EventDomain::Live,
            RuntimeEventType::TaskAssigned | RuntimeEventType::TaskRetry => EventDomain::Capability,
            RuntimeEventType::TaskCompleted | RuntimeEventType::SessionStateChanged => {
                EventDomain::Durable
            }
        }
    }
}

/// Event envelope fixed by CONTRACT.md section 2.2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEventEnvelope {
    pub session_id: String,
    pub turn_id: Option<String>,
    pub source: Option<String>,
    pub event_type: RuntimeEventType,
    pub payload: serde_json::Value,
}

impl RuntimeEventEnvelope {
    /// Build an envelope, rejecting a missing session identifier.
    pub fn new(
        session_id: impl Into<String>,
        event_type: RuntimeEventType,
        payload: serde_json::Value,
    ) -> Result<Self, ContractError> {
        let session_id = session_id.into();
        if session_id.is_empty() {
            return Err(ContractError::here(
                E_EVENT_003,
                DOMAIN_EVENT,
                "RuntimeEventEnvelope.session_id must be present",
            ));
        }
        Ok(Self {
            session_id,
            turn_id: None,
            source: None,
            event_type,
            payload,
        })
    }

    /// Set the turn identifier.
    pub fn with_turn(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    /// Set the source identifier.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Service Definition for the runtime event seam.
pub trait RuntimeEventSink: Send + Sync {
    /// Publish one runtime event.
    fn publish_runtime_event(&self, event: RuntimeEventEnvelope) -> Result<(), ContractError>;
    /// Number of events published through this sink.
    fn published_count(&self) -> u64;
}

/// Journal writer face used by the event sink.
///
/// The storage seam owns the durable write path; the event seam depends on this
/// trait instead of a concrete storage implementation, so the seams stay
/// decoupled (CONTRACT.md section 0.2).
pub trait JournalSink: Send + Sync {
    /// Append one journal record.
    fn append(&self, event: &RuntimeEventEnvelope) -> Result<(), ContractError>;
}

/// First-phase provider: bounded fan-out queue plus optional journal writer.
///
/// The fan-out receiver is returned to the caller at construction time instead
/// of being held here, so this type stays `Sync` and the consumer side owns the
/// draining end of the bounded channel.
pub struct BoundedEventBus {
    sender: SyncSender<RuntimeEventEnvelope>,
    journal: Option<Box<dyn JournalSink>>,
    published: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for BoundedEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundedEventBus")
            .field("has_journal", &self.journal.is_some())
            .field("published", &self.published_count())
            .finish()
    }
}

impl BoundedEventBus {
    /// Build a bus with the given bounded capacity.
    ///
    /// Returns the bus together with the receiver end of the bounded channel:
    /// the caller drains published events from that receiver.
    pub fn new(capacity: usize) -> (Self, Receiver<RuntimeEventEnvelope>) {
        let (sender, receiver) = sync_channel(capacity.max(1));
        let bus = Self {
            sender,
            journal: None,
            published: std::sync::atomic::AtomicU64::new(0),
        };
        (bus, receiver)
    }

    /// Attach the journal writer used for durable events.
    pub fn with_journal(mut self, journal: Box<dyn JournalSink>) -> Self {
        self.journal = Some(journal);
        self
    }

    /// Drain all currently queued events without blocking.
    pub fn drain(receiver: &Receiver<RuntimeEventEnvelope>) -> Vec<RuntimeEventEnvelope> {
        let mut drained = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            drained.push(event);
        }
        drained
    }
}

impl RuntimeEventSink for BoundedEventBus {
    fn publish_runtime_event(&self, event: RuntimeEventEnvelope) -> Result<(), ContractError> {
        if event.session_id.is_empty() {
            return Err(ContractError::here(
                E_EVENT_003,
                DOMAIN_EVENT,
                "RuntimeEventEnvelope.session_id must be present",
            ));
        }
        if event.event_type.domain() == EventDomain::Durable {
            if let Some(journal) = &self.journal {
                journal.append(&event)?;
            }
        }
        match self.sender.try_send(event) {
            Ok(()) => {
                self.published
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(ContractError::here(
                E_EVENT_001,
                DOMAIN_EVENT,
                "bounded event queue is full; back pressure not accepted",
            )),
            Err(TrySendError::Disconnected(_)) => Err(ContractError::here(
                E_EVENT_002,
                DOMAIN_EVENT,
                "event fan-out receiver is disconnected",
            )),
        }
    }

    fn published_count(&self) -> u64 {
        self.published.load(std::sync::atomic::Ordering::SeqCst)
    }
}
