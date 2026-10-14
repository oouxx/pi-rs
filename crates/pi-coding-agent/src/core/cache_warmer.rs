//! Prompt cache warming.
//!
//! Port of `packages/coding-agent/src/core/cache-warmer.ts` (TS #9668). Keeps one
//! prompt cache entry alive by re-sending its request with a one-token output
//! cap before the entry expires. Every refresh costs money, so a refresh is
//! only sent when it is expected to save at least
//! [`CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS`] dollars, and extensions may veto
//! or force each refresh through the `cache_warming_decision` event.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use pi_ai::models::calculate_cost;
use pi_ai::types::{
    AssistantMessage, CacheRetention, Context, Model, SimpleStreamOptions, StopReason, Usage,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::core::settings_manager::CacheWarmingMode;

/// Streaming warming never continues past this long after the real request
/// that started it.
const MAX_WARMING_AGE_MS: u64 = 60 * 60_000;
/// Idle warming uses a shorter horizon because continuation estimates become
/// less reliable with age.
const MAX_IDLE_WARMING_AGE_MS: u64 = 30 * 60_000;
/// A refresh is sent only when it is expected to save at least this many
/// dollars.
pub const CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS: f64 = 0.05;
/// Chance that a real request arrives before the cache entry expires while the
/// agent sits idle. Measured from our own usage; per-session estimates were not
/// better than this constant.
const IDLE_CONTINUATION_PROBABILITY: f64 = 0.15;
/// Below this lifetime a refresh cannot leave a useful margin.
const MIN_WARMABLE_TTL_MS: u64 = 10_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Refresh at 90% of the TTL while preserving at least ten seconds of margin
/// (TS `getCacheWarmingDelayMs`).
#[must_use]
pub fn get_cache_warming_delay_ms(ttl_ms: u64) -> Option<u64> {
    if ttl_ms <= MIN_WARMABLE_TTL_MS {
        return None;
    }
    Some(
        ((ttl_ms as f64 * 0.9) as u64)
            .min(ttl_ms - MIN_WARMABLE_TTL_MS)
            .max(1),
    )
}

/// Lifetime of the prompt cache entry a request writes, from the model's
/// `promptCache` tier for the retention the request used. `None` when the model
/// has no lifetime for that tier or caching is off
/// (TS `getPromptCacheTtlMs`).
#[must_use]
pub fn get_prompt_cache_ttl_ms(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
) -> Option<u64> {
    let retention = options
        .and_then(|o| o.base.cache_retention.clone())
        .unwrap_or_else(|| {
            let long = options
                .and_then(|o| o.base.env.as_ref())
                .and_then(|env| env.get("PI_CACHE_RETENTION"))
                .map(String::as_str)
                == Some("long");
            if long {
                CacheRetention::Long
            } else {
                CacheRetention::Short
            }
        });
    let seconds = match retention {
        CacheRetention::None => return None,
        CacheRetention::Short => model.prompt_cache.as_ref()?.short?,
        CacheRetention::Long => model.prompt_cache.as_ref()?.long?,
    };
    Some(seconds * 1000)
}

/// Whether replaying the request with a one-token output cap leaves its cache
/// entry untouched (TS `isReplayable`).
///
/// Anthropic's budget-based thinking (Claude models without adaptive thinking)
/// derives `budget_tokens` from `max_tokens`; the replay would get a different
/// budget, which Anthropic keys the message cache on, and the model could still
/// think for thousands of tokens.
#[must_use]
pub fn is_replayable(model: &Model, options: Option<&SimpleStreamOptions>) -> bool {
    let reasoning = options.and_then(|o| o.reasoning.as_ref()).is_some();
    if !reasoning || model.api != "anthropic-messages" {
        return true;
    }
    matches!(
        &model.compat,
        Some(pi_ai::types::ModelCompat::AnthropicMessages(compat))
            if compat.force_adaptive_thinking == Some(true)
    )
}

/// Prompt size of the most recent real request on the branch, as reported by
/// the provider (TS `lastPromptTokens`).
#[must_use]
pub fn last_prompt_tokens(entries: &[crate::core::session_manager::SessionEntry]) -> u64 {
    for entry in entries.iter().rev() {
        if let crate::core::session_manager::SessionEntry::Message { message, .. } = entry {
            if message.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                if let Some(usage) = message.get("usage") {
                    let field = |name: &str| usage.get(name).and_then(serde_json::Value::as_u64);
                    return field("input").unwrap_or(0)
                        + field("cacheRead").unwrap_or(0)
                        + field("cacheWrite").unwrap_or(0);
                }
                return 0;
            }
        }
    }
    0
}

/// Price of a token mix for a model (TS `price`).
fn price(model: &Model, tokens: TokenMix) -> f64 {
    let mut usage = Usage {
        input: tokens.input.unwrap_or(0),
        output: tokens.output.unwrap_or(0),
        cache_read: tokens.cache_read.unwrap_or(0),
        cache_write: tokens.cache_write.unwrap_or(0),
        ..Default::default()
    };
    calculate_cost(model, &mut usage);
    usage.cost.total
}

/// Token counts to price; unset counts are zero (TS `Partial<Pick<Usage, …>>`).
#[derive(Debug, Clone, Copy, Default)]
struct TokenMix {
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
}

/// Whether pi decides to refresh (TS `CacheWarmingAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingAction {
    /// Send the refresh.
    Warm,
    /// Stop warming until the next real request.
    Stop,
}

/// Inputs and outcome of one warm-or-stop decision, as shown by `/session`
/// (TS `CacheWarmingDecision`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingDecision {
    /// `"streaming"` while the agent run that sent the request is still active.
    pub phase: WarmingPhase,
    /// Price of this refresh: a cache read of the prompt plus one output token.
    pub warm_cost: f64,
    /// Extra price of the next real request if the cache entry is lost.
    pub miss_cost: f64,
    /// Estimated chance that a real request arrives before the entry expires.
    pub continuation_probability: f64,
    /// `continuation_probability * miss_cost - warm_cost`.
    pub expected_savings: f64,
    /// False when the prompt size or the model's prices are unknown.
    pub economics_available: bool,
    /// Pi's decision: `"warm"` when `expected_savings` is at least $0.05.
    pub action: CacheWarmingAction,
}

/// Which phase of the agent lifecycle a warming run is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WarmingPhase {
    /// The agent run that sent the request is still active.
    Streaming,
    /// The agent run settled.
    Idle,
}

/// Fired before each refresh with pi's decision filled in
/// (TS `CacheWarmingDecisionEvent`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingDecisionEvent {
    /// Event discriminator, always `"cache_warming_decision"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Price of this refresh.
    pub warm_cost: f64,
    /// Extra price of the next real request if the entry is lost.
    pub miss_cost: f64,
    /// Estimated chance the cache is used again before it expires.
    pub continuation_probability: f64,
    /// Pi's own decision, which the handler may override.
    pub action: CacheWarmingAction,
}

/// What an extension returned for a decision event (TS
/// `CacheWarmingDecisionEventResult`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingDecisionEventResult {
    /// Override whether this refresh is sent.
    pub action: Option<CacheWarmingAction>,
}

/// What `/session` reports about warming (TS `CacheWarmingStatus`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingStatus {
    /// `"scheduled"`: a refresh timer is armed; `"refreshing"`: a warm request
    /// is in flight.
    pub state: WarmingState,
    /// Why nothing is scheduled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When the next refresh is due.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_warm_at: Option<u64>,
    /// The pending decision, or the decision that stopped warming.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<CacheWarmingDecision>,
    /// True when an extension changed `decision.action`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_override: Option<bool>,
}

/// State of a warming run as reported by [`CacheWarmingStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WarmingState {
    /// Nothing is armed.
    Inactive,
    /// A refresh timer is armed.
    Scheduled,
    /// A warm request is in flight.
    Refreshing,
}

/// The request whose prompt cache entry should be kept warm, exactly as it was
/// sent (TS `CacheWarmRequest`).
#[derive(Clone)]
pub struct CacheWarmRequest {
    /// Model the request was sent to.
    pub model: Model,
    /// Provider context the request carried.
    pub context: Context,
    /// Options the request was sent with.
    pub options: SimpleStreamOptions,
}

/// The provider call the warmer replays (TS `Pick<ModelRuntime, "streamSimple">`).
pub trait CacheWarmModels: Send + Sync {
    /// Send one request and wait for its final message.
    fn stream_simple(
        &self,
        model: Model,
        context: Context,
        options: SimpleStreamOptions,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantMessage, String>> + Send>>;
}

/// The session surface the warmer reads and writes (TS
/// `Pick<SessionManager, "appendUsage" | "getBranch">`).
pub trait CacheWarmSession: Send + Sync {
    /// Entries from the root to the current leaf.
    fn branch_entries(&self) -> Vec<crate::core::session_manager::SessionEntry>;
    /// Record a usage entry for a refresh.
    fn append_usage(
        &self,
        kind: &str,
        provider: &str,
        model: &str,
        usage: Usage,
        note: Option<&str>,
    ) -> crate::core::session_manager::SessionEntry;
}

/// Called with the persisted usage entry after each successful refresh.
pub type WarmedFn = Arc<dyn Fn(&crate::core::session_manager::SessionEntry) + Send + Sync>;

/// [`CacheWarmModels`] over the session's model registry, which resolves auth
/// the same way a session request does.
pub struct RegistryCacheWarmModels {
    /// Registry used for the replay.
    pub registry: Arc<crate::core::model_registry::ModelRegistry>,
}

impl CacheWarmModels for RegistryCacheWarmModels {
    fn stream_simple(
        &self,
        model: Model,
        context: Context,
        options: SimpleStreamOptions,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantMessage, String>> + Send>> {
        let registry = Arc::clone(&self.registry);
        Box::pin(async move {
            let stream = registry.stream_simple(&model, &context, options).await?;
            stream.result().await
        })
    }
}

/// [`CacheWarmSession`] over the shared session manager.
pub struct ManagerCacheWarmSession {
    /// Session whose branch the warmer prices and records into.
    pub manager: Arc<std::sync::Mutex<crate::core::session_manager::SessionManager>>,
}

impl CacheWarmSession for ManagerCacheWarmSession {
    fn branch_entries(&self) -> Vec<crate::core::session_manager::SessionEntry> {
        self.manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_branch(None)
    }

    fn append_usage(
        &self,
        kind: &str,
        provider: &str,
        model: &str,
        usage: Usage,
        note: Option<&str>,
    ) -> crate::core::session_manager::SessionEntry {
        self.manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append_usage(kind, provider, model, usage, note)
    }
}

/// Callback run before each refresh; returns the (possibly overridden) action.
pub type DecideFn = Arc<
    dyn Fn(
            CacheWarmingDecisionEvent,
        ) -> Pin<Box<dyn Future<Output = CacheWarmingAction> + Send>>
        + Send
        + Sync,
>;

/// One in-flight warming run (TS `ActiveRun`).
struct ActiveRun {
    request: CacheWarmRequest,
    /// False once the session's model or messages no longer match the request.
    is_current: Arc<dyn Fn() -> bool + Send + Sync>,
    ttl_ms: u64,
    delay_ms: u64,
    /// Latest safe time to send this refresh, leaving half the original expiry
    /// margin.
    refresh_deadline_at: u64,
    started_at: u64,
    phase: WarmingPhase,
    next_warm_at: u64,
    /// Set while a refresh that an extension forced is in flight.
    extension_override: bool,
    /// Generation counter invalidating the timer of a cancelled run.
    generation: u64,
}

/// Keeps one prompt cache entry alive by re-sending its request with a
/// one-token output cap before the entry expires. `start` replaces any previous
/// run; warm requests never extend the fixed safety windows
/// (TS `CacheWarmer`).
pub struct CacheWarmer {
    run: Option<ActiveRun>,
    inactive: CacheWarmingStatus,
    models: Arc<dyn CacheWarmModels>,
    session: Arc<dyn CacheWarmSession>,
    get_mode: Arc<dyn Fn() -> CacheWarmingMode + Send + Sync>,
    /// Lets extensions override `event.action`; failures fall back to pi's
    /// decision.
    decide: DecideFn,
    /// Called with the persisted usage entry after each successful refresh.
    pub on_warmed: Option<WarmedFn>,
    /// Set by [`CacheWarmer::attach`] so timers can re-enter the warmer.
    me: Weak<AsyncMutex<CacheWarmer>>,
    generation: u64,
}

impl CacheWarmer {
    /// Build a warmer. The caller keeps it in an `Arc<AsyncMutex<…>>` and calls
    /// [`CacheWarmer::attach`] so timers can re-enter.
    #[must_use]
    pub fn new(
        models: Arc<dyn CacheWarmModels>,
        session: Arc<dyn CacheWarmSession>,
        get_mode: Arc<dyn Fn() -> CacheWarmingMode + Send + Sync>,
        decide: DecideFn,
    ) -> Self {
        Self {
            run: None,
            inactive: CacheWarmingStatus {
                state: WarmingState::Inactive,
                reason: Some("waiting for first request".to_string()),
                next_warm_at: None,
                decision: None,
                extension_override: None,
            },
            models,
            session,
            get_mode,
            decide,
            on_warmed: None,
            me: Weak::new(),
            generation: 0,
        }
    }

    /// Give the warmer a handle to itself, so armed timers can fire.
    pub fn attach(&mut self, me: &Arc<AsyncMutex<CacheWarmer>>) {
        self.me = Arc::downgrade(me);
    }

    /// What `/session` reports (TS `get status`).
    #[must_use]
    pub fn status(&self) -> CacheWarmingStatus {
        if (self.get_mode)() == CacheWarmingMode::Off {
            return CacheWarmingStatus {
                state: WarmingState::Inactive,
                reason: Some("cache warming disabled".to_string()),
                next_warm_at: None,
                decision: None,
                extension_override: None,
            };
        }
        let Some(run) = &self.run else {
            return self.inactive.clone();
        };
        if !(run.is_current)() {
            return inactive("conversation context changed");
        }
        let decision = self.evaluate(run);
        let refreshing = run.refresh_deadline_at == 0;
        if !decision.economics_available && !refreshing {
            return inactive("cache economics unavailable");
        }
        CacheWarmingStatus {
            state: if refreshing {
                WarmingState::Refreshing
            } else {
                WarmingState::Scheduled
            },
            reason: None,
            next_warm_at: Some(run.next_warm_at),
            decision: Some(decision),
            extension_override: Some(run.extension_override),
        }
    }

    /// Keep the prompt cache entry written by `request` warm while `is_current`
    /// holds (TS `start`).
    pub fn start(
        &mut self,
        request: CacheWarmRequest,
        is_current: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        self.clear_run();
        if (self.get_mode)() == CacheWarmingMode::Off {
            self.stop("cache warming disabled", None);
            return;
        }
        if !is_replayable(&request.model, Some(&request.options)) {
            self.stop("request cannot be replayed safely", None);
            return;
        }
        let Some(ttl_ms) = get_prompt_cache_ttl_ms(&request.model, Some(&request.options)) else {
            let reason = if request.options.base.cache_retention == Some(CacheRetention::None) {
                "request disabled prompt caching"
            } else {
                "cache lifetime unavailable"
            };
            self.stop(reason, None);
            return;
        };
        let Some(delay_ms) = get_cache_warming_delay_ms(ttl_ms) else {
            self.stop("cache lifetime unavailable", None);
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.run = Some(ActiveRun {
            request,
            is_current,
            ttl_ms,
            delay_ms,
            refresh_deadline_at: 0,
            started_at: now_ms(),
            phase: WarmingPhase::Streaming,
            next_warm_at: 0,
            extension_override: false,
            generation,
        });
        if let Some(run) = self.run.take() {
            self.schedule(run);
        }
    }

    /// The agent run that started warming settled (TS `onAgentSettled`).
    pub fn on_agent_settled(&mut self) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        if (self.get_mode)() == CacheWarmingMode::Streaming {
            self.stop("agent run settled", None);
            return;
        }
        run.phase = WarmingPhase::Idle;
        let deadline = run.started_at + MAX_IDLE_WARMING_AGE_MS;
        if run.next_warm_at > deadline || now_ms() >= deadline {
            self.stop("30-minute idle safety limit reached", None);
        }
    }

    /// Reconcile an active run after the persisted warming mode changes
    /// (TS `onModeChanged`).
    pub fn on_mode_changed(&mut self) {
        let Some(run) = self.run.as_ref() else {
            return;
        };
        if let Some(reason) = self.get_mode_stop_reason(run) {
            self.stop(&reason, None);
        }
    }

    /// Stop warming (TS `cancel`).
    pub fn cancel(&mut self) {
        self.stop("inactive", None);
    }

    fn clear_run(&mut self) {
        // Dropping the run invalidates its timer: the scheduled task checks the
        // generation before refreshing.
        self.run = None;
        self.generation = self.generation.wrapping_add(1);
    }

    fn stop(
        &mut self,
        reason: &str,
        stopped: Option<(Option<CacheWarmingDecision>, Option<bool>)>,
    ) {
        self.clear_run();
        let (decision, extension_override) = stopped.unwrap_or((None, None));
        self.inactive = CacheWarmingStatus {
            state: WarmingState::Inactive,
            reason: Some(reason.to_string()),
            next_warm_at: None,
            decision,
            extension_override,
        };
    }

    /// Arm the next refresh, unless it would land past the phase's safety
    /// window (TS `schedule`).
    fn schedule(&mut self, run: ActiveRun) {
        let mut run = run;
        run.extension_override = false;
        let now = now_ms();
        run.next_warm_at = now + run.delay_ms;
        // A timer can run late after sleep or event-loop blockage. Keep half of
        // the planned pre-expiry margin for that delay and request dispatch; a
        // late refresh is likely a full-price cache write, not a cache warm.
        run.refresh_deadline_at = run.next_warm_at + (run.ttl_ms - run.delay_ms) / 2;
        let horizon = if run.phase == WarmingPhase::Idle {
            MAX_IDLE_WARMING_AGE_MS
        } else {
            MAX_WARMING_AGE_MS
        };
        let deadline = run.started_at + horizon;
        if run.next_warm_at > deadline || now >= deadline {
            let reason = if run.phase == WarmingPhase::Idle {
                "30-minute idle safety limit reached"
            } else {
                "one-hour safety limit reached"
            };
            self.stop(reason, None);
            return;
        }
        let wait_ms = run.next_warm_at.saturating_sub(now);
        let generation = run.generation;
        self.run = Some(run);
        let Some(me) = self.me.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            let mut warmer = me.lock().await;
            warmer.refresh(generation).await;
        });
    }

    /// Send one refresh if the run is still current (TS `refresh`).
    async fn refresh(&mut self, generation: u64) {
        if self.run.as_ref().map(|r| r.generation) != Some(generation) {
            return;
        }
        if !self.validate_run(generation) {
            return;
        }
        if self.refresh_deadline_missed(generation) {
            return;
        }
        let decision = match self.run.as_ref() {
            Some(run) => self.evaluate(run),
            None => return,
        };
        let (warm_cost, miss_cost, continuation_probability) = (
            decision.warm_cost,
            decision.miss_cost,
            decision.continuation_probability,
        );
        let mut action = decision.action;
        // Extension failures fall back to pi's own decision.
        action = (self.decide)(CacheWarmingDecisionEvent {
            kind: "cache_warming_decision".to_string(),
            warm_cost,
            miss_cost,
            continuation_probability,
            action,
        })
        .await;

        if !self.validate_run(generation) || self.refresh_deadline_missed(generation) {
            return;
        }
        let extension_override = action != decision.action;
        if action == CacheWarmingAction::Stop {
            let reason = if extension_override {
                "stopped by extension"
            } else if decision.economics_available {
                "expected savings below threshold"
            } else {
                "cache economics unavailable"
            };
            self.stop(
                reason,
                Some((Some(decision), Some(extension_override))),
            );
            return;
        }

        if let Some(run) = self.run.as_mut() {
            run.extension_override = extension_override;
        }
        let Some(run) = self.run.as_ref() else {
            return;
        };
        let model = run.request.model.clone();
        let context = run.request.context.clone();
        let mut options = run.request.options.clone();
        options.base.max_tokens = Some(1);
        options.base.max_retries = Some(0);
        let result = self.models.stream_simple(model, context, options).await;

        if !self.validate_run(generation) {
            return;
        }
        if let Ok(message) = result {
            if message.stop_reason != StopReason::Error
                && message.stop_reason != StopReason::Aborted
            {
                let entry = self.session.append_usage(
                    "cache_warm",
                    &message.provider,
                    message
                        .response_model
                        .as_deref()
                        .unwrap_or(message.model.as_str()),
                    message.usage.clone(),
                    if extension_override {
                        Some("extension override")
                    } else {
                        None
                    },
                );
                if let Some(on_warmed) = &self.on_warmed {
                    on_warmed(&entry);
                }
            }
        }
        if let Some(run) = self.run.take() {
            self.schedule(run);
        }
    }

    fn refresh_deadline_missed(&mut self, generation: u64) -> bool {
        let missed = self
            .run
            .as_ref()
            .filter(|r| r.generation == generation)
            .is_some_and(|run| now_ms() > run.refresh_deadline_at);
        if missed {
            self.stop("cache refresh deadline missed", None);
        }
        missed
    }

    fn validate_run(&mut self, generation: u64) -> bool {
        let Some(run) = self.run.as_ref().filter(|r| r.generation == generation) else {
            return false;
        };
        let reason = self
            .get_mode_stop_reason(run)
            .or_else(|| (!(run.is_current)()).then(|| "conversation context changed".to_string()));
        let Some(reason) = reason else {
            return true;
        };
        self.stop(&reason, None);
        false
    }

    fn get_mode_stop_reason(&self, run: &ActiveRun) -> Option<String> {
        match (self.get_mode)() {
            CacheWarmingMode::Off => Some("cache warming disabled".to_string()),
            CacheWarmingMode::Streaming if run.phase == WarmingPhase::Idle => {
                Some("agent run settled".to_string())
            }
            _ => None,
        }
    }

    /// Price the refresh against the next real request (TS `evaluate`).
    fn evaluate(&self, run: &ActiveRun) -> CacheWarmingDecision {
        let model = &run.request.model;
        let prompt_tokens = last_prompt_tokens(&self.session.branch_entries());
        let cache_hit_cost = price(
            model,
            TokenMix {
                cache_read: Some(prompt_tokens),
                ..Default::default()
            },
        );
        let cache_miss_cost = price(
            model,
            if model.cost.cache_write > 0.0 {
                TokenMix {
                    cache_write: Some(prompt_tokens),
                    ..Default::default()
                }
            } else {
                TokenMix {
                    input: Some(prompt_tokens),
                    ..Default::default()
                }
            },
        );
        let warm_cost = price(
            model,
            TokenMix {
                cache_read: Some(prompt_tokens),
                output: Some(1),
                ..Default::default()
            },
        );
        let miss_cost = (cache_miss_cost - cache_hit_cost).max(0.0);
        let continuation_probability = if run.phase == WarmingPhase::Idle {
            IDLE_CONTINUATION_PROBABILITY
        } else {
            1.0
        };
        let economics_available = prompt_tokens > 0 && (cache_hit_cost > 0.0 || cache_miss_cost > 0.0);
        let expected_savings = continuation_probability * miss_cost - warm_cost;
        CacheWarmingDecision {
            phase: run.phase,
            warm_cost,
            miss_cost,
            continuation_probability,
            expected_savings,
            economics_available,
            action: if expected_savings >= CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS {
                CacheWarmingAction::Warm
            } else {
                CacheWarmingAction::Stop
            },
        }
    }
}

fn inactive(reason: &str) -> CacheWarmingStatus {
    CacheWarmingStatus {
        state: WarmingState::Inactive,
        reason: Some(reason.to_string()),
        next_warm_at: None,
        decision: None,
        extension_override: None,
    }
}

fn format_dollars(value: f64) -> String {
    if value < 0.0 {
        format!("-${:.3}", value.abs())
    } else {
        format!("${value:.3}")
    }
}

/// One-line economics of a decision (TS `formatCacheWarmingEconomics`).
#[must_use]
pub fn format_cache_warming_economics(decision: &CacheWarmingDecision) -> String {
    if !decision.economics_available {
        return "cache economics unavailable".to_string();
    }
    let probability = (decision.continuation_probability * 100.0).round() as i64;
    let probability_text = if probability >= 100 {
        format!("cache read {} vs miss {}", format_dollars(decision.warm_cost), format_dollars(decision.miss_cost))
    } else {
        format!(
            "{}% chance of a request within the window: {} to warm vs {} if it misses",
            probability,
            format_dollars(decision.warm_cost),
            format_dollars(decision.miss_cost)
        )
    };
    format!(
        "{probability_text} · expected {}",
        format_dollars(decision.expected_savings)
    )
}

/// When the next refresh is due (TS `formatCacheWarmingDecisionTime`).
#[must_use]
pub fn format_cache_warming_decision_time(next_warm_at: Option<u64>, now: u64) -> String {
    let Some(next_warm_at) = next_warm_at else {
        return String::new();
    };
    let remaining_ms = next_warm_at.saturating_sub(now);
    let seconds = remaining_ms / 1000;
    if remaining_ms == 0 {
        "now".to_string()
    } else if seconds < 60 {
        format!("in {seconds}s")
    } else {
        format!("in {}m {}s", seconds / 60, seconds % 60)
    }
}

/// Human-readable warming status for `/session` (TS `formatCacheWarmingStatus`).
#[must_use]
pub fn format_cache_warming_status(status: &CacheWarmingStatus, now: u64) -> String {
    match status.state {
        WarmingState::Inactive => {
            format!("inactive ({})", status.reason.as_deref().unwrap_or("unknown"))
        }
        WarmingState::Scheduled => {
            let decision = status
                .decision
                .as_ref()
                .map(format_cache_warming_economics)
                .unwrap_or_default();
            format!(
                "scheduled {} · {decision}",
                format_cache_warming_decision_time(status.next_warm_at, now)
            )
        }
        WarmingState::Refreshing => {
            let decision = status
                .decision
                .as_ref()
                .map(format_cache_warming_economics)
                .unwrap_or_default();
            format!("refreshing · {decision}")
        }
    }
}

/// Notice text for one recorded refresh (TS `formatCacheWarmingUsage`).
#[must_use]
pub fn format_cache_warming_usage(usage: &Usage, note: Option<&str>) -> String {
    let mut text = format!(
        "Cache warm: {} read, {} written, {}",
        usage.cache_read,
        usage.cache_write,
        format_dollars(usage.cost.total)
    );
    if let Some(note) = note {
        text.push_str(&format!(" ({note})"));
    }
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::core::session_manager::{SessionEntry, SessionManager};
    use pi_ai::types::AssistantMessage;
    use std::sync::Mutex;
    use pi_ai::types::{ModelCost, ModelPromptCache};

    fn now() -> u64 {
        now_ms()
    }

    fn empty_context() -> Context {
        Context {
            system_prompt: None,
            messages: Vec::new(),
            tools: None,
        }
    }

    fn anthropic_model() -> Model {
        Model {
            id: "claude-sonnet-5".into(),
            name: "Claude Sonnet 5".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
                tiers: vec![],
            },
            prompt_cache: Some(ModelPromptCache {
                short: Some(300),
                long: Some(3600),
            }),
            context_window: 200_000,
            max_tokens: 8192,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    /// A session surface backed by a real manager, so branch reads and usage
    /// appends behave as in production.
    struct RealSession(Mutex<SessionManager>);

    impl CacheWarmSession for RealSession {
        fn branch_entries(&self) -> Vec<SessionEntry> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_branch(None)
        }
        fn append_usage(
            &self,
            kind: &str,
            provider: &str,
            model: &str,
            usage: Usage,
            note: Option<&str>,
        ) -> SessionEntry {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .append_usage(kind, provider, model, usage, note)
        }
    }

    struct NoModels;

    impl CacheWarmModels for NoModels {
        fn stream_simple(
            &self,
            _model: Model,
            _context: Context,
            _options: SimpleStreamOptions,
        ) -> Pin<Box<dyn Future<Output = Result<AssistantMessage, String>> + Send>> {
            Box::pin(async { Err("not used in this test".to_string()) })
        }
    }

    fn session_with_prompt(prompt_tokens: u64) -> Arc<dyn CacheWarmSession> {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new(
            "/tmp/test",
            dir.path().to_str().unwrap(),
            None,
            false,
            None,
        );
        mgr.append_message(serde_json::json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "usage": {"input": prompt_tokens, "cacheRead": 0, "cacheWrite": 0,
                      "output": 0, "totalTokens": prompt_tokens},
            "timestamp": 1
        }));
        Arc::new(RealSession(Mutex::new(mgr)))
    }

    fn warmer(
        session: Arc<dyn CacheWarmSession>,
        mode: CacheWarmingMode,
    ) -> CacheWarmer {
        CacheWarmer::new(
            Arc::new(NoModels),
            session,
            Arc::new(move || mode),
            Arc::new(|event| Box::pin(async move { event.action })),
        )
    }

    /// Refresh timing keeps a ten-second margin and refuses lifetimes too short
    /// to leave one (TS `getCacheWarmingDelayMs`).
    #[test]
    fn warming_delay_preserves_a_margin() {
        assert_eq!(get_cache_warming_delay_ms(10_000), None);
        assert_eq!(get_cache_warming_delay_ms(5_000), None);
        // 90% of the TTL while the ten-second margin is larger.
        assert_eq!(get_cache_warming_delay_ms(300_000), Some(270_000));
        // The margin wins near the boundary: min(ttl*0.9, ttl-10s).
        assert_eq!(get_cache_warming_delay_ms(50_000), Some(40_000));
    }

    /// The TTL comes from the retention tier the request used
    /// (TS `getPromptCacheTtlMs`).
    #[test]
    fn prompt_cache_ttl_follows_retention() {
        let model = anthropic_model();
        let opts = |retention: Option<CacheRetention>, env: Option<&str>| {
            let mut o = SimpleStreamOptions::default();
            o.base.cache_retention = retention;
            if let Some(value) = env {
                o.base.env = Some(
                    [("PI_CACHE_RETENTION".to_string(), value.to_string())]
                        .into_iter()
                        .collect(),
                );
            }
            o
        };
        assert_eq!(
            get_prompt_cache_ttl_ms(&model, Some(&opts(None, None))),
            Some(300_000)
        );
        assert_eq!(
            get_prompt_cache_ttl_ms(&model, Some(&opts(None, Some("long")))),
            Some(3_600_000)
        );
        assert_eq!(
            get_prompt_cache_ttl_ms(&model, Some(&opts(Some(CacheRetention::Long), None))),
            Some(3_600_000)
        );
        assert_eq!(
            get_prompt_cache_ttl_ms(&model, Some(&opts(Some(CacheRetention::None), None))),
            None
        );

        let mut unknown = anthropic_model();
        unknown.prompt_cache = None;
        assert_eq!(get_prompt_cache_ttl_ms(&unknown, None), None);
    }

    /// Starting a run without a known cache lifetime stops immediately with the
    /// matching reason (TS `start`).
    #[test]
    fn start_stops_when_the_cache_lifetime_is_unknown() {
        let session = session_with_prompt(1000);
        let mut w = warmer(session.clone(), CacheWarmingMode::Streaming);
        let mut model = anthropic_model();
        model.prompt_cache = None;
        w.start(
            CacheWarmRequest {
                model,
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        let status = w.status();
        assert_eq!(status.state, WarmingState::Inactive);
        assert_eq!(status.reason.as_deref(), Some("cache lifetime unavailable"));

        // With a lifetime it arms a refresh instead.
        let mut w = warmer(session, CacheWarmingMode::Streaming);
        w.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        assert_eq!(w.status().state, WarmingState::Scheduled);
    }

    /// Warming is off when the mode says so, and setting the mode to
    /// `streaming` after settling stops an idle run (TS `onModeChanged`,
    /// `onAgentSettled`).
    #[test]
    fn mode_changes_reconcile_an_active_run() {
        let session = session_with_prompt(1000);
        let mut w = warmer(session.clone(), CacheWarmingMode::Idle);
        w.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        w.on_agent_settled();
        assert_eq!(
            w.status().state,
            WarmingState::Scheduled,
            "idle mode keeps warming after the run settles"
        );

        // Dropping back to streaming stops the idle run.
        let mut w2 = warmer(session.clone(), CacheWarmingMode::Idle);
        w2.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        w2.on_agent_settled();
        w2.get_mode = Arc::new(|| CacheWarmingMode::Streaming);
        w2.on_mode_changed();
        let status = w2.status();
        assert_eq!(status.state, WarmingState::Inactive);
        assert_eq!(status.reason.as_deref(), Some("agent run settled"));

        // Off stops immediately at start.
        let mut w3 = warmer(session, CacheWarmingMode::Off);
        w3.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        assert_eq!(
            w3.status().reason.as_deref(),
            Some("cache warming disabled")
        );
    }

    /// A conversation that moved on stops warming (TS `validateRun`).
    #[test]
    fn stale_context_stops_warming() {
        let session = session_with_prompt(1000);
        let mut w = warmer(session, CacheWarmingMode::Streaming);
        let current = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = current.clone();
        w.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(move || flag.load(std::sync::atomic::Ordering::SeqCst)),
        );
        current.store(false, std::sync::atomic::Ordering::SeqCst);
        let status = w.status();
        assert_eq!(status.state, WarmingState::Inactive);
        assert_eq!(status.reason.as_deref(), Some("conversation context changed"));
    }

    /// Prompt size comes from the last assistant usage on the branch, and the
    /// economics follow from it (TS `lastPromptTokens` / `evaluate`).
    #[test]
    fn economics_use_the_last_prompt_size() {
        let session = session_with_prompt(1000);
        assert_eq!(last_prompt_tokens(&session.branch_entries()), 1000);

        // Idle mode, so settling keeps the run alive for the second half.
        let mut w = warmer(session, CacheWarmingMode::Idle);
        w.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        let run = w.run.as_ref().unwrap();
        assert_eq!(run.phase, WarmingPhase::Streaming);
        let decision = w.evaluate(run);
        assert!(decision.economics_available);
        assert_eq!(decision.continuation_probability, 1.0);
        // 1000 cache-read tokens at $0.30/M plus one output token at $15/M.
        assert!((decision.warm_cost - (0.0003 + 0.000015)).abs() < 1e-9);
        // The miss writes the prompt at the $3.75/M cache-write rate minus the
        // $0.30/M read it would have paid.
        assert!((decision.miss_cost - (0.00375 - 0.0003)).abs() < 1e-9);
        // 1000 prompt tokens miss by three tenths of a cent: below the $0.05
        // threshold, so pi declines to warm.
        assert!(decision.expected_savings < CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS);
        assert_eq!(decision.action, CacheWarmingAction::Stop);

        // Idle warming discounts continuation to the measured probability,
        // which makes the same run less worthwhile.
        w.on_agent_settled();
        let run = w.run.as_ref().unwrap();
        let idle = w.evaluate(run);
        assert_eq!(idle.phase, WarmingPhase::Idle);
        assert!((idle.continuation_probability - IDLE_CONTINUATION_PROBABILITY).abs() < 1e-9);
        assert!((idle.expected_savings - (IDLE_CONTINUATION_PROBABILITY * idle.miss_cost - idle.warm_cost)).abs() < 1e-9);
        assert!(
            idle.expected_savings < CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS,
            "a small idle prompt is not worth warming"
        );
    }

    /// A prompt large enough for the miss to cost more than five cents is worth
    /// warming (TS `evaluate`).
    #[test]
    fn large_prompts_are_worth_warming() {
        let session = session_with_prompt(20_000);
        let mut w = warmer(session, CacheWarmingMode::Streaming);
        w.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        let run = w.run.as_ref().unwrap();
        let decision = w.evaluate(run);
        assert!(decision.economics_available);
        assert!(
            decision.expected_savings >= CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS,
            "expected savings {} below the threshold",
            decision.expected_savings
        );
        assert_eq!(decision.action, CacheWarmingAction::Warm);
    }

    /// A refresh replays the request with a one-token cap, records the usage,
    /// and rearms (TS `refresh`).
    #[tokio::test]
    async fn refresh_replays_and_records_usage() {
        struct Recording(Arc<Mutex<Vec<SimpleStreamOptions>>>);
        impl CacheWarmModels for Recording {
            fn stream_simple(
                &self,
                model: Model,
                _context: Context,
                options: SimpleStreamOptions,
            ) -> Pin<Box<dyn Future<Output = Result<AssistantMessage, String>> + Send>> {
                self.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(options);
                Box::pin(async move {
                    let usage = Usage {
                        cache_read: 20_000,
                        cost: pi_ai::types::UsageCost {
                            total: 0.006,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    Ok(AssistantMessage {
                        content: Vec::new(),
                        api: model.api,
                        provider: model.provider,
                        model: model.id,
                        response_model: Some("claude-sonnet-5-20260101".into()),
                        response_id: None,
                        diagnostics: None,
                        usage,
                        stop_reason: StopReason::Stop,
                        error_message: None,
                        raw_stop_reason: None,
                        end_turn: None,
                        timestamp: 0,
                    })
                })
            }
        }

        let session = session_with_prompt(20_000);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let warmed = Arc::new(Mutex::new(Vec::new()));
        let mut w = CacheWarmer::new(
            Arc::new(Recording(calls.clone())),
            session.clone(),
            Arc::new(|| CacheWarmingMode::Streaming),
            Arc::new(|event| Box::pin(async move { event.action })),
        );
        {
            let warmed = warmed.clone();
            w.on_warmed = Some(Arc::new(move |entry| {
                warmed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(entry.clone());
            }));
        }
        w.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        let generation = w.run.as_ref().unwrap().generation;
        w.refresh(generation).await;

        let sent = calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(sent.len(), 1, "one warm request");
        assert_eq!(sent[0].base.max_tokens, Some(1), "replay caps output at one token");
        assert_eq!(sent[0].base.max_retries, Some(0));
        drop(sent);

        let entry = warmed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("on_warmed must receive the usage entry"));
        match entry {
            SessionEntry::Usage {
                kind, model, note, ..
            } => {
                assert_eq!(kind, "cache_warm");
                assert_eq!(model, "claude-sonnet-5-20260101", "response model wins");
                assert_eq!(note, None);
            }
            other => panic!("expected a usage entry, got {other:?}"),
        }
        assert!(
            w.run.is_some(),
            "a completed refresh rearms for the next window"
        );
    }

    /// An extension can stop warming, and the status records the override
    /// (TS `refresh` with a `stop` decision).
    #[tokio::test]
    async fn extension_can_stop_warming() {
        let session = session_with_prompt(20_000);
        let mut w = CacheWarmer::new(
            Arc::new(NoModels),
            session,
            Arc::new(|| CacheWarmingMode::Streaming),
            Arc::new(|_event| Box::pin(async { CacheWarmingAction::Stop })),
        );
        w.start(
            CacheWarmRequest {
                model: anthropic_model(),
                context: empty_context(),
                options: SimpleStreamOptions::default(),
            },
            Arc::new(|| true),
        );
        let generation = w.run.as_ref().unwrap().generation;
        w.refresh(generation).await;

        let status = w.status();
        assert_eq!(status.state, WarmingState::Inactive);
        assert_eq!(status.reason.as_deref(), Some("stopped by extension"));
        assert_eq!(status.extension_override, Some(true));
    }

    /// Formatting helpers match the strings `/session` shows.
    #[test]
    fn formatting_matches_ts() {
        assert_eq!(format_dollars(0.5), "$0.500");
        assert_eq!(format_dollars(-0.25), "-$0.250");
        assert_eq!(format_cache_warming_decision_time(None, now()), "");
        assert_eq!(
            format_cache_warming_decision_time(Some(now() + 30_000), now()),
            "in 30s"
        );
        assert_eq!(
            format_cache_warming_decision_time(Some(now() + 90_000), now()),
            "in 1m 30s"
        );
        let inactive = inactive("cache lifetime unavailable");
        assert_eq!(
            format_cache_warming_status(&inactive, now()),
            "inactive (cache lifetime unavailable)"
        );
    }
}
