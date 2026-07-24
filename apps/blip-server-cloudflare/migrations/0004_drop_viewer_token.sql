CREATE TABLE videos_new (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  object_key TEXT NOT NULL,
  upload_id TEXT NOT NULL,
  storage_config TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('uploading', 'complete')),
  created_at TEXT NOT NULL,
  privacy TEXT NOT NULL DEFAULT 'public' CHECK (privacy IN ('public', 'password', 'private')),
  password_hash TEXT
);

INSERT INTO videos_new (id, user_id, object_key, upload_id, storage_config, status, created_at, privacy, password_hash)
  SELECT id, user_id, object_key, upload_id, storage_config, status, created_at, privacy, password_hash FROM videos;

DROP TABLE videos;
ALTER TABLE videos_new RENAME TO videos;
CREATE INDEX videos_user_id ON videos(user_id);
