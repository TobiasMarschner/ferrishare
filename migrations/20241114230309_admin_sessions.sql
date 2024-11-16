CREATE TABLE IF NOT EXISTS admin_sessions
(
  id INTEGER PRIMARY KEY NOT NULL,
  session_id_sha256sum TEXT,
  expiry_ts TEXT
) STRICT;
