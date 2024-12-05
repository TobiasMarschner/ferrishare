//! Background async task cleans up expired files and admin sessions

use crate::*;

/// Async task that cleans up expired files and admin sessions every 15 minutes
///
/// Is started by [main] and then runs indefinitely.
/// Has three responsibilites:
/// 1) Deleting expired files, both from the database and from disk.
/// 2) Clearing expired admin sessions from the session-db.
/// 3) Decreasing the accumulated request count from the rate-limiter, thereby
///    implementing the leaky-bucket algorithm.
#[tracing::instrument(level = "info", skip(aps))]
pub async fn cleanup_cronjob(aps: AppState) {
    // Run indefinitely.
    loop {
        // It's completely fine if this only runs every 15 minutes.
        // All of the queries are written in a way that they check the expiration
        // time and will refuse to serve resources that still exist but have already expired.
        tokio::time::sleep(tokio::time::Duration::from_secs(900)).await;

        // Acquire the rate-limiter.
        let mut rl = aps.rate_limiter.write().await;

        // Leak from the bucket, i.e. reduce the request count for all IPs.
        for (_, v) in rl.iter_mut() {
            // Decrease, but ensure no underflows take place.
            // If the value would be below 0, simply insert 0.
            *v = v
                .checked_sub(std::cmp::max(aps.conf.daily_request_limit_per_ip / 96, 1))
                .unwrap_or_default();
        }

        // Next up, clear all IPs that have fully cooled down.
        // If we don't do this the rate-limiter will simply collect
        // a list of *all* IPs that ever talked to the server - no good.
        rl.retain(|_, v| *v > 0);

        // Free the guard before continuing - this way we can serve requests again.
        drop(rl);

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
