-- adk-rs SQL session schema (v1).
-- Compatible with sqlite and postgres (we serialize payloads as TEXT so the
-- same DDL works for both; on postgres you may swap to JSONB for indexing).

CREATE TABLE IF NOT EXISTS sessions (
  app_name         TEXT    NOT NULL,
  user_id          TEXT    NOT NULL,
  id               TEXT    NOT NULL,
  state            TEXT    NOT NULL DEFAULT '{}',
  last_update_time DOUBLE PRECISION NOT NULL DEFAULT 0,
  PRIMARY KEY (app_name, user_id, id)
);

CREATE TABLE IF NOT EXISTS events (
  app_name      TEXT    NOT NULL,
  user_id       TEXT    NOT NULL,
  session_id    TEXT    NOT NULL,
  id            TEXT    NOT NULL,
  invocation_id TEXT    NOT NULL,
  author        TEXT    NOT NULL,
  branch        TEXT,
  timestamp     DOUBLE PRECISION NOT NULL,
  payload       TEXT    NOT NULL,
  PRIMARY KEY (app_name, user_id, session_id, id)
);

CREATE INDEX IF NOT EXISTS events_by_session_ts
  ON events (app_name, user_id, session_id, timestamp);

CREATE TABLE IF NOT EXISTS app_state (
  app_name TEXT NOT NULL,
  key      TEXT NOT NULL,
  value    TEXT NOT NULL,
  PRIMARY KEY (app_name, key)
);

CREATE TABLE IF NOT EXISTS user_state (
  app_name TEXT NOT NULL,
  user_id  TEXT NOT NULL,
  key      TEXT NOT NULL,
  value    TEXT NOT NULL,
  PRIMARY KEY (app_name, user_id, key)
);
