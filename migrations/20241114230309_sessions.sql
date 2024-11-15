CREATE TABLE IF NOT EXISTS sessions
(
  id INTEGER PRIMARY KEY NOT NULL,
  session_sha256sum TEXT,
  expiry_ts TEXT
) STRICT;
