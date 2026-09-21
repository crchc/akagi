//! Bounded in-process broadcast buses shared by capture, analysis, bots, and
//! the Web SSE stream. Slow consumers may lag without blocking producers.

use crate::analysis::result::AnalysisResult;
use crate::bot::BotResponse;
use crate::config::AppConfig;
use crate::schema::{BotStatus, CaptureStatus, HistoryEvent, MjaiEvent, Notification};
use tokio::sync::broadcast;

/// Fan-out for `MjaiEvent`s from platform bridges.
pub type MjaiBus = broadcast::Sender<MjaiEvent>;

/// Fan-out for `BotResponse`s from the active bot.
pub type BotResponseBus = broadcast::Sender<BotResponse>;

/// Fan-out for `BotStatus` lifecycle transitions.
pub type BotStatusBus = broadcast::Sender<BotStatus>;

/// Fan-out for `CaptureStatus` lifecycle transitions.
pub type CaptureStatusBus = broadcast::Sender<CaptureStatus>;

/// Fan-out for transient `Notification`s pushed at the user.
pub type NotifyBus = broadcast::Sender<Notification>;

/// Fan-out for saved configuration changes, including the live game count.
pub type ConfigBus = broadcast::Sender<AppConfig>;

/// Fan-out for `AnalysisResult`s produced after each game-state update.
pub type AnalysisBus = broadcast::Sender<AnalysisResult>;

/// One `MjaiEvent` as re-emitted after the `GameTracker` applied it.
///
/// `can_act` rides with the event rather than being read off the tracker
/// on arrival, and that is the whole point of the type: a subscriber that
/// pauses between events (the bot manager waits on inference) would
/// otherwise ask a tracker that has moved on, and get the answer for a
/// later event than the one it is holding. One frame can carry several
/// seats' actions, so that is not a rare race — it is most of them.
#[derive(Debug, Clone)]
pub struct TrackedEvent {
    pub event: MjaiEvent,
    /// Whether the riichi engine offers our seat a choice in the state this
    /// event produced — its own turn, or a claim on someone's discard.
    ///
    /// `None` when the engine has no opinion to give: no game in progress,
    /// or no seat tagged (observer / replay). Consumers treat that as "no
    /// opinion" and fall back to their own policy rather than going silent.
    pub can_act: Option<bool>,
}

/// Post-tracker fan-out: each event re-emitted *after* the `GameTracker`
/// has applied it to the engine state, carrying what the engine then had
/// to say about our seat. Subscribers can rely on the live game-state
/// mirror being current when this fires (vs. the raw `MjaiBus` where
/// ordering against the tracker is racy).
pub type PostTrackerBus = broadcast::Sender<TrackedEvent>;

/// Fan-out for game-history additions and deletions.
pub type HistoryBus = broadcast::Sender<HistoryEvent>;

/// Default capacity. Live pacing produces ~1 second of mjai events at a time
/// (start_kyoku + 13 tehai + a few tsumo/dahai pairs), which is tiny. The
/// sizing constraint is the **one-shot GameRestore replay**: on reconnect the
/// bridge emits an entire kyoku's events in a single synchronous burst (the
/// CDP send loop does not yield between `send`s), and consumers treat overflow
/// as `Lagged → skip`. A skipped mid-hand `dahai`/`pon` would silently corrupt
/// the game-state tracker with no self-heal until the next kyoku, so the buffer
/// must comfortably exceed a worst-case full-kyoku event count (~a few hundred).
pub const DEFAULT_CAPACITY: usize = 1024;

/// Smaller buffer for status / notification streams — these are bursty
/// but low-rate; 64 is plenty.
pub const STATUS_CAPACITY: usize = 64;

pub fn mjai_bus() -> MjaiBus {
    // Drop the placeholder receiver — real consumers subscribe later via
    // `Sender::subscribe`. The sender stays alive as long as anyone holds
    // a clone of it.
    let (tx, _rx) = broadcast::channel(DEFAULT_CAPACITY);
    tx
}

pub fn bot_response_bus() -> BotResponseBus {
    let (tx, _rx) = broadcast::channel(DEFAULT_CAPACITY);
    tx
}

pub fn bot_status_bus() -> BotStatusBus {
    let (tx, _rx) = broadcast::channel(STATUS_CAPACITY);
    tx
}

pub fn capture_status_bus() -> CaptureStatusBus {
    let (tx, _rx) = broadcast::channel(STATUS_CAPACITY);
    tx
}

pub fn notify_bus() -> NotifyBus {
    let (tx, _rx) = broadcast::channel(STATUS_CAPACITY);
    tx
}

pub fn config_bus() -> ConfigBus {
    let (tx, _rx) = broadcast::channel(STATUS_CAPACITY);
    tx
}

pub fn analysis_bus() -> AnalysisBus {
    let (tx, _rx) = broadcast::channel(DEFAULT_CAPACITY);
    tx
}

pub fn post_tracker_bus() -> PostTrackerBus {
    let (tx, _rx) = broadcast::channel(DEFAULT_CAPACITY);
    tx
}

pub fn history_bus() -> HistoryBus {
    let (tx, _rx) = broadcast::channel(STATUS_CAPACITY);
    tx
}
