//! 订阅健康状态机（设计稿 §6）。
//!
//! 每次转换都是原子的：调用方持有 `SubscriptionRuntime` 的写锁，调用一个
//! `apply(...)`；副作用仅限于更新 DB 和发事件。
//!
//! 冷却定时器用 `tokio::spawn` + `sleep_until` 实现：到期后回调再次
//! 调用 `apply(CooldownExpired)`。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::AppResult;
use crate::subscription::model::{SubscriptionRuntime, SubscriptionState};
use crate::subscription::store;

/// 外部事件驱动状态机。
#[derive(Debug, Clone)]
pub enum Event {
    RequestSucceeded,
    HttpStatus(u16),
    NetworkError,
    CooldownExpired,
    UserEnable,
    UserDisable,
    UserUpdateKey,
}

/// 状态转换结果。
#[derive(Debug, Clone, Copy)]
pub struct Transition {
    pub state_changed: bool,
    pub new_state: SubscriptionState,
    pub schedule_cooldown: Option<Duration>,
}

pub fn apply<'a>(
    pool: &'a SqlitePool,
    app: &'a AppHandle,
    rt: Arc<RwLock<SubscriptionRuntime>>,
    event: Event,
) -> Pin<Box<dyn Future<Output = AppResult<Transition>> + Send + 'a>> {
    Box::pin(async move {
        let (transition, id, error_message_update) = {
            let mut guard = rt.write().await;
            transition(&mut guard, &event)
        };

        // 持久化（§6.5）
        if transition.state_changed {
            let is_auth_failed = transition.new_state == SubscriptionState::AuthFailed;
            store::update_auth_failed(
                pool,
                &id,
                is_auth_failed,
                error_message_update.as_deref(),
            )
            .await?;

            let _ = app.emit("subscription_state_changed", &id.to_string());
        }

        // 冷却定时器
        if let Some(duration) = transition.schedule_cooldown {
            let pool = pool.clone();
            let app = app.clone();
            let rt = rt.clone();
            tokio::spawn(async move {
                tokio::time::sleep(duration).await;
                if let Err(e) = apply(&pool, &app, rt, Event::CooldownExpired).await {
                    warn!(?e, "cooldown apply 失败");
                }
            });
        }

        Ok(transition)
    })
}

fn transition(
    rt: &mut SubscriptionRuntime,
    event: &Event,
) -> (Transition, Uuid, Option<String>) {
    let now = Utc::now();
    let prev_state = rt.state;
    let id = rt.row.id;
    let mut last_error_update: Option<String> = None;

    let new_state = match (rt.state, event) {
        // healthy ==================================
        (SubscriptionState::Healthy, Event::RequestSucceeded) => {
            rt.consecutive_errors = 0;
            rt.transient_error_level = 0;
            rt.last_error_message = None;
            SubscriptionState::Healthy
        }
        (SubscriptionState::Healthy, Event::HttpStatus(s)) => {
            classify_http(*s, rt, &mut last_error_update)
        }
        (SubscriptionState::Healthy, Event::NetworkError) => {
            bump_transient(rt, "network error", &mut last_error_update)
        }
        (SubscriptionState::Healthy, Event::UserDisable) => {
            rt.row.enabled = false;
            SubscriptionState::Disabled
        }

        // rate_limited / quota_exhausted ============
        (SubscriptionState::RateLimited | SubscriptionState::QuotaExhausted, Event::CooldownExpired) => {
            rt.consecutive_errors = 0;
            SubscriptionState::Healthy
        }
        (SubscriptionState::RateLimited | SubscriptionState::QuotaExhausted, Event::UserEnable) => {
            rt.cooldown_until = None;
            SubscriptionState::Healthy
        }

        // transient_error ==========================
        (SubscriptionState::TransientError, Event::CooldownExpired) => {
            rt.consecutive_errors = 0;
            SubscriptionState::Healthy
        }
        (SubscriptionState::TransientError, Event::RequestSucceeded) => {
            rt.consecutive_errors = 0;
            rt.transient_error_level = 0;
            SubscriptionState::Healthy
        }

        // auth_failed ==============================
        (SubscriptionState::AuthFailed, Event::UserUpdateKey) => {
            rt.row.is_auth_failed = false;
            last_error_update = None;
            rt.last_error_message = None;
            SubscriptionState::Healthy
        }

        // disabled =================================
        (SubscriptionState::Disabled, Event::UserEnable) => {
            rt.row.enabled = true;
            rt.consecutive_errors = 0;
            rt.transient_error_level = 0;
            rt.cooldown_until = None;
            rt.last_error_message = None;
            SubscriptionState::Healthy
        }

        // 用户禁用是所有状态的汇聚点
        (_, Event::UserDisable) => {
            rt.row.enabled = false;
            SubscriptionState::Disabled
        }

        // 其余情况保持状态
        _ => rt.state,
    };

    let mut schedule_cooldown = None;
    if new_state != prev_state {
        rt.state = new_state;
        rt.state_entered_at = now;
        rt.row.updated_at = now;
        match new_state {
            SubscriptionState::RateLimited | SubscriptionState::QuotaExhausted => {
                rt.cooldown_until = Some(now + chrono::Duration::seconds(60));
                schedule_cooldown = Some(Duration::from_secs(60));
            }
            SubscriptionState::TransientError => {
                let secs = match rt.transient_error_level {
                    0 => 30,
                    1 => 60,
                    2 => 120,
                    _ => 300,
                };
                rt.cooldown_until = Some(now + chrono::Duration::seconds(secs));
                schedule_cooldown = Some(Duration::from_secs(secs as u64));
                rt.transient_error_level = rt.transient_error_level.saturating_add(1);
            }
            SubscriptionState::AuthFailed => {
                rt.row.is_auth_failed = true;
                rt.cooldown_until = None;
            }
            SubscriptionState::Healthy => {
                rt.cooldown_until = None;
            }
            SubscriptionState::Disabled => {
                rt.cooldown_until = None;
            }
        }

        info!(
            id = %rt.row.id,
            ?prev_state,
            ?new_state,
            "subscription state transition"
        );
    }

    (
        Transition {
            state_changed: new_state != prev_state,
            new_state,
            schedule_cooldown,
        },
        id,
        last_error_update,
    )
}

fn classify_http(
    status: u16,
    rt: &mut SubscriptionRuntime,
    last_error: &mut Option<String>,
) -> SubscriptionState {
    match status {
        200..=299 => {
            rt.consecutive_errors = 0;
            rt.transient_error_level = 0;
            SubscriptionState::Healthy
        }
        400 => {
            // 请求内容问题，不计入错误（§5.2）
            SubscriptionState::Healthy
        }
        401 | 403 => {
            *last_error = Some(format!("HTTP {status}: 凭证失效"));
            rt.last_error_message = last_error.clone();
            SubscriptionState::AuthFailed
        }
        429 => {
            *last_error = Some("HTTP 429: 限流".to_string());
            rt.last_error_message = last_error.clone();
            // MVP: rate_limited 与 quota_exhausted 合并（§6.1）
            SubscriptionState::RateLimited
        }
        500..=599 => bump_transient(rt, &format!("HTTP {status}"), last_error),
        _ => SubscriptionState::Healthy,
    }
}

fn bump_transient(
    rt: &mut SubscriptionRuntime,
    reason: &str,
    last_error: &mut Option<String>,
) -> SubscriptionState {
    rt.consecutive_errors = rt.consecutive_errors.saturating_add(1);
    *last_error = Some(reason.to_string());
    rt.last_error_message = last_error.clone();
    if rt.consecutive_errors >= 3 {
        SubscriptionState::TransientError
    } else {
        SubscriptionState::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::model::{ModelSlots, SubscriptionRow};

    fn runtime() -> SubscriptionRuntime {
        let row = SubscriptionRow {
            id: Uuid::new_v4(),
            provider_id: "anthropic".into(),
            endpoint_id: "api_paygo".into(),
            display_name: "test".into(),
            api_key: "test-key".into(),
            model_slots: ModelSlots {
                opus: "a".into(),
                sonnet: "b".into(),
                haiku: "c".into(),
            },
            enabled: true,
            is_auth_failed: false,
            last_error_message: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        SubscriptionRuntime::from_row(row)
    }

    #[test]
    fn healthy_to_rate_limited_on_429() {
        let mut rt = runtime();
        let (t, _, _) = transition(&mut rt, &Event::HttpStatus(429));
        assert!(t.state_changed);
        assert_eq!(rt.state, SubscriptionState::RateLimited);
        assert!(rt.cooldown_until.is_some());
    }

    #[test]
    fn healthy_to_auth_failed_on_401() {
        let mut rt = runtime();
        let (t, _, _) = transition(&mut rt, &Event::HttpStatus(401));
        assert!(t.state_changed);
        assert_eq!(rt.state, SubscriptionState::AuthFailed);
    }

    #[test]
    fn transient_error_after_three_fives() {
        let mut rt = runtime();
        for _ in 0..2 {
            let (t, _, _) = transition(&mut rt, &Event::HttpStatus(500));
            assert!(!t.state_changed);
            assert_eq!(rt.state, SubscriptionState::Healthy);
        }
        let (t, _, _) = transition(&mut rt, &Event::HttpStatus(500));
        assert!(t.state_changed);
        assert_eq!(rt.state, SubscriptionState::TransientError);
    }

    #[test]
    fn exponential_backoff_levels() {
        let mut rt = runtime();
        for _ in 0..3 {
            transition(&mut rt, &Event::HttpStatus(500));
        }
        assert_eq!(rt.state, SubscriptionState::TransientError);
        let cooldown = rt.cooldown_until.unwrap();
        let delta = (cooldown - Utc::now()).num_seconds();
        assert!((28..=32).contains(&delta), "first cooldown ~30s, got {delta}");
    }

    #[test]
    fn cooldown_expired_returns_to_healthy() {
        let mut rt = runtime();
        for _ in 0..3 {
            transition(&mut rt, &Event::HttpStatus(500));
        }
        let (t, _, _) = transition(&mut rt, &Event::CooldownExpired);
        assert!(t.state_changed);
        assert_eq!(rt.state, SubscriptionState::Healthy);
    }

    #[test]
    fn disabled_preserved_through_events() {
        let mut rt = runtime();
        transition(&mut rt, &Event::UserDisable);
        assert_eq!(rt.state, SubscriptionState::Disabled);
        let (t, _, _) = transition(&mut rt, &Event::RequestSucceeded);
        assert!(!t.state_changed);
        assert_eq!(rt.state, SubscriptionState::Disabled);
    }

    #[test]
    fn request_success_resets_error_count() {
        let mut rt = runtime();
        transition(&mut rt, &Event::HttpStatus(500));
        transition(&mut rt, &Event::RequestSucceeded);
        assert_eq!(rt.consecutive_errors, 0);
    }

    #[test]
    fn auth_failed_only_clears_on_user_key_update() {
        let mut rt = runtime();
        transition(&mut rt, &Event::HttpStatus(401));
        assert_eq!(rt.state, SubscriptionState::AuthFailed);
        let (t, _, _) = transition(&mut rt, &Event::CooldownExpired);
        assert!(!t.state_changed);
        let (t, _, _) = transition(&mut rt, &Event::UserUpdateKey);
        assert!(t.state_changed);
        assert_eq!(rt.state, SubscriptionState::Healthy);
    }
}
