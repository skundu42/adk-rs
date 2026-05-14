//! In-memory [`SessionService`](crate::core::SessionService).

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;

use crate::core::{
    Event, GetSessionConfig, ListSessionsResponse, Session, SessionMeta, SessionService, State,
    StateScope,
};
use crate::error::{Error, Result};

/// Volatile session store. Keys: `(app, user, session_id)`.
#[derive(Debug, Default)]
pub struct InMemorySessionService {
    sessions: DashMap<(String, String, String), Arc<Mutex<Session>>>,
    /// `app:` scope — shared across all users and sessions of an app.
    app_state: DashMap<String, Arc<Mutex<State>>>,
    /// `user:` scope — shared across all sessions of one `(app, user)`.
    user_state: DashMap<(String, String), Arc<Mutex<State>>>,
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

    fn app_slot(&self, app: &str) -> Arc<Mutex<State>> {
        // `entry` holds the shard's write lock across the read-or-insert, so
        // two concurrent first-accesses cannot each construct their own slot
        // and lose one of them to the second `insert`.
        self.app_state
            .entry(app.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(State::new())))
            .value()
            .clone()
    }

    fn user_slot(&self, app: &str, user: &str) -> Arc<Mutex<State>> {
        self.user_state
            .entry((app.to_string(), user.to_string()))
            .or_insert_with(|| Arc::new(Mutex::new(State::new())))
            .value()
            .clone()
    }

    /// Overlay app + user + session state into a flattened view. The session
    /// keys win conflicts (most specific scope), then user, then app.
    fn overlay_state(&self, sess: &mut Session) {
        let app_view = self.app_slot(&sess.app_name);
        let user_view = self.user_slot(&sess.app_name, &sess.user_id);
        let mut merged = State::new();
        for (k, v) in app_view.lock().iter() {
            merged.set(k.clone(), v.clone());
        }
        for (k, v) in user_view.lock().iter() {
            merged.set(k.clone(), v.clone());
        }
        for (k, v) in sess.state.iter() {
            merged.set(k.clone(), v.clone());
        }
        sess.state = merged;
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
            .unwrap_or_else(crate::core::services::new_session_id);
        let key = Self::key(app_name, user_id, &sid);
        if self.sessions.contains_key(&key) {
            return Err(Error::already_exists(format!("session {sid}")));
        }
        let mut s = Session::new(app_name, user_id, sid);
        if let Some(state) = state {
            // Route by scope so `app:` / `user:` keys land in the shared
            // stores (visible to other sessions) rather than getting pinned
            // to this session. Temp keys are invocation-local and dropped.
            let (app_delta, user_delta, session_delta, _temp_delta) =
                State::partition_by_scope(&state.map);
            if !app_delta.is_empty() {
                self.app_slot(app_name).lock().apply(&app_delta);
            }
            if !user_delta.is_empty() {
                self.user_slot(app_name, user_id).lock().apply(&user_delta);
            }
            s.state = State::from_iter(session_delta);
        }
        let arc = Arc::new(Mutex::new(s.clone()));
        self.sessions.insert(key, arc);
        // Hand the caller a session with the merged overlay applied so the
        // initial state they passed is visible on the returned value.
        self.overlay_state(&mut s);
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
        let mut snap = arc.lock().clone();
        self.overlay_state(&mut snap);
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
        // Route the state delta by scope. App/user keys go to dedicated
        // stores so they're visible across sessions; session keys stay on
        // the session itself; temp keys live in-memory only.
        let (app_delta, user_delta, session_delta, temp_delta) =
            State::partition_by_scope(&event.actions.state_delta);

        for (k, v) in &temp_delta {
            session.state.set(k.clone(), v.clone());
        }
        if !app_delta.is_empty() {
            self.app_slot(&session.app_name).lock().apply(&app_delta);
        }
        if !user_delta.is_empty() {
            self.user_slot(&session.app_name, &session.user_id)
                .lock()
                .apply(&user_delta);
        }
        // The live `session.state` view still gets ALL non-temp keys so the
        // in-flight invocation can read them, but only session-scoped keys
        // are persisted into the session's own state slot (see mirror below).
        session.state.apply(&app_delta);
        session.state.apply(&user_delta);
        session.state.apply(&session_delta);

        // The persisted state delta should be the scope-trimmed set: temp
        // already absent, and app/user already routed elsewhere.
        event.actions.state_delta = session_delta.clone();
        session.last_update_time = crate::core::session::now_secs();
        session.events.push(event.clone());

        // Mirror into our authoritative store — but only with session-scope
        // state so app/user keys aren't pinned per-session.
        let key = Self::key(&session.app_name, &session.user_id, &session.id);
        let mut to_mirror = session.clone();
        to_mirror.state = State::from_iter(
            session
                .state
                .iter()
                .filter(|(k, _)| StateScope::of(k) == StateScope::Session)
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        if let Some(arc) = self.sessions.get(&key) {
            *arc.lock() = to_mirror;
        } else {
            self.sessions.insert(key, Arc::new(Mutex::new(to_mirror)));
        }
        Ok(event)
    }

    /// Race-free read-modify-write. Applies the event in-memory under the
    /// live lock, then mirrors the post-apply session into the internal
    /// store. The whole operation is atomic from the perspective of
    /// concurrent writers on the same `Arc<Mutex<Session>>`. Routes app/user
    /// scoped delta keys to their dedicated stores.
    async fn append_event_locked(
        &self,
        session_lock: &Arc<Mutex<Session>>,
        event: Event,
    ) -> Result<Event> {
        if event.partial == Some(true) {
            return Ok(event);
        }
        // Compute scope split once, outside the live lock — pure data.
        let (app_delta, user_delta, _session_delta, _temp_delta) =
            State::partition_by_scope(&event.actions.state_delta);

        // Critical section: apply delta + push event + snapshot for mirroring.
        // `apply_event_to_session` handles temp + non-temp into the in-memory
        // view so the invocation sees the fully-merged state.
        let (event, key, session_only_snap) = {
            let mut sess = session_lock.lock();
            let event = crate::core::services::apply_event_to_session(&mut sess, event);
            let key = Self::key(&sess.app_name, &sess.user_id, &sess.id);
            // Persistence snapshot carries only session-scope state (app/user
            // go to their own stores).
            let session_state = State::from_iter(
                sess.state
                    .iter()
                    .filter(|(k, _)| StateScope::of(k) == StateScope::Session)
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
            let mut snap = sess.clone();
            snap.state = session_state;
            (event, key, snap)
        };

        // Persist app/user keys to dedicated maps (out-of-lock; their own
        // mutexes serialize concurrent writers).
        if !app_delta.is_empty() {
            self.app_slot(&session_only_snap.app_name)
                .lock()
                .apply(&app_delta);
        }
        if !user_delta.is_empty() {
            self.user_slot(&session_only_snap.app_name, &session_only_snap.user_id)
                .lock()
                .apply(&user_delta);
        }

        // Mirror session-scope state into the internal slot.
        // Take the user/app id pieces we need before the move.
        let mirror_key = key;
        if let Some(arc) = self.sessions.get(&mirror_key) {
            *arc.lock() = session_only_snap.clone();
        } else {
            self.sessions
                .insert(mirror_key, Arc::new(Mutex::new(session_only_snap.clone())));
        }
        Ok(event)
    }
}

#[cfg(test)]
mod race_tests {
    use super::*;
    use crate::core::LlmResponse;

    /// Bug fix v0.2.1 #2 (session clone-overwrite race): N concurrent writers
    /// against the same `Arc<Mutex<Session>>` must not lose events. Before
    /// the fix this test reliably dropped writes because of the `clone() ...
    /// await ... overwrite` anti-pattern in callers.
    /// Regression for P1#3: a write to `app:foo` from one session must be
    /// visible to a *different* session for the same app (cross-session
    /// scope). Before the fix this kept the value pinned to the writing
    /// session.
    #[tokio::test]
    async fn app_scope_state_is_shared_across_sessions() {
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let s1 = svc
            .create_session("app", "alice", None, None)
            .await
            .unwrap();
        let lock1 = Arc::new(Mutex::new(s1.clone()));

        // Write `app:globals` from session 1.
        let mut ev = Event::new("agent", crate::core::LlmResponse::default());
        ev.actions
            .state_delta
            .insert("app:globals".into(), serde_json::json!({"tier": "premium"}));
        svc.append_event_locked(&lock1, ev).await.unwrap();

        // Read session 1 — sees the app key.
        let reloaded = svc
            .get_session("app", "alice", &s1.id, Default::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reloaded
                .state
                .get("app:globals")
                .and_then(|v| v.get("tier")),
            Some(&serde_json::Value::String("premium".into()))
        );

        // Create a fresh session for a DIFFERENT user under the same app —
        // it must also see the app-scoped key.
        let s2 = svc.create_session("app", "bob", None, None).await.unwrap();
        let s2_loaded = svc
            .get_session("app", "bob", &s2.id, Default::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            s2_loaded
                .state
                .get("app:globals")
                .and_then(|v| v.get("tier")),
            Some(&serde_json::Value::String("premium".into()))
        );
    }

    /// Regression for P1#3: a write to `user:prefs` is visible to another
    /// session for the same `(app, user)` but NOT to a different user.
    #[tokio::test]
    async fn user_scope_state_is_per_user() {
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let s1 = svc
            .create_session("app", "alice", None, None)
            .await
            .unwrap();
        let lock1 = Arc::new(Mutex::new(s1.clone()));

        let mut ev = Event::new("agent", crate::core::LlmResponse::default());
        ev.actions
            .state_delta
            .insert("user:lang".into(), serde_json::json!("en"));
        svc.append_event_locked(&lock1, ev).await.unwrap();

        // Alice's second session: sees `user:lang`.
        let s2_alice = svc
            .create_session("app", "alice", None, None)
            .await
            .unwrap();
        let alice2 = svc
            .get_session("app", "alice", &s2_alice.id, Default::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            alice2.state.get("user:lang"),
            Some(&serde_json::Value::String("en".into()))
        );

        // Bob's session: does NOT.
        let s_bob = svc.create_session("app", "bob", None, None).await.unwrap();
        let bob = svc
            .get_session("app", "bob", &s_bob.id, Default::default())
            .await
            .unwrap()
            .unwrap();
        assert!(bob.state.get("user:lang").is_none());
    }

    #[tokio::test]
    async fn concurrent_append_event_locked_preserves_every_event() {
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let s = svc.create_session("app", "u", None, None).await.unwrap();
        let lock = Arc::new(Mutex::new(s));

        const N: usize = 64;
        let mut handles = Vec::with_capacity(N);
        for i in 0..N {
            let svc_c = svc.clone();
            let lock_c = lock.clone();
            handles.push(tokio::spawn(async move {
                let ev = Event::new(format!("writer-{i}"), LlmResponse::default());
                svc_c.append_event_locked(&lock_c, ev).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let final_session = lock.lock();
        assert_eq!(
            final_session.events.len(),
            N,
            "every concurrent writer's event must survive (got {} of {})",
            final_session.events.len(),
            N
        );
    }

    /// Regression for the `app_slot()` TOCTOU race. Concurrent first-access
    /// to the same app's slot from different sessions used to race on
    /// `DashMap::get` + `insert`, so one writer's slot would be overwritten
    /// in the map before its writes landed where readers could see them.
    /// With the `entry().or_insert_with(...)` fix every `app:` key survives.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_app_writes_from_different_sessions_preserve_all_keys() {
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        const N: usize = 32;
        // Pre-create sessions so the race we exercise is `app_slot()`, not
        // `sessions` insertion.
        let mut locks = Vec::with_capacity(N);
        for i in 0..N {
            let s = svc
                .create_session("app", &format!("u{i}"), None, None)
                .await
                .unwrap();
            locks.push(Arc::new(Mutex::new(s)));
        }

        let barrier = Arc::new(tokio::sync::Barrier::new(N));
        let mut handles = Vec::with_capacity(N);
        for (i, lock) in locks.into_iter().enumerate() {
            let svc_c = svc.clone();
            let barrier_c = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier_c.wait().await;
                let mut ev = Event::new("agent", LlmResponse::default());
                ev.actions
                    .state_delta
                    .insert(format!("app:k{i}"), serde_json::json!(i));
                svc_c.append_event_locked(&lock, ev).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        // Read back through a fresh session — every `app:k_i` must be visible.
        let reader = svc
            .create_session("app", "reader", None, None)
            .await
            .unwrap();
        let reloaded = svc
            .get_session("app", "reader", &reader.id, Default::default())
            .await
            .unwrap()
            .unwrap();
        for i in 0..N {
            let k = format!("app:k{i}");
            assert!(
                reloaded.state.get(&k).is_some(),
                "missing {k}; got keys: {:?}",
                reloaded
                    .state
                    .iter()
                    .map(|(k, _)| k.clone())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Symmetric regression for `user_slot()`: concurrent first-access from
    /// different sessions of the same `(app, user)` must not lose writes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_user_writes_for_same_user_preserve_all_keys() {
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        const N: usize = 32;
        let mut locks = Vec::with_capacity(N);
        for _ in 0..N {
            let s = svc
                .create_session("app", "alice", None, None)
                .await
                .unwrap();
            locks.push(Arc::new(Mutex::new(s)));
        }

        let barrier = Arc::new(tokio::sync::Barrier::new(N));
        let mut handles = Vec::with_capacity(N);
        for (i, lock) in locks.into_iter().enumerate() {
            let svc_c = svc.clone();
            let barrier_c = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier_c.wait().await;
                let mut ev = Event::new("agent", LlmResponse::default());
                ev.actions
                    .state_delta
                    .insert(format!("user:k{i}"), serde_json::json!(i));
                svc_c.append_event_locked(&lock, ev).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let reader = svc
            .create_session("app", "alice", None, None)
            .await
            .unwrap();
        let reloaded = svc
            .get_session("app", "alice", &reader.id, Default::default())
            .await
            .unwrap()
            .unwrap();
        for i in 0..N {
            let k = format!("user:k{i}");
            assert!(reloaded.state.get(&k).is_some(), "missing {k}");
        }
    }

    /// `create_session(state: Some(...))` must route the initial state by
    /// scope just like `append_event` does, so `app:` and `user:` keys are
    /// visible to other sessions and `temp:` keys are dropped.
    #[tokio::test]
    async fn create_session_with_initial_scoped_state_routes_correctly() {
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let mut state = State::new();
        state.set("app:foo", serde_json::json!(1));
        state.set("user:bar", serde_json::json!(2));
        state.set("baz", serde_json::json!(3));
        state.set("temp:x", serde_json::json!(4));

        let s1 = svc
            .create_session("app", "alice", Some(state), Some("s1"))
            .await
            .unwrap();
        // Returned session exposes the merged view of all non-temp scopes.
        assert_eq!(s1.state.get("app:foo"), Some(&serde_json::json!(1)));
        assert_eq!(s1.state.get("user:bar"), Some(&serde_json::json!(2)));
        assert_eq!(s1.state.get("baz"), Some(&serde_json::json!(3)));
        assert!(
            s1.state.get("temp:x").is_none(),
            "temp keys must not survive create_session"
        );

        // A different session for a different user under the same app sees
        // `app:foo` only — `user:bar` and `baz` must not leak.
        let s2 = svc
            .create_session("app", "bob", None, Some("s2"))
            .await
            .unwrap();
        let bob = svc
            .get_session("app", "bob", &s2.id, Default::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bob.state.get("app:foo"), Some(&serde_json::json!(1)));
        assert!(
            bob.state.get("user:bar").is_none(),
            "user-scope must not leak across users"
        );
        assert!(
            bob.state.get("baz").is_none(),
            "session-scope must not leak across sessions"
        );

        // A different session for the same `(app, user=alice)` sees `app:` +
        // `user:` but not session-scope `baz`.
        let s3 = svc
            .create_session("app", "alice", None, Some("s3"))
            .await
            .unwrap();
        let alice2 = svc
            .get_session("app", "alice", &s3.id, Default::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alice2.state.get("app:foo"), Some(&serde_json::json!(1)));
        assert_eq!(alice2.state.get("user:bar"), Some(&serde_json::json!(2)));
        assert!(
            alice2.state.get("baz").is_none(),
            "session-scope must not leak across sessions"
        );
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
        // Temp state is invocation-scoped — it lives only on the live
        // `session` reference during the call. It is NOT persisted to the
        // store, so a subsequent `get_session` does not see it.
        assert!(
            got.state.get("temp:t").is_none(),
            "temp:t should not survive get_session: {:?}",
            got.state
        );
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
