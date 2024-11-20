CREATE TABLE IF NOT EXISTS uploaded_files
(
  id INTEGER PRIMARY KEY NOT NULL,
  efd_sha256sum TEXT,
  admin_key_sha256sum TEXT,
  e_filename BLOB,
  iv_fd BLOB,
  iv_fn BLOB,
  filesize INTEGER,
  upload_ip TEXT,
  upload_ts TEXT,
  expiry_ts TEXT,
  downloads INTEGER DEFAULT 0
) STRICT;
