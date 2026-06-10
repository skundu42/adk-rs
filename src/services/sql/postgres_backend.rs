//! postgres backend.
//!
//! Same shape as the sqlite backend; query placeholders are `$1`, `$2`, ...
//! Sub-second timestamps use `DOUBLE PRECISION`.

use async_trait::async_trait;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::core::services::new_session_id;
use crate::core::{
    Event, GetSessionConfig, ListSessionsResponse, Session, SessionMeta, SessionService, State,
};
use crate::error::{Error, Result, ServiceError};

/// SQL session service backed by PostgreSQL.
#[derive(Debug, Clone)]
pub struct SqlSessionService {
    pool: PgPool,
}

impl SqlSessionService {
    /// Connect using a `postgres://` URL.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
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
        sqlx::query("INSERT INTO sessions (app_name, user_id, id, state, last_update_time) VALUES ($1, $2, $3, $4, $5)")
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
                "INSERT INTO app_state (app_name, key, value) VALUES ($1, $2, $3) \
                 ON CONFLICT (app_name, key) DO UPDATE SET value = EXCLUDED.value",
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
                "INSERT INTO user_state (app_name, user_id, key, value) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (app_name, user_id, key) DO UPDATE SET value = EXCLUDED.value",
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
            "SELECT state, last_update_time FROM sessions WHERE app_name = $1 AND user_id = $2 AND id = $3",
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
        let app_kv = sqlx::query("SELECT key, value FROM app_state WHERE app_name = $1")
            .bind(app_name)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let user_kv =
            sqlx::query("SELECT key, value FROM user_state WHERE app_name = $1 AND user_id = $2")
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
            "SELECT payload, timestamp FROM events WHERE app_name = $1 AND user_id = $2 AND session_id = $3",
        );
        if cfg.after_timestamp.is_some() {
            q.push_str(" AND timestamp >= $4");
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
            "SELECT id, last_update_time FROM sessions WHERE app_name = $1 AND user_id = $2",
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
        sqlx::query("DELETE FROM events WHERE app_name = $1 AND user_id = $2 AND session_id = $3")
            .bind(app_name)
            .bind(user_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        sqlx::query("DELETE FROM sessions WHERE app_name = $1 AND user_id = $2 AND id = $3")
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

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;

        // 1. INSERT event row.
        sqlx::query("INSERT INTO events (app_name, user_id, session_id, id, invocation_id, author, branch, timestamp, payload) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
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

        // 2. Read-merge-write the session-scope state column under
        //    `SELECT ... FOR UPDATE` (row-level lock). Fixes the P1#2
        //    "last-writer-wins" race that otherwise lets concurrent invocations
        //    clobber each other's keys.
        if session_delta.is_empty() {
            sqlx::query("UPDATE sessions SET last_update_time = $1 WHERE app_name = $2 AND user_id = $3 AND id = $4")
                .bind(session.last_update_time)
                .bind(&session.app_name)
                .bind(&session.user_id)
                .bind(&session.id)
                .execute(&mut *tx)
                .await
                .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        } else {
            let current_json: Option<String> = sqlx::query_scalar(
                "SELECT state FROM sessions WHERE app_name = $1 AND user_id = $2 AND id = $3 FOR UPDATE",
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
            sqlx::query("UPDATE sessions SET state = $1, last_update_time = $2 WHERE app_name = $3 AND user_id = $4 AND id = $5")
                .bind(&state_json)
                .bind(session.last_update_time)
                .bind(&session.app_name)
                .bind(&session.user_id)
                .bind(&session.id)
                .execute(&mut *tx)
                .await
                .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        }

        // 3. Upsert app_state / user_state for routed keys.
        for (k, v) in &app_delta {
            let vj = serde_json::to_string(v)?;
            sqlx::query(
                "INSERT INTO app_state (app_name, key, value) VALUES ($1, $2, $3) \
                 ON CONFLICT (app_name, key) DO UPDATE SET value = EXCLUDED.value",
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
                "INSERT INTO user_state (app_name, user_id, key, value) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (app_name, user_id, key) DO UPDATE SET value = EXCLUDED.value",
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
    /// then writes through to PostgreSQL inside a transaction with
    /// `SELECT ... FOR UPDATE` on the session row, so concurrent writers no
    /// longer clobber each other's state keys (fix for P1#2). `app:` /
    /// `user:` scoped deltas go to dedicated tables (fix for P1#3).
    async fn append_event_locked(
        &self,
        session_lock: &std::sync::Arc<parking_lot::Mutex<Session>>,
        event: Event,
    ) -> Result<Event> {
        if event.partial == Some(true) {
            return Ok(event);
        }
        let (event, mut snapshot) = {
            let mut sess = session_lock.lock();
            let event = crate::core::services::apply_event_to_session(&mut sess, event);
            (event, sess.clone())
        };
        if snapshot.events.last().map(|e| &e.id) == Some(&event.id) {
            snapshot.events.pop();
        }
        self.append_event(&mut snapshot, event.clone()).await?;
        Ok(event)
    }
}
