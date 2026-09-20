//! Weixin iLink bot orchestration for Remote Connect.
//!
//! Provider HTTP/CDN/QR/message parsing lives in `openbitfun-services-integrations`.
//! This module keeps product pairing, command routing, persistence, and agent
//! turn orchestration.

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use log::{debug, error, info, warn};
use openbitfun_services_integrations::remote_connect::bot::weixin as weixin_provider;
use openbitfun_services_integrations::remote_connect::bot::weixin::WeixinProviderClient;
pub use openbitfun_services_integrations::remote_connect::bot::weixin::{
    WeixinConfig, WeixinQrPollResponse, WeixinQrPollStatus, WeixinQrStartResponse,
    MAX_INBOUND_IMAGES, MAX_WEIXIN_FILE_BYTES, WEIXIN_SESSION_EXPIRED_ERRCODE,
};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock};

use super::command_router::{
    complete_im_bot_pairing, current_bot_language, execute_forwarded_turn, handle_command,
    parse_command, welcome_message, BotChatState, BotInteractionHandler, BotInteractiveRequest,
    BotMessageSender, HandleResult,
};
use super::{
    load_bot_persistence, update_bot_persistence, BotConfig, BotRuntimeFence, SavedBotConnection,
};
use crate::agentic::coordination::get_global_coordinator;
use crate::agentic::events::{AgenticEvent, EventSubscriber};
use crate::service::remote_connect::remote_server::ImageAttachment;
use openbitfun_agent_runtime::event_bus::EventSubscriberResult;

const LONG_POLL_TIMEOUT_SECS: u64 = 36;

/// Maximum proactive messages held for one peer before they are merged into a
/// single reply. Older entries are dropped so a peer that never comes back
/// cannot grow the backlog without bound.
const MAX_PENDING_OUTBOUND_PER_PEER: usize = 20;

/// Maximum age of a queued proactive message. Anything older is dropped
/// instead of replayed: the answer it carries is stale by then, and a restart
/// must not push a batch of expired turns.
const PENDING_OUTBOUND_TTL_SECS: i64 = 3600;

/// Separator between merged turn outputs, so one reply built from several
/// answers still reads as several answers.
const PENDING_SEPARATOR: &str = "\n\n---\n\n";

/// How often the outbound worker retries a backlog that no new event woke it
/// for, so a token refreshed by an inbound message is picked up even if the
/// wakeup notification races the enqueue.
const OUTBOUND_RETRY_INTERVAL_SECS: u64 = 5;

/// Bound on the remembered bot-owned turn ids. The set only needs to cover
/// turns that can still complete, so a small window is enough.
const MAX_TRACKED_OWN_TURNS: usize = 256;

/// Reply quota of the WeChat ClawBot channel over one activation window, as
/// the OpenBitFun product copy for this channel records it (`botWeixinRestriction`):
/// after the user sends a message the bot may send at most 10 replies in the
/// following 24 hours, a split long reply spends one reply per part, and the
/// window restarts when the user sends another message.
///
/// This is a product-level channel restriction, not a field the wire protocol
/// documents — `Tencent/openclaw-weixin` only specifies passing the inbound
/// `context_token` back on a reply.
const REPLY_QUOTA_WINDOW_SECS: i64 = 24 * 60 * 60;
const REPLY_QUOTA_PER_WINDOW: usize = 10;

/// Replies this path may spend inside one quota window.
///
/// The quota is shared with the replies the user's own messages receive, and
/// this path cannot observe what the inbound reply path already spent, so it
/// keeps a small share and leaves the rest alone. A proactive answer is never
/// urgent, and three unprompted replies in one window already exceeds what a
/// user expects from a channel they did not just talk to.
const MAX_PROACTIVE_REPLIES_PER_WINDOW: usize = 3;

/// Largest payload one proactive push may merge into. One channel reply is the
/// unit the quota counts, so capping the merge at a single part keeps one push
/// from spending the whole share at once.
const MAX_PROACTIVE_PUSH_BYTES: usize = weixin_provider::MAX_TEXT_CHUNK;

/// The proactive share must stay well clear of the channel quota, so the
/// inbound reply path always keeps room to answer the user.
const _: () = assert!(MAX_PROACTIVE_REPLIES_PER_WINDOW < REPLY_QUOTA_PER_WINDOW);

/// One piece of assistant output waiting for a deliverable context_token.
#[derive(Debug, Clone)]
struct PendingOutbound {
    text: String,
    queued_at: i64,
}

/// One peer's proactive backlog.
#[derive(Debug, Default)]
struct PendingPeer {
    queue: VecDeque<PendingOutbound>,
    /// Reason the last delivery attempt held the backlog back, so a peer that
    /// stays blocked logs once per distinct reason instead of on every retry.
    deferred_reason: Option<&'static str>,
}

/// Drops entries older than [`PENDING_OUTBOUND_TTL_SECS`].
///
/// Entries are appended in time order, so expired ones are always at the
/// front; returns how many were dropped so the caller can report them outside
/// the lock.
fn drop_expired_pending(queue: &mut VecDeque<PendingOutbound>, now: i64) -> usize {
    let mut dropped = 0;
    while let Some(front) = queue.front() {
        if now.saturating_sub(front.queued_at) <= PENDING_OUTBOUND_TTL_SECS {
            break;
        }
        queue.pop_front();
        dropped += 1;
    }
    dropped
}

/// Queues one proactive message, enforcing the retention window and the count
/// cap. Returns how many entries were dropped for logging.
fn push_pending(queue: &mut VecDeque<PendingOutbound>, item: PendingOutbound, now: i64) -> usize {
    let mut dropped = drop_expired_pending(queue, now);
    while queue.len() >= MAX_PENDING_OUTBOUND_PER_PEER {
        queue.pop_front();
        dropped += 1;
    }
    queue.push_back(item);
    dropped
}

fn push_tracked_turn(turns: &mut VecDeque<String>, turn_id: &str) {
    while turns.len() >= MAX_TRACKED_OWN_TURNS {
        turns.pop_front();
    }
    turns.push_back(turn_id.to_string());
}

/// A completed turn is worth pushing when this bot did not start it (the
/// inbound path already answered it) and it did not end in failure. Whether
/// there is anything to send is decided by the turn's own text, which keeps a
/// degraded turn that still carries an explanation from being dropped.
fn proactive_push_eligible(own_turn: bool, success: Option<bool>) -> bool {
    !own_turn && success != Some(false)
}

/// Whether `planned_replies` more replies still fit the proactive share.
///
/// The unit is one channel reply, so a message the channel splits into two
/// parts asks for two. The share is never allowed to overspend, which is what
/// keeps the rest of the channel quota available for inbound replies.
fn proactive_reply_budget_allows(used_replies: usize, planned_replies: usize) -> bool {
    used_replies + planned_replies <= MAX_PROACTIVE_REPLIES_PER_WINDOW
}

/// Drops reply spend that has left the quota window.
///
/// The window rolls with each reply instead of resetting on a fixed boundary,
/// so this path can never spend the whole share in one burst at the end of a
/// window. Spend is recorded in order, so expired entries are always at the
/// front.
fn drop_expired_replies(spent: &mut VecDeque<i64>, now: i64) -> usize {
    let mut dropped = 0;
    while let Some(front) = spent.front() {
        if now.saturating_sub(*front) < REPLY_QUOTA_WINDOW_SECS {
            break;
        }
        spent.pop_front();
        dropped += 1;
    }
    dropped
}

/// Records `replies` replies spent at `now`. The unit is one channel reply, so
/// a message the channel splits into two parts records two.
fn record_replies_spent(spent: &mut VecDeque<i64>, replies: usize, now: i64) {
    for _ in 0..replies {
        spent.push_back(now);
    }
}

/// Joins a peer's backlog into the single reply that carries all of it.
///
/// The quota counts replies, so a backlog must not be delivered one reply per
/// turn. The merged payload is capped at [`MAX_PROACTIVE_PUSH_BYTES`]: the
/// oldest outputs are dropped first, and when the newest output alone exceeds
/// the cap its head is sent, because the newest answer is the one the user is
/// waiting for.
fn merge_pending(pending: &[PendingOutbound]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    // Walk newest first so the byte cap drops the oldest output.
    for item in pending.iter().rev() {
        let text = item.text.trim();
        if text.is_empty() {
            continue;
        }
        let separator = if kept.is_empty() {
            0
        } else {
            PENDING_SEPARATOR.len()
        };
        if bytes + separator + text.len() > MAX_PROACTIVE_PUSH_BYTES {
            break;
        }
        bytes += separator + text.len();
        kept.push(text);
    }
    if kept.is_empty() {
        return pending
            .iter()
            .rev()
            .map(|item| item.text.trim())
            .find(|text| !text.is_empty())
            .map(|text| {
                crate::util::truncate_at_char_boundary(text, MAX_PROACTIVE_PUSH_BYTES).to_string()
            })
            .unwrap_or_default();
    }
    kept.reverse();
    kept.join(PENDING_SEPARATOR)
}

#[derive(Debug, Clone)]
struct PendingPairing {
    created_at: i64,
}

pub struct WeixinBot {
    api: Arc<WeixinProviderClient>,
    pending_pairings: Arc<RwLock<HashMap<String, PendingPairing>>>,
    chat_states: Arc<RwLock<HashMap<String, BotChatState>>>,
    context_tokens: Arc<RwLock<HashMap<String, String>>>,
    runtime_fence: BotRuntimeFence,
    /// Turn ids this bot started from an inbound message. The inbound path
    /// already answers those turns, so the proactive channel must skip them or
    /// every inbound message would be delivered twice.
    own_turns: Arc<Mutex<VecDeque<String>>>,
    /// Proactive output per peer that has no deliverable context_token yet,
    /// oldest first.
    pending_outbound: Arc<Mutex<HashMap<String, PendingPeer>>>,
    /// Timestamps of the channel replies this path already spent, one entry
    /// per reply including the parts the channel split it into.
    proactive_replies: Arc<Mutex<VecDeque<i64>>>,
    outbound_wakeup: Arc<Notify>,
    /// Identity of this instance's proactive session-output subscription, so a
    /// replaced bot retires its own hook instead of the replacement's.
    session_output_hook_id: String,
}

pub async fn weixin_qr_start(base_url_override: Option<String>) -> Result<WeixinQrStartResponse> {
    weixin_provider::weixin_qr_start(base_url_override, None, None).await
}

pub async fn weixin_qr_start_with_existing(
    base_url_override: Option<String>,
    existing_ilink_token: Option<String>,
    existing_bot_account_id: Option<String>,
) -> Result<WeixinQrStartResponse> {
    weixin_provider::weixin_qr_start(
        base_url_override,
        existing_ilink_token,
        existing_bot_account_id,
    )
    .await
}

pub async fn weixin_qr_poll(
    session_key: &str,
    base_url_override: Option<String>,
    verify_code: Option<String>,
) -> Result<WeixinQrPollResponse> {
    weixin_provider::weixin_qr_poll(session_key, base_url_override, verify_code).await
}

impl WeixinBot {
    #[cfg(test)]
    pub fn new(config: WeixinConfig) -> Self {
        Self::new_fenced(config, BotRuntimeFence::standalone())
    }

    pub(crate) fn new_fenced(config: WeixinConfig, runtime_fence: BotRuntimeFence) -> Self {
        let context_tokens = weixin_provider::load_context_tokens(&config.bot_account_id);
        Self {
            api: Arc::new(WeixinProviderClient::new(config)),
            pending_pairings: Arc::new(RwLock::new(HashMap::new())),
            chat_states: Arc::new(RwLock::new(HashMap::new())),
            context_tokens: Arc::new(RwLock::new(context_tokens)),
            runtime_fence,
            own_turns: Arc::new(Mutex::new(VecDeque::new())),
            pending_outbound: Arc::new(Mutex::new(HashMap::new())),
            proactive_replies: Arc::new(Mutex::new(VecDeque::new())),
            outbound_wakeup: Arc::new(Notify::new()),
            session_output_hook_id: format!("weixin_session_output_{}", uuid::Uuid::new_v4()),
        }
    }

    pub async fn restore_chat_state(&self, peer_id: &str, mut state: BotChatState) {
        state.prepare_for_restore();
        let mut states = self.chat_states.write().await;
        self.runtime_fence.reconcile_states(&mut states);
        states.insert(peer_id.to_string(), state);
        let restored = states
            .get(peer_id)
            .cloned()
            .expect("restored Weixin state should exist");
        drop(states);
        self.persist_chat_state(peer_id, &restored).await;
    }

    pub async fn clear_delegated_identities(&self) {
        match tokio::time::timeout(
            std::time::Duration::from_millis(100),
            self.chat_states.write(),
        )
        .await
        {
            Ok(mut states) => {
                self.runtime_fence.clear_states(&mut states);
                let snapshots: Vec<_> = states
                    .iter()
                    .map(|(peer_id, state)| (peer_id.clone(), state.clone()))
                    .collect();
                drop(states);
                for (peer_id, state) in snapshots {
                    self.persist_chat_state(&peer_id, &state).await;
                }
            }
            Err(_) => {
                warn!("Weixin account identity clear deferred behind an in-flight command");
            }
        }
    }

    pub async fn register_pairing(&self, pairing_code: &str) -> Result<()> {
        self.pending_pairings.write().await.insert(
            pairing_code.to_string(),
            PendingPairing {
                created_at: chrono::Utc::now().timestamp(),
            },
        );
        Ok(())
    }

    pub async fn verify_pairing_code(&self, code: &str) -> bool {
        let mut pairings = self.pending_pairings.write().await;
        if let Some(pairing) = pairings.remove(code) {
            let age = chrono::Utc::now().timestamp() - pairing.created_at;
            return age < 300;
        }
        false
    }

    pub async fn send_text(&self, peer_id: &str, text: &str) -> Result<()> {
        let token = self.context_token_for_peer(peer_id).await?;
        if let Err(err) = self.api.send_text_chunks(peer_id, &token, text).await {
            if WeixinProviderClient::is_context_token_error(&err) {
                let mut tokens = self.context_tokens.write().await;
                if tokens
                    .get(peer_id)
                    .map(|cached| cached == &token)
                    .unwrap_or(false)
                {
                    tokens.remove(peer_id);
                    weixin_provider::save_context_tokens(
                        &self.api.config().bot_account_id,
                        &tokens,
                    );
                    warn!(
                        "weixin: dropped stale context_token for peer {peer_id} after send error: {err}"
                    );
                }
            }
            return Err(err);
        }
        Ok(())
    }

    async fn remember_context_token(&self, peer_id: &str, token: String) {
        if !self.runtime_fence.is_lifecycle_current() {
            return;
        }
        let mut tokens = self.context_tokens.write().await;
        tokens.insert(peer_id.to_string(), token);
        weixin_provider::save_context_tokens(&self.api.config().bot_account_id, &tokens);
        drop(tokens);
        // A fresh token is the only thing that makes a stalled proactive
        // backlog deliverable, so retry it as soon as one arrives.
        self.outbound_wakeup.notify_one();
    }

    /// Registers the proactive session-output hook for this bot instance.
    ///
    /// Turns started outside the inbound path — a scheduled job, the desktop
    /// window, another controller — produce assistant output that the reply
    /// path never sees. The hook forwards that output to the peer bound to the
    /// turn's session instead of dropping it.
    pub fn subscribe_session_output(self: &Arc<Self>) {
        let Some(coordinator) = get_global_coordinator() else {
            warn!("weixin: session output hook unavailable; proactive push disabled");
            return;
        };
        coordinator.subscribe_internal(
            self.session_output_hook_id.clone(),
            WeixinSessionOutputSubscriber {
                bot: Arc::downgrade(self),
            },
        );
    }

    /// Retires this instance's proactive session-output hook.
    pub fn unsubscribe_session_output(&self) {
        if let Some(coordinator) = get_global_coordinator() {
            coordinator.unsubscribe_internal(&self.session_output_hook_id);
        }
    }

    /// Starts the single worker that drains proactive backlogs.
    ///
    /// One worker per bot preserves order and keeps two triggers from
    /// delivering the same queued message.
    fn spawn_outbound_worker(self: &Arc<Self>, mut stop: tokio::sync::watch::Receiver<bool>) {
        let bot = self.clone();
        let wakeup = self.outbound_wakeup.clone();
        tokio::spawn(async move {
            loop {
                if *stop.borrow() || !bot.runtime_fence.is_lifecycle_current() {
                    break;
                }
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = wakeup.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(OUTBOUND_RETRY_INTERVAL_SECS)) => {}
                }
                bot.flush_pending_outbound().await;
            }
        });
    }

    /// Delivers assistant output produced by a turn this bot did not start.
    ///
    /// This is the explicit entry point behind the session-output hook, so the
    /// proactive path can also be driven directly when no coordinator event is
    /// available.
    pub async fn notify_session_output(&self, session_id: &str, text: &str) {
        if !self.runtime_fence.is_lifecycle_current() {
            return;
        }
        let Some(peer_id) = self.peer_for_session(session_id).await else {
            return;
        };
        self.queue_proactive(&peer_id, text, &format!("session {session_id}"))
            .await;
    }

    async fn handle_session_turn_completed(
        &self,
        session_id: String,
        turn_id: String,
        success: Option<bool>,
    ) {
        if !self.runtime_fence.is_lifecycle_current() {
            return;
        }
        if !proactive_push_eligible(self.is_own_turn(&turn_id).await, success) {
            return;
        }
        let Some(peer_id) = self.peer_for_session(&session_id).await else {
            return;
        };
        let text = self.read_turn_text(&session_id, &turn_id).await;
        // The session read is an await point: a bot replaced meanwhile must not
        // push, even though the turn itself is worth delivering.
        if !self.runtime_fence.is_lifecycle_current() {
            return;
        }
        self.queue_proactive(&peer_id, &text, &format!("turn {turn_id}"))
            .await;
    }

    async fn queue_proactive(&self, peer_id: &str, text: &str, source: &str) {
        if text.trim().is_empty() {
            debug!("weixin: proactive push skipped for peer {peer_id}: {source} has no text");
            return;
        }
        info!("weixin: queueing proactive push for peer {peer_id} from {source}");
        self.enqueue_outbound(peer_id, text).await;
        self.outbound_wakeup.notify_one();
    }

    /// The peer whose bot chat is bound to `session_id`, if this bot owns it.
    ///
    /// A chat routed to another device executes the turn elsewhere, so its text
    /// is unreadable here and must not be pushed from this host.
    async fn peer_for_session(&self, session_id: &str) -> Option<String> {
        let states = self.chat_states.read().await;
        states
            .iter()
            .find(|(_, state)| {
                state.paired
                    && !state.account_remote_context
                    && state.active_remote_device.is_none()
                    && state.current_session_id.as_deref() == Some(session_id)
            })
            .map(|(peer_id, _)| peer_id.clone())
    }

    /// Reads the assistant text of `turn_id` through the replay-safe turn
    /// projection the inbound reply path already relies on.
    async fn read_turn_text(&self, session_id: &str, turn_id: &str) -> String {
        use openbitfun_services_integrations::remote_connect::bot::remote_turn::observe_turn;
        use openbitfun_services_integrations::remote_connect::{
            handle_remote_poll_command, RemoteCommand,
        };

        let dispatcher =
            crate::service::remote_connect::remote_server::get_or_init_global_dispatcher();
        let poll_host =
            crate::service_agent_runtime::CoreServiceAgentRuntime::remote_poll_host(&dispatcher);
        let poll = handle_remote_poll_command(
            &poll_host,
            &RemoteCommand::PollSession {
                session_id: session_id.to_string(),
                since_version: 0,
                known_msg_count: 0,
                known_model_catalog_version: None,
            },
        )
        .await;
        serde_json::to_value(poll)
            .ok()
            .and_then(|poll| observe_turn(&poll, turn_id))
            .map(|turn| turn.text)
            .unwrap_or_default()
    }

    async fn mark_own_turn(&self, turn_id: &str) {
        let mut turns = self.own_turns.lock().await;
        push_tracked_turn(&mut turns, turn_id);
    }

    async fn is_own_turn(&self, turn_id: &str) -> bool {
        let turns = self.own_turns.lock().await;
        turns.iter().any(|tracked| tracked == turn_id)
    }

    async fn enqueue_outbound(&self, peer_id: &str, text: &str) {
        let now = chrono::Utc::now().timestamp();
        let dropped = {
            let mut pending = self.pending_outbound.lock().await;
            let peer = pending.entry(peer_id.to_string()).or_default();
            push_pending(
                &mut peer.queue,
                PendingOutbound {
                    text: text.to_string(),
                    queued_at: now,
                },
                now,
            )
        };
        if dropped > 0 {
            warn!(
                "weixin: proactive push backlog for peer {peer_id} exceeded its limit or retention window; dropped {dropped} oldest message(s)"
            );
        }
    }

    async fn flush_pending_outbound(&self) {
        let peers: Vec<String> = {
            let pending = self.pending_outbound.lock().await;
            pending.keys().cloned().collect()
        };
        for peer_id in peers {
            self.flush_pending_for_peer(&peer_id).await;
        }
    }

    /// Delivers one peer's backlog as a single reply.
    ///
    /// The channel counts every reply against a 24 hour quota that also answers
    /// the user's own messages, so the backlog is merged and the reply count the
    /// merge will spend is checked before anything is sent.
    async fn flush_pending_for_peer(&self, peer_id: &str) {
        if !self.runtime_fence.is_lifecycle_current() {
            return;
        }
        let expired = self.expire_pending(peer_id).await;
        if expired > 0 {
            warn!(
                "weixin: dropped {expired} proactive message(s) for peer {peer_id} past the {PENDING_OUTBOUND_TTL_SECS}s retention window"
            );
        }
        let pending = self.pending_snapshot(peer_id).await;
        if pending.is_empty() {
            return;
        }
        let merged = merge_pending(&pending);
        if merged.trim().is_empty() {
            // Nothing deliverable in the backlog, so it is not worth a reply.
            self.confirm_pending_sent(peer_id, pending.len()).await;
            return;
        }
        let planned_replies = weixin_provider::weixin_reply_count(&merged);
        let now = chrono::Utc::now().timestamp();
        if !self.proactive_budget_allows(now, planned_replies).await {
            if self
                .mark_deferred(peer_id, "proactive_reply_share_spent")
                .await
            {
                warn!(
                    "weixin: proactive push to peer {peer_id} waiting; the proactive share of {MAX_PROACTIVE_REPLIES_PER_WINDOW} of the channel's {REPLY_QUOTA_PER_WINDOW} replies per {}h window is spent and this push needs {planned_replies}",
                    REPLY_QUOTA_WINDOW_SECS / 3600
                );
            }
            return;
        }
        if let Err(err) = self.send_text(peer_id, &merged).await {
            // Keep the backlog. A stale token is replaced by the next inbound
            // message and `send_text` has already dropped it, so the retry has a
            // chance to succeed; the retention window bounds it.
            if self.mark_deferred(peer_id, "no_context_token").await {
                warn!(
                    "weixin: proactive push to peer {peer_id} deferred until a fresh context_token arrives: {err}"
                );
            }
            return;
        }
        self.record_spent_replies(now, planned_replies).await;
        info!(
            "weixin: proactive push delivered to peer {peer_id} ({planned_replies} reply(ies) for {} queued message(s))",
            pending.len()
        );
        self.confirm_pending_sent(peer_id, pending.len()).await;
    }

    /// Drops queued entries past the retention window and returns how many.
    async fn expire_pending(&self, peer_id: &str) -> usize {
        let now = chrono::Utc::now().timestamp();
        let mut pending = self.pending_outbound.lock().await;
        let (dropped, now_empty) = match pending.get_mut(peer_id) {
            Some(peer) => {
                let dropped = drop_expired_pending(&mut peer.queue, now);
                (dropped, peer.queue.is_empty())
            }
            None => (0, false),
        };
        if now_empty {
            pending.remove(peer_id);
        }
        dropped
    }

    /// Clones a peer's backlog so the send below runs without holding the lock.
    async fn pending_snapshot(&self, peer_id: &str) -> Vec<PendingOutbound> {
        let pending = self.pending_outbound.lock().await;
        pending
            .get(peer_id)
            .map(|peer| peer.queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Removes the entries a successful send covered and clears the blocker
    /// flag. Anything enqueued while the send was in flight stays queued.
    async fn confirm_pending_sent(&self, peer_id: &str, sent: usize) {
        let mut pending = self.pending_outbound.lock().await;
        let now_empty = match pending.get_mut(peer_id) {
            Some(peer) => {
                for _ in 0..sent {
                    peer.queue.pop_front();
                }
                peer.deferred_reason = None;
                peer.queue.is_empty()
            }
            None => false,
        };
        if now_empty {
            pending.remove(peer_id);
        }
    }

    /// Flags the backlog's current blocker. Returns whether this is the first
    /// attempt blocked for that reason, so the caller logs once instead of on
    /// every retry.
    async fn mark_deferred(&self, peer_id: &str, reason: &'static str) -> bool {
        let mut pending = self.pending_outbound.lock().await;
        match pending.get_mut(peer_id) {
            Some(peer) if peer.deferred_reason != Some(reason) => {
                peer.deferred_reason = Some(reason);
                true
            }
            _ => false,
        }
    }

    /// Whether `planned_replies` more replies fit the proactive share, after
    /// retiring spend that has left the quota window.
    async fn proactive_budget_allows(&self, now: i64, planned_replies: usize) -> bool {
        let mut spent = self.proactive_replies.lock().await;
        drop_expired_replies(&mut spent, now);
        proactive_reply_budget_allows(spent.len(), planned_replies)
    }

    async fn record_spent_replies(&self, now: i64, replies: usize) {
        let mut spent = self.proactive_replies.lock().await;
        record_replies_spent(&mut spent, replies, now);
    }

    pub async fn notify_start(&self) -> Result<()> {
        self.api.notify_start().await
    }

    pub async fn notify_stop(&self) -> Result<()> {
        self.api.notify_stop().await
    }

    async fn context_token_for_peer(&self, peer_id: &str) -> Result<String> {
        self.context_tokens
            .read()
            .await
            .get(peer_id)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "context_token unavailable for peer {peer_id} (waiting for next inbound message)"
                )
            })
    }

    async fn try_send_text(&self, peer_id: &str, text: &str, ctx: &str) {
        if let Err(err) = self.send_text(peer_id, text).await {
            warn!("weixin: {ctx} send to peer {peer_id} failed: {err}");
        }
    }

    async fn send_handle_result(&self, peer_id: &str, result: &HandleResult) {
        let language = current_bot_language().await;
        let text = if result.menu.items.is_empty() && result.menu.title.is_empty() {
            result.reply.clone()
        } else {
            result.menu.render_plain_text(language)
        };
        if text.trim().is_empty() {
            return;
        }
        if let Err(err) = self.send_text(peer_id, &text).await {
            warn!("weixin send_handle_result: {err}");
        }
    }

    async fn inbound_image_attachments_from_message(
        &self,
        msg: &Value,
    ) -> (Vec<ImageAttachment>, usize) {
        const MAX_BYTES: usize = 1024 * 1024;

        let (raw_images, skipped) = self.api.download_inbound_images(msg).await;
        let mut attachments = Vec::with_capacity(raw_images.len());
        for raw in raw_images {
            let data_url = if raw.bytes.len() <= MAX_BYTES {
                let b64 = B64.encode(&raw.bytes);
                format!("data:{};base64,{b64}", raw.mime_type)
            } else {
                match crate::agentic::image_analysis::optimize_image_with_size_limit(
                    raw.bytes.clone(),
                    "openai",
                    Some(raw.mime_type),
                    Some(MAX_BYTES),
                ) {
                    Ok(processed) => {
                        let b64 = B64.encode(&processed.data);
                        format!("data:{};base64,{}", processed.mime_type, b64)
                    }
                    Err(err) => {
                        warn!("Weixin image compression failed: {err}");
                        let b64 = B64.encode(&raw.bytes);
                        format!("data:{};base64,{b64}", raw.mime_type)
                    }
                }
            };
            attachments.push(ImageAttachment {
                name: raw.name,
                data_url,
            });
        }
        (attachments, skipped)
    }

    async fn notify_files_ready(
        &self,
        peer_id: &str,
        session_id: &str,
        remote_target: Option<&super::command_router::RemoteBotTarget>,
        text: &str,
        identity_epoch: u64,
    ) {
        let language = current_bot_language().await;
        for reference in super::extract_output_file_references(text) {
            if !self.runtime_fence.is_lifecycle_current()
                || self.runtime_fence.identity_epoch() != identity_epoch
            {
                return;
            }
            let content = super::read_output_file(
                session_id,
                remote_target,
                &reference,
                MAX_WEIXIN_FILE_BYTES,
                &|| {
                    self.runtime_fence.is_lifecycle_current()
                        && self.runtime_fence.identity_epoch() == identity_epoch
                },
            )
            .await;
            if !self.runtime_fence.is_lifecycle_current()
                || self.runtime_fence.identity_epoch() != identity_epoch
            {
                return;
            }
            let result = match content {
                Ok(content) => match self.context_token_for_peer(peer_id).await {
                    Ok(token) => {
                        self.api
                            .send_file_content_to_peer(peer_id, &token, content)
                            .await
                    }
                    Err(error) => Err(error),
                }
                .map_err(|error| error.to_string()),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                warn!("Weixin output file delivery failed: {error}");
                let notice = super::auto_push_failed_message(language, &reference, &error);
                if self.runtime_fence.is_lifecycle_current()
                    && self.runtime_fence.identity_epoch() == identity_epoch
                {
                    let _ = self.send_text(peer_id, &notice).await;
                }
            }
        }
    }

    async fn persist_chat_state(&self, peer_id: &str, state: &BotChatState) {
        let config = self.api.config().clone();
        let snapshot = self.runtime_fence.persistence_snapshot(state);
        let connection = SavedBotConnection {
            account_user_id: self.runtime_fence.account_user_id(),
            bot_type: "weixin".to_string(),
            chat_id: peer_id.to_string(),
            config: BotConfig::Weixin {
                ilink_token: config.ilink_token.clone(),
                base_url: config.base_url.clone(),
                bot_account_id: config.bot_account_id.clone(),
            },
            chat_state: snapshot,
            connected_at: chrono::Utc::now().timestamp(),
        };
        self.runtime_fence.commit_if_current(|| {
            update_bot_persistence(|data| data.upsert(connection));
        });
    }

    pub async fn wait_for_pairing(
        &self,
        stop_rx: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<String> {
        info!("Weixin bot waiting for pairing code (getupdates)...");
        let mut buf = weixin_provider::load_sync_buf(&self.api.config().bot_account_id);
        let mut long_poll_timeout = Duration::from_secs(LONG_POLL_TIMEOUT_SECS);

        loop {
            if *stop_rx.borrow() {
                return Err(anyhow!("bot stop requested"));
            }

            let poll = tokio::select! {
                _ = stop_rx.changed() => {
                    return Err(anyhow!("bot stop requested"));
                }
                result = self.api.get_updates_once(
                    &buf,
                    long_poll_timeout,
                ) => result,
            };

            let resp = match poll {
                Ok(value) => value,
                Err(err) => {
                    error!("weixin getupdates: {err}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            long_poll_timeout =
                weixin_provider::suggested_long_poll_timeout(&resp, long_poll_timeout);

            let ret = resp["ret"].as_i64().unwrap_or(0);
            let errcode = resp["errcode"].as_i64().unwrap_or(0);
            if weixin_provider::updates_failed(&resp) {
                if errcode == WEIXIN_SESSION_EXPIRED_ERRCODE
                    || ret == WEIXIN_SESSION_EXPIRED_ERRCODE
                {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                warn!("weixin getupdates ret={ret} errcode={errcode}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }

            if let Some(new_buf) = resp["get_updates_buf"]
                .as_str()
                .filter(|buf| !buf.is_empty())
            {
                buf = new_buf.to_string();
                weixin_provider::save_sync_buf(&self.api.config().bot_account_id, &buf);
            }

            if let Some(msgs) = resp["msgs"].as_array() {
                for msg in msgs {
                    if !weixin_provider::is_user_message(msg) {
                        continue;
                    }
                    let Some(peer) = weixin_provider::peer_id(msg) else {
                        continue;
                    };
                    if let Some(token) = weixin_provider::context_token(msg) {
                        self.remember_context_token(&peer, token).await;
                    }
                    let text = weixin_provider::body_from_message(msg).trim().to_string();
                    let language = current_bot_language().await;

                    if text == "/start" {
                        self.try_send_text(&peer, welcome_message(language), "welcome")
                            .await;
                        continue;
                    }

                    if text.len() == 6 && text.chars().all(|c| c.is_ascii_digit()) {
                        if self.verify_pairing_code(&text).await {
                            info!("Weixin pairing successful peer={peer}");
                            let mut state = BotChatState::new(peer.clone());
                            let identity_epoch = self.runtime_fence.identity_epoch();
                            let result = complete_im_bot_pairing(&mut state).await;
                            if *stop_rx.borrow() || !self.runtime_fence.is_lifecycle_current() {
                                return Err(anyhow!("bot lifecycle replaced during pairing"));
                            }
                            let mut states = self.chat_states.write().await;
                            self.runtime_fence.reconcile_states(&mut states);
                            self.runtime_fence
                                .sanitize_after_epoch(identity_epoch, &mut state);
                            states.insert(peer.clone(), state.clone());
                            drop(states);
                            self.persist_chat_state(&peer, &state).await;

                            self.send_handle_result(&peer, &result).await;
                            return Ok(peer);
                        }
                        let err = if language.is_chinese() {
                            "\u{914d}\u{5bf9}\u{7801}\u{65e0}\u{6548}\u{6216}\u{5df2}\u{8fc7}\u{671f}\u{ff0c}\u{8bf7}\u{91cd}\u{8bd5}\u{3002}"
                        } else {
                            "Invalid or expired pairing code."
                        };
                        self.try_send_text(&peer, err, "pairing-invalid").await;
                    } else if !text.is_empty() {
                        let err = if language.is_chinese() {
                            "\u{8bf7}\u{8f93}\u{5165} OpenBitFun \u{684c}\u{9762}\u{7aef}\u{8fdc}\u{7a0b}\u{8fde}\u{63a5}\u{4e2d}\u{663e}\u{793a}\u{7684} 6 \u{4f4d}\u{914d}\u{5bf9}\u{7801}\u{3002}"
                        } else {
                            "Please send the 6-digit pairing code from OpenBitFun Desktop Remote Connect."
                        };
                        self.try_send_text(&peer, err, "pairing-prompt").await;
                    } else if weixin_provider::has_inbound_image_items(msg) {
                        let err = if language.is_chinese() {
                            "\u{914d}\u{5bf9}\u{8bf7}\u{76f4}\u{63a5}\u{53d1}\u{9001} 6 \u{4f4d}\u{6570}\u{5b57}\u{914d}\u{5bf9}\u{7801}\u{ff1b}\u{5b8c}\u{6210}\u{914d}\u{5bf9}\u{540e}\u{518d}\u{53d1}\u{9001}\u{56fe}\u{7247}\u{4e0e}\u{52a9}\u{624b}\u{5bf9}\u{8bdd}\u{3002}"
                        } else {
                            "To pair, send the 6-digit code only. After pairing you can send images to chat."
                        };
                        self.try_send_text(&peer, err, "pairing-image-hint").await;
                    }
                }
            }
        }
    }

    pub async fn run_message_loop(self: Arc<Self>, stop_rx: tokio::sync::watch::Receiver<bool>) {
        info!("Weixin message loop started");
        self.subscribe_session_output();
        self.spawn_outbound_worker(stop_rx.clone());
        let mut stop = stop_rx;
        let mut buf = weixin_provider::load_sync_buf(&self.api.config().bot_account_id);
        let mut long_poll_timeout = Duration::from_secs(LONG_POLL_TIMEOUT_SECS);

        loop {
            if *stop.borrow() {
                break;
            }

            let poll = tokio::select! {
                _ = stop.changed() => break,
                result = self.api.get_updates_once(
                    &buf,
                    long_poll_timeout,
                ) => result,
            };

            let resp = match poll {
                Ok(value) => value,
                Err(err) => {
                    error!("weixin getupdates (loop): {err}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            long_poll_timeout =
                weixin_provider::suggested_long_poll_timeout(&resp, long_poll_timeout);

            let ret = resp["ret"].as_i64().unwrap_or(0);
            let errcode = resp["errcode"].as_i64().unwrap_or(0);
            if weixin_provider::updates_failed(&resp) {
                if errcode == WEIXIN_SESSION_EXPIRED_ERRCODE
                    || ret == WEIXIN_SESSION_EXPIRED_ERRCODE
                {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }

            if let Some(new_buf) = resp["get_updates_buf"]
                .as_str()
                .filter(|buf| !buf.is_empty())
            {
                buf = new_buf.to_string();
                weixin_provider::save_sync_buf(&self.api.config().bot_account_id, &buf);
            }

            let Some(msgs) = resp["msgs"].as_array() else {
                continue;
            };

            for msg in msgs {
                if !weixin_provider::is_user_message(msg) {
                    continue;
                }
                let Some(peer) = weixin_provider::peer_id(msg) else {
                    continue;
                };
                if let Some(token) = weixin_provider::context_token(msg) {
                    self.remember_context_token(&peer, token).await;
                }
                let msg_value = msg.clone();
                let bot = self.clone();
                tokio::spawn(async move {
                    let (images, skipped_images) =
                        bot.inbound_image_attachments_from_message(&msg_value).await;
                    let language = current_bot_language().await;
                    if skipped_images > 0 {
                        let note = if language.is_chinese() {
                            format!(
                                "\u{4ec5}\u{4f1a}\u{5904}\u{7406}\u{524d} {} \u{5f20}\u{56fe}\u{7247}\u{ff0c}\u{5176}\u{4f59} {} \u{5f20}\u{5df2}\u{4e22}\u{5f03}\u{3002}",
                                MAX_INBOUND_IMAGES, skipped_images
                            )
                        } else {
                            format!(
                                "Only the first {} images will be processed; the remaining {} were discarded.",
                                MAX_INBOUND_IMAGES, skipped_images
                            )
                        };
                        bot.try_send_text(&peer, &note, "image-truncation-notice")
                            .await;
                    }
                    let body = weixin_provider::body_from_message(&msg_value);
                    let text = if body.trim().is_empty() && !images.is_empty() {
                        if language.is_chinese() {
                            "[\u{7528}\u{6237}\u{53d1}\u{9001}\u{4e86}\u{4e00}\u{5f20}\u{56fe}\u{7247}]".to_string()
                        } else {
                            "[User sent an image]".to_string()
                        }
                    } else {
                        body
                    };
                    bot.handle_incoming_message(peer, &text, images).await;
                });
            }
        }
        self.unsubscribe_session_output();
        info!("Weixin message loop stopped");
    }

    async fn handle_incoming_message(
        self: &Arc<Self>,
        peer_id: String,
        text: &str,
        images: Vec<ImageAttachment>,
    ) {
        if !self.runtime_fence.is_lifecycle_current() {
            return;
        }
        let mut states = self.chat_states.write().await;
        let command_identity_epoch = self.runtime_fence.identity_epoch();
        self.runtime_fence.reconcile_states(&mut states);
        let state = states.entry(peer_id.clone()).or_insert_with(|| {
            let mut state = BotChatState::new(peer_id.clone());
            state.paired = true;
            state
        });
        let language = current_bot_language().await;

        if !state.paired {
            let trimmed = text.trim();
            if trimmed == "/start" {
                drop(states);
                self.try_send_text(&peer_id, welcome_message(language), "welcome")
                    .await;
                return;
            }
            if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_digit()) {
                if self.verify_pairing_code(trimmed).await {
                    let identity_epoch = self.runtime_fence.identity_epoch();
                    let result = complete_im_bot_pairing(state).await;
                    self.runtime_fence
                        .sanitize_after_epoch(identity_epoch, state);
                    self.persist_chat_state(&peer_id, state).await;
                    drop(states);
                    if !self.runtime_fence.is_lifecycle_current()
                        || self.runtime_fence.identity_epoch() != command_identity_epoch
                    {
                        return;
                    }
                    self.send_handle_result(&peer_id, &result).await;
                    return;
                }
                let err = if language.is_chinese() {
                    "\u{914d}\u{5bf9}\u{7801}\u{65e0}\u{6548}\u{6216}\u{5df2}\u{8fc7}\u{671f}\u{3002}"
                } else {
                    "Invalid or expired pairing code."
                };
                drop(states);
                self.try_send_text(&peer_id, err, "pairing-invalid").await;
                return;
            }
            drop(states);
            let err = if language.is_chinese() {
                "\u{8bf7}\u{8f93}\u{5165} 6 \u{4f4d}\u{914d}\u{5bf9}\u{7801}\u{3002}"
            } else {
                "Please send the 6-digit pairing code."
            };
            self.try_send_text(&peer_id, err, "pairing-prompt").await;
            return;
        }

        if self.runtime_fence.identity_epoch() != command_identity_epoch {
            return;
        }
        let command = parse_command(text);
        let result = handle_command(state, command, images).await;
        self.runtime_fence.reconcile_states(&mut states);
        if let Some(state) = states.get(&peer_id) {
            self.persist_chat_state(&peer_id, state).await;
        }
        drop(states);

        if !self.runtime_fence.is_lifecycle_current()
            || self.runtime_fence.identity_epoch() != command_identity_epoch
        {
            return;
        }

        self.send_handle_result(&peer_id, &result).await;

        if let Some(forward) = result.forward_to_session {
            // The inbound path answers this turn itself; keep the proactive
            // hook from delivering the same answer a second time.
            self.mark_own_turn(&forward.turn_id).await;
            let output_session_id = forward.session_id.clone();
            let output_remote_target = forward.remote_target.clone();
            let output_identity_epoch = command_identity_epoch;
            let bot = self.clone();
            let peer = peer_id.clone();
            let typing_token = self.context_tokens.read().await.get(&peer_id).cloned();
            let typing_for_turn = self.api.start_typing(peer_id.clone(), typing_token);
            tokio::spawn(async move {
                let interaction_bot = bot.clone();
                let peer_c = peer.clone();
                let handler: BotInteractionHandler =
                    Arc::new(move |interaction: BotInteractiveRequest| {
                        let interaction_bot = interaction_bot.clone();
                        let peer_i = peer_c.clone();
                        Box::pin(async move {
                            interaction_bot
                                .deliver_interaction(peer_i, interaction, output_identity_epoch)
                                .await;
                        })
                    });
                let msg_bot = bot.clone();
                let peer_m = peer.clone();
                let sender: BotMessageSender = Arc::new(move |text: String| {
                    let msg_bot = msg_bot.clone();
                    let peer_s = peer_m.clone();
                    Box::pin(async move {
                        if !msg_bot.runtime_fence.is_lifecycle_current()
                            || msg_bot.runtime_fence.identity_epoch() != output_identity_epoch
                        {
                            return;
                        }
                        if let Err(err) = msg_bot.send_text(&peer_s, &text).await {
                            warn!(
                                "weixin: send intermediate message to peer {peer_s} failed: {err}"
                            );
                        }
                    })
                });
                let verbose_mode = load_bot_persistence().verbose_mode;
                let turn_result = execute_forwarded_turn(
                    forward,
                    Some(handler),
                    Some(sender),
                    verbose_mode,
                    &bot.runtime_fence,
                    output_identity_epoch,
                )
                .await;
                if !bot.runtime_fence.is_lifecycle_current()
                    || bot.runtime_fence.identity_epoch() != output_identity_epoch
                {
                    return;
                }
                if let Some(next) = super::retire_remote_interactions(
                    &bot.chat_states,
                    &peer,
                    output_remote_target.as_ref(),
                    &turn_result.completed_remote_tools,
                    &bot.runtime_fence,
                    output_identity_epoch,
                )
                .await
                {
                    bot.deliver_interaction(peer.clone(), next, output_identity_epoch)
                        .await;
                }
                if !turn_result.display_text.is_empty() {
                    if let Err(err) = bot.send_text(&peer, &turn_result.display_text).await {
                        warn!("weixin: send final reply to peer {peer} failed: {err}");
                    }
                }
                bot.notify_files_ready(
                    &peer,
                    &output_session_id,
                    output_remote_target.as_ref(),
                    &turn_result.full_text,
                    output_identity_epoch,
                )
                .await;
                typing_for_turn.stop().await;
            });
        }
    }

    async fn deliver_interaction(
        &self,
        peer_id: String,
        interaction: BotInteractiveRequest,
        identity_epoch: u64,
    ) {
        if !self.runtime_fence.is_lifecycle_current() {
            return;
        }
        let mut states = self.chat_states.write().await;
        self.runtime_fence.reconcile_states(&mut states);
        let state = states.entry(peer_id.clone()).or_insert_with(|| {
            let mut state = BotChatState::new(peer_id.clone());
            state.paired = true;
            state
        });
        if self.runtime_fence.identity_epoch() != identity_epoch {
            return;
        }
        if !super::command_router::apply_interactive_request(state, &interaction) {
            return;
        }
        self.persist_chat_state(&peer_id, state).await;
        drop(states);

        let result = HandleResult {
            reply: interaction.reply,
            actions: interaction.actions,
            forward_to_session: None,
            menu: interaction.menu,
        };
        self.send_handle_result(&peer_id, &result).await;
    }
}

/// Forwards completed turns that the inbound reply path does not own.
///
/// The bot is held weakly so a retired instance cannot keep pushing after its
/// lifecycle slot was replaced.
struct WeixinSessionOutputSubscriber {
    bot: Weak<WeixinBot>,
}

#[async_trait::async_trait]
impl EventSubscriber for WeixinSessionOutputSubscriber {
    async fn on_event(&self, event: &AgenticEvent) -> EventSubscriberResult {
        let AgenticEvent::DialogTurnCompleted {
            session_id,
            turn_id,
            success,
            ..
        } = event
        else {
            return Ok(());
        };
        let Some(bot) = self.bot.upgrade() else {
            return Ok(());
        };
        let session_id = session_id.clone();
        let turn_id = turn_id.clone();
        let success = *success;
        // The router awaits every subscriber inline while this path reads the
        // session and sends over the network, so the work must not run here.
        tokio::spawn(async move {
            bot.handle_session_turn_completed(session_id, turn_id, success)
                .await;
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn context_token_error_heuristic_uses_provider_contract() {
        let app_err = anyhow!(
            "ilink ilink/bot/sendmessage application error ret=0 errcode=12345 errmsg=context_token expired"
        );
        assert!(WeixinProviderClient::is_context_token_error(&app_err));

        let net_err = anyhow!("error sending request: connection refused");
        assert!(!WeixinProviderClient::is_context_token_error(&net_err));
    }

    #[test]
    fn body_from_message_plain_text_uses_provider_parser() {
        let msg = json!({
            "item_list": [{ "type": 1, "text_item": { "text": "hi" } }]
        });
        assert_eq!(weixin_provider::body_from_message(&msg), "hi");
    }

    #[test]
    fn body_from_message_quoted_text_uses_provider_parser() {
        let msg = json!({
            "item_list": [{
                "type": 1,
                "text_item": { "text": "reply" },
                "ref_msg": { "title": " earlier ", "message_item": { "type": 1, "text_item": { "text": "orig" } } }
            }]
        });
        let body = weixin_provider::body_from_message(&msg);
        assert!(body.contains("[\u{5f15}\u{7528}:"));
        assert!(body.contains("reply"));
    }

    fn pending(text: &str, queued_at: i64) -> PendingOutbound {
        PendingOutbound {
            text: text.to_string(),
            queued_at,
        }
    }

    fn backlog(texts: &[&str]) -> Vec<PendingOutbound> {
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| pending(*text, 1_000 + index as i64))
            .collect()
    }

    #[test]
    fn proactive_push_skips_bot_owned_and_failed_turns() {
        // A turn this bot started already got its reply on the inbound path, so
        // pushing it again would deliver every inbound message twice.
        assert!(!proactive_push_eligible(true, Some(true)));
        assert!(proactive_push_eligible(false, Some(true)));
        // A turn that ended in failure must not be pushed.
        assert!(!proactive_push_eligible(false, Some(false)));
        // Older runtimes omit `success`; the turn stays eligible and the
        // turn's own text decides whether there is anything to send.
        assert!(proactive_push_eligible(false, None));
    }

    #[test]
    fn tracked_bot_turns_stay_within_the_window() {
        let mut turns = VecDeque::new();
        for index in 0..(MAX_TRACKED_OWN_TURNS + 5) {
            push_tracked_turn(&mut turns, &format!("turn_{index}"));
        }
        assert_eq!(turns.len(), MAX_TRACKED_OWN_TURNS);
        let newest = format!("turn_{}", MAX_TRACKED_OWN_TURNS + 4);
        assert!(turns.iter().any(|id| id == &newest));
        assert!(!turns.iter().any(|id| id == "turn_0"));
    }

    #[test]
    fn pending_backlog_drops_the_oldest_past_the_pre_merge_cap() {
        // The count cap bounds what is held before merging; the merged payload
        // has its own byte cap.
        let mut queue = VecDeque::new();
        let now = 1_000_000;
        let mut dropped = 0;
        for index in 0..(MAX_PENDING_OUTBOUND_PER_PEER + 3) {
            dropped += push_pending(&mut queue, pending(&format!("m{index}"), now), now);
        }
        assert_eq!(queue.len(), MAX_PENDING_OUTBOUND_PER_PEER);
        assert_eq!(dropped, 3);
        assert_eq!(queue.front().map(|item| item.text.as_str()), Some("m3"));
        assert_eq!(queue.back().map(|item| item.text.as_str()), Some("m22"));
    }

    #[test]
    fn pending_backlog_expires_entries_past_the_retention_window() {
        let mut queue = VecDeque::new();
        let now = 1_000_000;
        push_pending(
            &mut queue,
            pending("stale", now - PENDING_OUTBOUND_TTL_SECS - 1),
            now,
        );
        push_pending(&mut queue, pending("fresh", now), now);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.front().map(|item| item.text.as_str()), Some("fresh"));
        assert_eq!(drop_expired_pending(&mut queue, now), 0);
    }

    #[test]
    fn merged_backlog_becomes_one_reply_in_time_order() {
        let merged = merge_pending(&backlog(&["first", "second", "third"]));
        assert_eq!(
            merged,
            format!("first{PENDING_SEPARATOR}second{PENDING_SEPARATOR}third")
        );
        // However many turns the backlog carries, it spends one reply.
        assert_eq!(weixin_provider::weixin_reply_count(&merged), 1);
    }

    #[test]
    fn merged_backlog_skips_entries_with_no_text() {
        let merged = merge_pending(&backlog(&["first", "   ", "", "second"]));
        assert_eq!(merged, format!("first{PENDING_SEPARATOR}second"));
    }

    #[test]
    fn merged_backlog_drops_the_oldest_to_fit_one_reply() {
        // The two answers cannot share one reply, so the newest is kept.
        let oldest = "o".repeat(MAX_PROACTIVE_PUSH_BYTES - 100);
        let newest = "n".repeat(200);
        let merged = merge_pending(&backlog(&[oldest.as_str(), newest.as_str()]));
        assert_eq!(merged, newest);
        assert_eq!(weixin_provider::weixin_reply_count(&merged), 1);
    }

    #[test]
    fn merged_backlog_truncates_a_single_oversized_answer() {
        // The newest answer alone exceeds the cap, so its head is sent instead
        // of nothing at all.
        let huge = "a".repeat(MAX_PROACTIVE_PUSH_BYTES + 100);
        let merged = merge_pending(&backlog(&[huge.as_str()]));
        assert_eq!(merged.len(), MAX_PROACTIVE_PUSH_BYTES);
        assert_eq!(weixin_provider::weixin_reply_count(&merged), 1);
    }

    #[test]
    fn merged_backlog_never_splits_a_character() {
        // The byte cap lands mid-character, so the head must stay valid UTF-8
        // and still fit inside one reply.
        let huge = "\u{5b57}".repeat(MAX_PROACTIVE_PUSH_BYTES);
        let merged = merge_pending(&backlog(&[huge.as_str()]));
        assert!(merged.len() <= MAX_PROACTIVE_PUSH_BYTES);
        assert_eq!(weixin_provider::weixin_reply_count(&merged), 1);
    }

    #[test]
    fn proactive_reply_budget_keeps_room_for_inbound_replies() {
        // The share is a minority slice of the channel quota, so the replies
        // the user's own messages receive still work.
        assert!(MAX_PROACTIVE_REPLIES_PER_WINDOW * 2 < REPLY_QUOTA_PER_WINDOW);
        // A push fits while the share has room for it.
        assert!(proactive_reply_budget_allows(0, 1));
        assert!(proactive_reply_budget_allows(
            MAX_PROACTIVE_REPLIES_PER_WINDOW - 1,
            1
        ));
        // Once the share is spent, nothing more is sent.
        assert!(!proactive_reply_budget_allows(
            MAX_PROACTIVE_REPLIES_PER_WINDOW,
            1
        ));
        // A push that needs more replies than the whole share is refused, which
        // is what keeps one push from draining it.
        assert!(!proactive_reply_budget_allows(
            0,
            MAX_PROACTIVE_REPLIES_PER_WINDOW + 1
        ));
    }

    #[test]
    fn reply_budget_allows_a_push_that_exactly_spends_the_share() {
        assert!(proactive_reply_budget_allows(
            MAX_PROACTIVE_REPLIES_PER_WINDOW - 2,
            2
        ));
    }

    #[test]
    fn reply_spend_leaves_the_window_on_a_rolling_basis() {
        let mut spent = VecDeque::new();
        let now = 1_000_000;
        record_replies_spent(&mut spent, 2, now);
        assert_eq!(spent.len(), 2);
        assert!(!proactive_reply_budget_allows(
            spent.len(),
            MAX_PROACTIVE_REPLIES_PER_WINDOW - 1
        ));

        // Still inside the window: the spend stands, so no burst is possible
        // at the end of a window.
        drop_expired_replies(&mut spent, now + REPLY_QUOTA_WINDOW_SECS - 1);
        assert_eq!(spent.len(), 2);

        // Past the window the spend is retired and the share is free again.
        assert_eq!(
            drop_expired_replies(&mut spent, now + REPLY_QUOTA_WINDOW_SECS),
            2
        );
        assert!(spent.is_empty());
        assert!(proactive_reply_budget_allows(spent.len(), 1));
    }
}
