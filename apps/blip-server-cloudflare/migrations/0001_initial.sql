CREATE TABLE users (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  api_key_hash TEXT NOT NULL UNIQUE,
  storage_config TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE videos (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  object_key TEXT NOT NULL,
  upload_id TEXT NOT NULL,
  storage_config TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('uploading', 'complete')),
  created_at TEXT NOT NULL
);

CREATE INDEX videos_user_id ON videos(user_id);
