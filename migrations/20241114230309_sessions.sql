CREATE TABLE IF NOT EXISTS sessions
(
  id INTEGER PRIMARY KEY NOT NULL,
  session_key TEXT,
  expiry_ts TEXT,
) STRICT;
