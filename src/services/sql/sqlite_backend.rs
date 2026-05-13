//! sqlite backend.

use async_trait::async_trait;
use sqlx::Row;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

use crate::core::services::new_session_id;
use crate::core::{
    Event, GetSessionConfig, ListSessionsResponse, Session, SessionMeta, SessionService, State,
    StateScope,
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
        let state_json = state
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_else(|_| "{}".into()))
            .unwrap_or_else(|| "{}".into());
        let now = crate::core::session::now_secs();
        sqlx::query("INSERT INTO sessions (app_name, user_id, id, state, last_update_time) VALUES (?, ?, ?, ?, ?)")
            .bind(app_name)
            .bind(user_id)
            .bind(&sid)
            .bind(&state_json)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        let mut s = Session::new(app_name, user_id, sid);
        if let Some(st) = state {
            s.state = st;
        }
        s.last_update_time = now;
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
        let state: State = serde_json::from_str(&state_json).unwrap_or_default();

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
        for (k, v) in &event.actions.state_delta {
            if StateScope::of(k) == StateScope::Temp {
                session.state.set(k.clone(), v.clone());
            }
        }
        event.actions.state_delta = State::trim_temp_keys(&event.actions.state_delta);
        session.state.apply(&event.actions.state_delta);
        session.last_update_time = crate::core::session::now_secs();
        session.events.push(event.clone());

        let payload = serde_json::to_string(&event)?;
        // Persist state WITHOUT temp keys.
        let persisted_state: State = State::from_iter(
            session
                .state
                .iter()
                .filter(|(k, _)| StateScope::of(k) != StateScope::Temp)
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        let state_json = serde_json::to_string(&persisted_state)?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
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
        sqlx::query("UPDATE sessions SET state = ?, last_update_time = ? WHERE app_name = ? AND user_id = ? AND id = ?")
            .bind(&state_json)
            .bind(session.last_update_time)
            .bind(&session.app_name)
            .bind(&session.user_id)
            .bind(&session.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        tx.commit()
            .await
            .map_err(|e| Error::Service(ServiceError::Backend(e.to_string())))?;
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Event;

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
}
