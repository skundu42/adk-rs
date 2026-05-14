//! sqlite backend.

use async_trait::async_trait;
use sqlx::Row;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

use crate::core::services::new_session_id;
use crate::core::{
    Event, GetSessionConfig, ListSessionsResponse, Session, SessionMeta, SessionService, State,
};
use crate::error::{Error, Result, ServiceError};

/// SQL session service backed by SQLite.
#[derive(Debug, Clone)]
pub struct SqlSessionService {
    pool: SqlitePool,
}

impl SqlSessionService {
    /// Connect using a URL like `sqlite::memory:` or `sqlite:///path.db`.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect(url)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let svc = Self { pool };
        svc.run_migrations().await?;
        Ok(svc)
    }

    async fn run_migrations(&self) -> Result<()> {
        let sql = include_str!("migrations/0001_init.sql");
        for stmt in split_statements(sql) {
            sqlx::query(&stmt)
                .execute(&self.pool)
                .await
                .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        }
        Ok(())
    }
}

fn split_statements(s: &str) -> Vec<String> {
    // Strip line comments first, then split on `;`.
    let no_comments: String = s
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    no_comments
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("{s};"))
        .collect()
}

#[async_trait]
impl SessionService for SqlSessionService {
    async fn create_session(
        &self,
        app_name: &str,
        user_id: &str,
        state: Option<State>,
        id: Option<&str>,
    ) -> Result<Session> {
        let sid = id.map(str::to_string).unwrap_or_else(new_session_id);
        let now = crate::core::session::now_secs();
        // Route initial state by scope. App/user keys land in the dedicated
        // tables so they're visible to other sessions; session keys go on
        // this session's row; temp keys are invocation-local and dropped.
        let (app_delta, user_delta, session_delta) = match &state {
            Some(st) => {
                let (a, u, s, _t) = State::partition_by_scope(&st.map);
                (a, u, s)
            }
            None => Default::default(),
        };
        let session_state = State::from_iter(session_delta.clone());
        let state_json = serde_json::to_string(&session_state)?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        sqlx::query("INSERT INTO sessions (app_name, user_id, id, state, last_update_time) VALUES (?, ?, ?, ?, ?)")
            .bind(app_name)
            .bind(user_id)
            .bind(&sid)
            .bind(&state_json)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        for (k, v) in &app_delta {
            let vj = serde_json::to_string(v)?;
            sqlx::query(
                "INSERT INTO app_state (app_name, key, value) VALUES (?, ?, ?) \
                 ON CONFLICT(app_name, key) DO UPDATE SET value = excluded.value",
            )
            .bind(app_name)
            .bind(k)
            .bind(&vj)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        }
        for (k, v) in &user_delta {
            let vj = serde_json::to_string(v)?;
            sqlx::query(
                "INSERT INTO user_state (app_name, user_id, key, value) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(app_name, user_id, key) DO UPDATE SET value = excluded.value",
            )
            .bind(app_name)
            .bind(user_id)
            .bind(k)
            .bind(&vj)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        }
        tx.commit()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;

        let mut s = Session::new(app_name, user_id, sid);
        s.last_update_time = now;
        // Returned `state` is the merged view (matching get_session): the
        // caller sees the non-temp keys they passed in.
        let mut merged = State::new();
        for (k, v) in app_delta {
            merged.set(k, v);
        }
        for (k, v) in user_delta {
            merged.set(k, v);
        }
        for (k, v) in session_delta {
            merged.set(k, v);
        }
        s.state = merged;
        Ok(s)
    }

    async fn get_session(
        &self,
        app_name: &str,
        user_id: &str,
        session_id: &str,
        cfg: GetSessionConfig,
    ) -> Result<Option<Session>> {
        let row = sqlx::query(
            "SELECT state, last_update_time FROM sessions WHERE app_name = ? AND user_id = ? AND id = ?",
        )
        .bind(app_name)
        .bind(user_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let Some(row) = row else { return Ok(None) };

        let state_json: String = row
            .try_get(0)
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let last: f64 = row
            .try_get(1)
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let mut state: State = serde_json::from_str(&state_json).unwrap_or_default();

        // Overlay app + user scoped state. Session-scope keys win conflicts.
        let app_kv = sqlx::query("SELECT key, value FROM app_state WHERE app_name = ?")
            .bind(app_name)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let user_kv =
            sqlx::query("SELECT key, value FROM user_state WHERE app_name = ? AND user_id = ?")
                .bind(app_name)
                .bind(user_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let mut overlay = State::new();
        for r in app_kv {
            let k: String = r.try_get(0).unwrap_or_default();
            let v_json: String = r.try_get(1).unwrap_or_else(|_| "null".into());
            overlay.set(
                k,
                serde_json::from_str(&v_json).unwrap_or(serde_json::Value::Null),
            );
        }
        for r in user_kv {
            let k: String = r.try_get(0).unwrap_or_default();
            let v_json: String = r.try_get(1).unwrap_or_else(|_| "null".into());
            overlay.set(
                k,
                serde_json::from_str(&v_json).unwrap_or(serde_json::Value::Null),
            );
        }
        for (k, v) in state.iter() {
            overlay.set(k.clone(), v.clone());
        }
        state = overlay;

        let mut q = String::from(
            "SELECT payload, timestamp FROM events WHERE app_name = ? AND user_id = ? AND session_id = ?",
        );
        if cfg.after_timestamp.is_some() {
            q.push_str(" AND timestamp >= ?");
        }
        q.push_str(" ORDER BY timestamp ASC");

        let mut qb = sqlx::query(&q)
            .bind(app_name)
            .bind(user_id)
            .bind(session_id);
        if let Some(after) = cfg.after_timestamp {
            qb = qb.bind(after);
        }
        let rows = qb
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;

        let mut events: Vec<Event> = rows
            .into_iter()
            .filter_map(|r| {
                let payload: String = r.try_get(0).ok()?;
                serde_json::from_str(&payload).ok()
            })
            .collect();
        if let Some(n) = cfg.num_recent_events {
            let drop = events.len().saturating_sub(n);
            events.drain(..drop);
        }

        Ok(Some(Session {
            id: session_id.to_string(),
            app_name: app_name.to_string(),
            user_id: user_id.to_string(),
            state,
            events,
            last_update_time: last,
        }))
    }

    async fn list_sessions(&self, app_name: &str, user_id: &str) -> Result<ListSessionsResponse> {
        let rows = sqlx::query(
            "SELECT id, last_update_time FROM sessions WHERE app_name = ? AND user_id = ?",
        )
        .bind(app_name)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let sessions = rows
            .into_iter()
            .filter_map(|r| {
                let id: String = r.try_get(0).ok()?;
                let last: f64 = r.try_get(1).ok()?;
                Some(SessionMeta {
                    id,
                    app_name: app_name.to_string(),
                    user_id: user_id.to_string(),
                    last_update_time: last,
                })
            })
            .collect();
        Ok(ListSessionsResponse { sessions })
    }

    async fn delete_session(&self, app_name: &str, user_id: &str, session_id: &str) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        sqlx::query("DELETE FROM events WHERE app_name = ? AND user_id = ? AND session_id = ?")
            .bind(app_name)
            .bind(user_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        sqlx::query("DELETE FROM sessions WHERE app_name = ? AND user_id = ? AND id = ?")
            .bind(app_name)
            .bind(user_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        tx.commit()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        Ok(())
    }

    async fn append_event(&self, session: &mut Session, mut event: Event) -> Result<Event> {
        if event.partial == Some(true) {
            return Ok(event);
        }
        // Route by scope.
        let (app_delta, user_delta, session_delta, temp_delta) =
            State::partition_by_scope(&event.actions.state_delta);

        // Live in-memory: apply ALL non-temp + temp (temp lives for the
        // invocation; persisted state never carries it).
        for (k, v) in &temp_delta {
            session.state.set(k.clone(), v.clone());
        }
        session.state.apply(&app_delta);
        session.state.apply(&user_delta);
        session.state.apply(&session_delta);
        session.last_update_time = crate::core::session::now_secs();
        session.events.push(event.clone());

        // The persisted event carries only session-scope deltas (app/user
        // are routed to their tables; temp is invocation-local).
        event.actions.state_delta = session_delta.clone();
        let payload = serde_json::to_string(&event)?;

        // BEGIN IMMEDIATE: take a write lock right away so the SELECT/UPDATE
        // pair is serialised against concurrent writers (P1#2).
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;

        // 1. INSERT the event row. Each event is independent; no merge needed.
        sqlx::query("INSERT INTO events (app_name, user_id, session_id, id, invocation_id, author, branch, timestamp, payload) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&session.app_name)
            .bind(&session.user_id)
            .bind(&session.id)
            .bind(&event.id)
            .bind(&event.invocation_id)
            .bind(&event.author)
            .bind(event.branch.as_deref())
            .bind(event.timestamp)
            .bind(&payload)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;

        // 2. Read-merge-write the session-scope state column. Without this,
        //    a `last-writer-wins` UPDATE would clobber state keys set by
        //    concurrent invocations on the same session.
        if session_delta.is_empty() {
            sqlx::query("UPDATE sessions SET last_update_time = ? WHERE app_name = ? AND user_id = ? AND id = ?")
                .bind(session.last_update_time)
                .bind(&session.app_name)
                .bind(&session.user_id)
                .bind(&session.id)
                .execute(&mut *tx)
                .await
                .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        } else {
            let current_json: Option<String> = sqlx::query_scalar(
                "SELECT state FROM sessions WHERE app_name = ? AND user_id = ? AND id = ?",
            )
            .bind(&session.app_name)
            .bind(&session.user_id)
            .bind(&session.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
            let mut current: State = current_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            current.apply(&session_delta);
            let state_json = serde_json::to_string(&current)?;
            sqlx::query("UPDATE sessions SET state = ?, last_update_time = ? WHERE app_name = ? AND user_id = ? AND id = ?")
                .bind(&state_json)
                .bind(session.last_update_time)
                .bind(&session.app_name)
                .bind(&session.user_id)
                .bind(&session.id)
                .execute(&mut *tx)
                .await
                .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        }

        // 3. Upsert app_state / user_state rows for the routed deltas.
        for (k, v) in &app_delta {
            let vj = serde_json::to_string(v)?;
            sqlx::query(
                "INSERT INTO app_state (app_name, key, value) VALUES (?, ?, ?) \
                 ON CONFLICT(app_name, key) DO UPDATE SET value = excluded.value",
            )
            .bind(&session.app_name)
            .bind(k)
            .bind(&vj)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        }
        for (k, v) in &user_delta {
            let vj = serde_json::to_string(v)?;
            sqlx::query(
                "INSERT INTO user_state (app_name, user_id, key, value) VALUES (?, ?, ?, ?) \
                 ON CONFLICT(app_name, user_id, key) DO UPDATE SET value = excluded.value",
            )
            .bind(&session.app_name)
            .bind(&session.user_id)
            .bind(k)
            .bind(&vj)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        }

        tx.commit()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        Ok(event)
    }

    /// Race-free read-modify-write. Mutates the live session under one lock
    /// then writes through to SQL inside a transaction. The SQL write merges
    /// the state delta against the latest persisted state (read inside the
    /// same `BEGIN IMMEDIATE` transaction), so concurrent writers no longer
    /// clobber each other's keys (fix for P1#2). `app:` / `user:` scoped
    /// deltas go to dedicated tables (fix for P1#3).
    async fn append_event_locked(
        &self,
        session_lock: &std::sync::Arc<parking_lot::Mutex<Session>>,
        event: Event,
    ) -> Result<Event> {
        if event.partial == Some(true) {
            return Ok(event);
        }
        // Apply in-memory atomically.
        let (event, mut snapshot) = {
            let mut sess = session_lock.lock();
            let event = crate::core::services::apply_event_to_session(&mut sess, event);
            (event, sess.clone())
        };
        // The snapshot now has the event applied + state mutated. To avoid
        // double-pushing inside `self.append_event`, remove the just-pushed
        // event before delegating.
        if snapshot.events.last().map(|e| &e.id) == Some(&event.id) {
            snapshot.events.pop();
        }
        // Re-run the apply+persist on the snapshot so the SQL INSERT/UPDATE
        // see consistent state.
        let _ = self.append_event(&mut snapshot, event.clone()).await?;
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Event;
    use std::sync::Arc;

    async fn fresh() -> SqlSessionService {
        SqlSessionService::connect("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn end_to_end() {
        let svc = fresh().await;
        let s = svc
            .create_session("app", "u", None, Some("s1"))
            .await
            .unwrap();
        assert_eq!(s.id, "s1");
        let list = svc.list_sessions("app", "u").await.unwrap();
        assert_eq!(list.sessions.len(), 1);
        let mut s = svc
            .get_session("app", "u", "s1", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        let mut ev = Event::user_text("hi");
        ev.actions
            .state_delta
            .insert("foo".into(), serde_json::json!("bar"));
        ev.actions
            .state_delta
            .insert("temp:t".into(), serde_json::json!(1));
        svc.append_event(&mut s, ev).await.unwrap();
        let got = svc
            .get_session("app", "u", "s1", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.state.get("foo"), Some(&serde_json::json!("bar")));
        assert!(got.state.get("temp:t").is_none());

        svc.delete_session("app", "u", "s1").await.unwrap();
        assert!(
            svc.get_session("app", "u", "s1", GetSessionConfig::default())
                .await
                .unwrap()
                .is_none()
        );
    }

    /// P1#3: `app:` and `user:` scoped keys persist to dedicated tables and
    /// overlay back on `get_session`, so a new session sees them.
    #[tokio::test]
    async fn scoped_state_persists_and_overlays() {
        let svc = fresh().await;
        let s1 = svc
            .create_session("app", "alice", None, Some("s1"))
            .await
            .unwrap();
        assert_eq!(s1.id, "s1");
        let mut s1m = svc
            .get_session("app", "alice", "s1", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();

        let mut ev = Event::user_text("hi");
        ev.actions
            .state_delta
            .insert("app:plan".into(), serde_json::json!("pro"));
        ev.actions
            .state_delta
            .insert("user:lang".into(), serde_json::json!("en"));
        ev.actions
            .state_delta
            .insert("seenwelcome".into(), serde_json::json!(true));
        svc.append_event(&mut s1m, ev).await.unwrap();

        // Reload alice/s1 — sees all three.
        let alice_s1 = svc
            .get_session("app", "alice", "s1", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            alice_s1.state.get("app:plan"),
            Some(&serde_json::json!("pro"))
        );
        assert_eq!(
            alice_s1.state.get("user:lang"),
            Some(&serde_json::json!("en"))
        );
        assert_eq!(
            alice_s1.state.get("seenwelcome"),
            Some(&serde_json::json!(true))
        );

        // New session for alice — sees app + user (not session).
        let _ = svc
            .create_session("app", "alice", None, Some("s2"))
            .await
            .unwrap();
        let alice_s2 = svc
            .get_session("app", "alice", "s2", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            alice_s2.state.get("app:plan"),
            Some(&serde_json::json!("pro"))
        );
        assert_eq!(
            alice_s2.state.get("user:lang"),
            Some(&serde_json::json!("en"))
        );
        assert!(
            alice_s2.state.get("seenwelcome").is_none(),
            "session-scope key must NOT leak across sessions"
        );

        // Different user — sees app, not user.
        let _ = svc
            .create_session("app", "bob", None, Some("s3"))
            .await
            .unwrap();
        let bob_s3 = svc
            .get_session("app", "bob", "s3", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            bob_s3.state.get("app:plan"),
            Some(&serde_json::json!("pro"))
        );
        assert!(
            bob_s3.state.get("user:lang").is_none(),
            "user-scope key must NOT leak across users"
        );
    }

    /// P1#2: concurrent state writers do not clobber each other's keys. The
    /// transactional read-merge-write must serialize against the sqlite row
    /// lock — both keys must be present after both writers commit.
    #[tokio::test]
    async fn concurrent_state_deltas_are_merged_not_clobbered() {
        let svc = Arc::new(fresh().await);
        let _ = svc
            .create_session("app", "u", None, Some("s"))
            .await
            .unwrap();

        // Two writers, each setting a disjoint session-scope key.
        let svc_a = svc.clone();
        let svc_b = svc.clone();
        let a = tokio::spawn(async move {
            let mut s = svc_a
                .get_session("app", "u", "s", GetSessionConfig::default())
                .await
                .unwrap()
                .unwrap();
            let mut ev = Event::user_text("a");
            ev.actions
                .state_delta
                .insert("from_a".into(), serde_json::json!(1));
            svc_a.append_event(&mut s, ev).await.unwrap();
        });
        let b = tokio::spawn(async move {
            let mut s = svc_b
                .get_session("app", "u", "s", GetSessionConfig::default())
                .await
                .unwrap()
                .unwrap();
            let mut ev = Event::user_text("b");
            ev.actions
                .state_delta
                .insert("from_b".into(), serde_json::json!(2));
            svc_b.append_event(&mut s, ev).await.unwrap();
        });
        a.await.unwrap();
        b.await.unwrap();

        let final_session = svc
            .get_session("app", "u", "s", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            final_session.state.get("from_a"),
            Some(&serde_json::json!(1)),
            "writer A's key was clobbered: {final_session:?}"
        );
        assert_eq!(
            final_session.state.get("from_b"),
            Some(&serde_json::json!(2)),
            "writer B's key was clobbered: {final_session:?}"
        );
    }

    /// `create_session(state: Some(...))` must route the initial state by
    /// scope: app/user keys go to dedicated tables (visible to other
    /// sessions), session keys go to the session row, temp keys are dropped.
    #[tokio::test]
    async fn create_session_with_initial_scoped_state_routes_correctly() {
        let svc = fresh().await;
        let mut state = State::new();
        state.set("app:foo", serde_json::json!(1));
        state.set("user:bar", serde_json::json!(2));
        state.set("baz", serde_json::json!(3));
        state.set("temp:x", serde_json::json!(4));

        let s1 = svc
            .create_session("app", "alice", Some(state), Some("s1"))
            .await
            .unwrap();
        assert_eq!(s1.state.get("app:foo"), Some(&serde_json::json!(1)));
        assert_eq!(s1.state.get("user:bar"), Some(&serde_json::json!(2)));
        assert_eq!(s1.state.get("baz"), Some(&serde_json::json!(3)));
        assert!(
            s1.state.get("temp:x").is_none(),
            "temp keys must not survive create_session"
        );

        // A reload of s1 also drops temp and exposes the merged overlay.
        let alice_s1 = svc
            .get_session("app", "alice", "s1", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alice_s1.state.get("app:foo"), Some(&serde_json::json!(1)));
        assert_eq!(alice_s1.state.get("user:bar"), Some(&serde_json::json!(2)));
        assert_eq!(alice_s1.state.get("baz"), Some(&serde_json::json!(3)));
        assert!(alice_s1.state.get("temp:x").is_none());

        // A different user under the same app sees `app:foo` only.
        svc.create_session("app", "bob", None, Some("s2"))
            .await
            .unwrap();
        let bob = svc
            .get_session("app", "bob", "s2", GetSessionConfig::default())
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

        // A different session for the same alice sees `app:` + `user:` but
        // not session-scope `baz`.
        svc.create_session("app", "alice", None, Some("s3"))
            .await
            .unwrap();
        let alice_s3 = svc
            .get_session("app", "alice", "s3", GetSessionConfig::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alice_s3.state.get("app:foo"), Some(&serde_json::json!(1)));
        assert_eq!(alice_s3.state.get("user:bar"), Some(&serde_json::json!(2)));
        assert!(
            alice_s3.state.get("baz").is_none(),
            "session-scope must not leak across sessions"
        );
    }

    /// Cross-session concurrent writes to distinct `app:` keys from
    /// different sessions must all be preserved. SQLite serializes the
    /// per-event transactions, so each upsert lands.
    #[tokio::test]
    async fn concurrent_app_writes_from_different_sessions_preserve_all_keys() {
        let svc = Arc::new(fresh().await);
        const N: usize = 8;
        for i in 0..N {
            svc.create_session("app", &format!("u{i}"), None, Some(&format!("s{i}")))
                .await
                .unwrap();
        }

        let mut handles = Vec::with_capacity(N);
        for i in 0..N {
            let svc_c = svc.clone();
            handles.push(tokio::spawn(async move {
                let mut s = svc_c
                    .get_session(
                        "app",
                        &format!("u{i}"),
                        &format!("s{i}"),
                        GetSessionConfig::default(),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                let mut ev = Event::user_text(format!("w{i}"));
                ev.actions
                    .state_delta
                    .insert(format!("app:k{i}"), serde_json::json!(i));
                svc_c.append_event(&mut s, ev).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        // Any session reading sees every `app:k_i`.
        svc.create_session("app", "reader", None, Some("reader-s"))
            .await
            .unwrap();
        let reloaded = svc
            .get_session("app", "reader", "reader-s", GetSessionConfig::default())
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
}
