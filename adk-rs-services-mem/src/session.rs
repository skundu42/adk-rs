//! In-memory [`SessionService`](adk_rs_core::SessionService).

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;

use adk_rs_core::{
    Event, GetSessionConfig, ListSessionsResponse, Session, SessionMeta, SessionService, State,
    StateScope,
};
use adk_rs_error::{Error, Result};

/// Volatile session store. Keys: `(app, user, session_id)`.
#[derive(Debug, Default)]
pub struct InMemorySessionService {
    sessions: DashMap<(String, String, String), Arc<Mutex<Session>>>,
}

impl InMemorySessionService {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn key(app: &str, user: &str, sid: &str) -> (String, String, String) {
        (app.to_string(), user.to_string(), sid.to_string())
    }
}

#[async_trait]
impl SessionService for InMemorySessionService {
    async fn create_session(
        &self,
        app_name: &str,
        user_id: &str,
        state: Option<State>,
        id: Option<&str>,
    ) -> Result<Session> {
        let sid = id
            .map(str::to_string)
            .unwrap_or_else(adk_rs_core::services::new_session_id);
        let key = Self::key(app_name, user_id, &sid);
        if self.sessions.contains_key(&key) {
            return Err(Error::already_exists(format!("session {sid}")));
        }
        let mut s = Session::new(app_name, user_id, sid);
        if let Some(state) = state {
            s.state = state;
        }
        let arc = Arc::new(Mutex::new(s.clone()));
        self.sessions.insert(key, arc);
        Ok(s)
    }

    async fn get_session(
        &self,
        app_name: &str,
        user_id: &str,
        session_id: &str,
        cfg: GetSessionConfig,
    ) -> Result<Option<Session>> {
        let key = Self::key(app_name, user_id, session_id);
        let Some(arc) = self.sessions.get(&key) else {
            return Ok(None);
        };
        let snap = arc.lock().clone();
        Ok(Some(apply_filter(snap, &cfg)))
    }

    async fn list_sessions(&self, app_name: &str, user_id: &str) -> Result<ListSessionsResponse> {
        let sessions: Vec<SessionMeta> = self
            .sessions
            .iter()
            .filter(|kv| kv.key().0 == app_name && kv.key().1 == user_id)
            .map(|kv| {
                let s = kv.value().lock();
                SessionMeta {
                    id: s.id.clone(),
                    app_name: s.app_name.clone(),
                    user_id: s.user_id.clone(),
                    last_update_time: s.last_update_time,
                }
            })
            .collect();
        Ok(ListSessionsResponse { sessions })
    }

    async fn delete_session(&self, app_name: &str, user_id: &str, session_id: &str) -> Result<()> {
        self.sessions
            .remove(&Self::key(app_name, user_id, session_id));
        Ok(())
    }

    async fn append_event(&self, session: &mut Session, mut event: Event) -> Result<Event> {
        if event.partial == Some(true) {
            return Ok(event);
        }
        // Apply temp state in-memory before trimming so subsequent agents
        // can read it for the rest of the invocation.
        for (k, v) in &event.actions.state_delta {
            if StateScope::of(k) == StateScope::Temp {
                session.state.set(k.clone(), v.clone());
            }
        }
        event.actions.state_delta = State::trim_temp_keys(&event.actions.state_delta);
        session.state.apply(&event.actions.state_delta);
        session.last_update_time = adk_rs_core::session::now_secs();
        session.events.push(event.clone());

        // Mirror into our authoritative store.
        let key = Self::key(&session.app_name, &session.user_id, &session.id);
        if let Some(arc) = self.sessions.get(&key) {
            *arc.lock() = session.clone();
        } else {
            self.sessions
                .insert(key, Arc::new(Mutex::new(session.clone())));
        }
        Ok(event)
    }
}

fn apply_filter(mut s: Session, cfg: &GetSessionConfig) -> Session {
    if let Some(after) = cfg.after_timestamp {
        s.events.retain(|e| e.timestamp >= after);
    }
    if let Some(n) = cfg.num_recent_events {
        let drop = s.events.len().saturating_sub(n);
        s.events.drain(..drop);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_get_delete_roundtrip() {
        let svc = InMemorySessionService::new();
        let s = svc
            .create_session("app", "user", None, Some("s1"))
            .await
            .unwrap();
        assert_eq!(s.id, "s1");
        let got = svc
            .get_session("app", "user", "s1", GetSessionConfig::default())
            .await
            .unwrap();
        assert!(got.is_some());
        svc.delete_session("app", "user", "s1").await.unwrap();
        let gone = svc
            .get_session("app", "user", "s1", GetSessionConfig::default())
            .await
            .unwrap();
        assert!(gone.is_none());
    }

    #[tokio::test]
    async fn append_event_persists_and_applies_state() {
        let svc = InMemorySessionService::new();
        let mut s = svc.create_session("app", "user", None, None).await.unwrap();
        let mut ev = Event::user_text("hello");
        ev.actions
            .state_delta
            .insert("foo".into(), serde_json::json!("bar"));
        ev.actions
            .state_delta
            .insert("temp:t".into(), serde_json::json!(1));
        svc.append_event(&mut s, ev).await.unwrap();

        let got = svc
            .get_session("app", "user", &s.id, GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        // Non-temp state persisted.
        assert_eq!(got.state.get("foo"), Some(&serde_json::json!("bar")));
        // Temp state is on the in-memory copy because we applied it before trimming,
        // and our store mirrors the session, so it should be there too.
        assert!(got.state.get("temp:t").is_some());
        // The stored event delta has temp keys trimmed.
        let stored_delta = &got.events[0].actions.state_delta;
        assert!(!stored_delta.contains_key("temp:t"));
    }

    #[tokio::test]
    async fn list_filters_by_app_and_user() {
        let svc = InMemorySessionService::new();
        svc.create_session("app", "u1", None, None).await.unwrap();
        svc.create_session("app", "u2", None, None).await.unwrap();
        svc.create_session("other", "u1", None, None).await.unwrap();
        let r = svc.list_sessions("app", "u1").await.unwrap();
        assert_eq!(r.sessions.len(), 1);
    }

    #[tokio::test]
    async fn get_session_filters_recent_events() {
        let svc = InMemorySessionService::new();
        let mut s = svc.create_session("app", "user", None, None).await.unwrap();
        for i in 0..5 {
            let mut e = Event::user_text(format!("m{i}"));
            e.timestamp = f64::from(i);
            svc.append_event(&mut s, e).await.unwrap();
        }
        let got = svc
            .get_session(
                "app",
                "user",
                &s.id,
                GetSessionConfig {
                    num_recent_events: Some(2),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.events.len(), 2);
        assert_eq!(
            got.events[0]
                .response
                .content
                .as_ref()
                .unwrap()
                .text_concat(),
            "m3"
        );
    }
}
