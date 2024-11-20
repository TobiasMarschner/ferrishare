use crate::*;

/// Regularly cleans up expired files and sessions.
///
/// Every query
#[tracing::instrument(level = "info", skip(aps))]
pub async fn cleanup_cronjob(aps: AppState) {
    // Run indefinitely.
    loop {
        // Wait some time.
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Now query all files from the database.
        #[derive(Debug, FromRow)]
        struct FileRow {
            efd_sha256sum: String,
            expiry_ts: String,
        }

        let files: Vec<FileRow> =
            match sqlx::query_as("SELECT efd_sha256sum, expiry_ts FROM uploaded_files;")
                .fetch_all(&aps.db)
                .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!("failed to read files from database: {e}");
                    // This cronjob must not fail.
                    // If the db-query failed that's critical (which is why we log it)
                    // but our best approach is nonetheless to simply try again after a while.
                    continue;
                }
            };

        // Now filter out all files that have not yet expired.
        let files = files
            .into_iter()
            .filter(|e| {
                match has_expired(&e.expiry_ts) {
                    Ok(exp) => exp,
                    Err(e) => {
                        // Parsing errors on the timestamp should never happen.
                        // If they do, that's an indicator that the databse is corrupt.
                        // Best we can do here is log the issue and pretend the entry has already
                        // expired. This may cause files to get cleaned up early.
                        tracing::error!("failed to parse timestamp: {}", e.message);
                        true
                    }
                }
            })
            .collect_vec();

        // Delete each file one after the other.
        // I'd have preferred it if I could delete all expired files with one query,
        // but sqlx currently makes this rather difficult, as I cannot bind a whole Vec
        // in the following query: "DELETE FROM uploaded_files WHERE efd_sha256sum IN (?)".
        for file in files {
            match delete::cleanup_file(&file.efd_sha256sum, &aps.db).await {
                Ok(_) => {
                    tracing::info!(
                        efd_sha256sum = file.efd_sha256sum,
                        "file expired and was automatically removed"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        efd_sha256sum = file.efd_sha256sum,
                        "failed to delete file from database / disk: {}",
                        e
                    );
                }
            }
        }

        // Next up, query all sessions and delete the ones that have expired.
        #[derive(Debug, FromRow)]
        struct SessionRow {
            session_id_sha256sum: String,
            expiry_ts: String,
        }

        let sessions: Vec<SessionRow> =
            match sqlx::query_as("SELECT session_id_sha256sum, expiry_ts FROM admin_sessions;")
                .fetch_all(&aps.db)
                .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::error!("failed to read sessions from database: {e}");
                    // This cronjob must not fail.
                    // If the db-query failed that's critical (which is why we log it)
                    // but our best approach is nonetheless to simply try again after a while.
                    continue;
                }
            };

        // Now filter out all sessions that have not yet expired.
        let sessions = sessions
            .into_iter()
            .filter(|e| {
                match has_expired(&e.expiry_ts) {
                    Ok(exp) => exp,
                    Err(e) => {
                        // Parsing errors on the timestamp should never happen.
                        // If they do, that's an indicator that the databse is corrupt.
                        // Best we can do here is log the issue and pretend the entry has already
                        // expired. This may cause sessions to get cleaned up early.
                        tracing::error!("failed to parse timestamp: {}", e.message);
                        true
                    }
                }
            })
            .collect_vec();

        // Delete each session one after the other.
        // I'd have preferred it if I could delete all expired sessions with one query,
        // but sqlx currently makes this rather difficult, as I cannot bind a whole Vec
        // in the following query: "DELETE FROM admin_sessions WHERE session_id_sha256sum IN (?)".
        for session in sessions {
            match sqlx::query("DELETE FROM admin_sessions WHERE session_id_sha256sum = ?;")
                .bind(&session.session_id_sha256sum)
                .execute(&aps.db)
                .await
            {
                Ok(_) => {
                    tracing::info!("admin session expired and was automatically removed");
                }
                Err(e) => {
                    tracing::error!("failed to delete row from database: {e}");
                    continue;
                }
            };
        }
    }
}

