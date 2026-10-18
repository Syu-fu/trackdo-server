//! Durable, per-user operation log.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{SqliteConnection, SqlitePool};
use thiserror::Error;
use trackdo_core::op::{Op, OpId, Seq};

use self::row::OpRow;

mod row;

/// How long a write waits for another connection's write to finish.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Owner of a log. Every token currently maps to [`UserId::DEFAULT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(pub i64);

impl UserId {
    pub const DEFAULT: Self = Self(1);
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("stored op at seq {0} cannot be decoded")]
    Corrupt(i64),
}

#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens the database at `path`, creating and migrating it as needed.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(BUSY_TIMEOUT);
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        Self::migrated(pool).await
    }

    /// A private database that lives as long as the store.
    pub async fn in_memory() -> Result<Self, StoreError> {
        // Every connection would get its own in-memory database, so keep
        // exactly one open for good.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(SqliteConnectOptions::from_str("sqlite::memory:")?)
            .await?;
        Self::migrated(pool).await
    }

    async fn migrated(pool: SqlitePool) -> Result<Self, StoreError> {
        sqlx::migrate!().run(&pool).await?;
        Ok(Self { pool })
    }

    /// Stores the ops not seen before, in order, under new sequence numbers.
    ///
    /// Returns the ids of every op in `ops`, retransmitted ones included.
    pub async fn append(&self, user: UserId, ops: &[Op]) -> Result<Vec<OpId>, StoreError> {
        // Taking the write lock up front serializes sequence numbering.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut last: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM ops WHERE user_id = ?")
                .bind(user.0)
                .fetch_one(&mut *tx)
                .await?;
        for op in ops {
            if insert(&mut tx, user, &OpRow::encode(last + 1, op)).await? {
                last += 1;
            }
        }
        tx.commit().await?;
        Ok(ops.iter().map(|op| op.op_id).collect())
    }

    /// Up to `limit` ops after `after` (from the start when `None`), in
    /// sequence order.
    pub async fn ops_after(
        &self,
        user: UserId,
        after: Option<Seq>,
        limit: u32,
    ) -> Result<Vec<(Seq, Op)>, StoreError> {
        let after = after.map_or(0, |Seq(seq)| i64::try_from(seq).unwrap_or(i64::MAX));
        let rows: Vec<OpRow> = sqlx::query_as(
            "SELECT seq, op_id, hlc_wall_ms, hlc_counter, hlc_device, target, task_id, \
                    entry_id, field, value_text, value_int \
             FROM ops WHERE user_id = ? AND seq > ? ORDER BY seq LIMIT ?",
        )
        .bind(user.0)
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let seq = row.seq;
                row.decode().ok_or(StoreError::Corrupt(seq))
            })
            .collect()
    }
}

/// Inserts `row` unless its op id is already stored. Returns whether it was.
async fn insert(
    conn: &mut SqliteConnection,
    user: UserId,
    row: &OpRow,
) -> Result<bool, StoreError> {
    let result = sqlx::query(
        "INSERT INTO ops (user_id, seq, op_id, hlc_wall_ms, hlc_counter, hlc_device, \
                          target, task_id, entry_id, field, value_text, value_int) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (user_id, op_id) DO NOTHING",
    )
    .bind(user.0)
    .bind(row.seq)
    .bind(&row.op_id)
    .bind(row.hlc_wall_ms)
    .bind(row.hlc_counter)
    .bind(&row.hlc_device)
    .bind(&row.target)
    .bind(&row.task_id)
    .bind(&row.entry_id)
    .bind(&row.field)
    .bind(&row.value_text)
    .bind(row.value_int)
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests;
