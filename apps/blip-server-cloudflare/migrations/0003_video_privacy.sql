ALTER TABLE videos ADD COLUMN privacy TEXT NOT NULL DEFAULT 'public'
  CHECK (privacy IN ('public', 'password', 'private'));
ALTER TABLE videos ADD COLUMN password_hash TEXT;
