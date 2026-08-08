//! Shared SQLite connection configuration.

#![expect(
    clippy::disallowed_methods,
    reason = "this is the centralized SQLite connection shim"
)]

use crate::DbTelemetry;
use crate::migrations::repair_legacy_recency_migration_version;
use crate::migrations::runtime_goals_migrator;
use crate::migrations::runtime_logs_migrator;
use crate::migrations::runtime_memories_migrator;
use crate::migrations::runtime_queue_migrator;
use crate::migrations::runtime_state_migrator;
use crate::migrations::runtime_thread_history_migrator;
use crate::runtime::RuntimeDbInitError;
use crate::runtime::validate_applied_migrations;
use crate::telemetry;
use crate::telemetry::DbKind;
use codex_utils_absolute_path::AbsolutePathBuf;
use log::LevelFilter;
use sqlx::ConnectOptions;
use sqlx::Error;
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteAutoVacuum;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteSynchronous;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

const LOGS_DB_FILENAME: &str = "pfterminal_logs_2.sqlite";
const GOALS_DB_FILENAME: &str = "pfterminal_goals_1.sqlite";
const MEMORIES_DB_FILENAME: &str = "pfterminal_memories_1.sqlite";
const QUEUE_DB_FILENAME: &str = "pfterminal_queue_1.sqlite";
const STATE_DB_FILENAME: &str = "pfterminal_state_5.sqlite";
const THREAD_HISTORY_DB_FILENAME: &str = "pfterminal_thread_history_1.sqlite";

#[derive(Clone, Copy)]
struct RuntimeDbSpec {
    label: &'static str,
    filename: &'static str,
    legacy_filename: &'static str,
    kind: DbKind,
    open_phase: &'static str,
    migrate_phase: &'static str,
}

impl RuntimeDbSpec {
    fn path(self, codex_home: &Path) -> PathBuf {
        codex_home.join(self.filename)
    }
}

const STATE_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "state DB",
    filename: STATE_DB_FILENAME,
    legacy_filename: "state_5.sqlite",
    kind: DbKind::State,
    open_phase: "open_state",
    migrate_phase: "migrate_state",
};

const LOGS_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "log DB",
    filename: LOGS_DB_FILENAME,
    legacy_filename: "logs_2.sqlite",
    kind: DbKind::Logs,
    open_phase: "open_logs",
    migrate_phase: "migrate_logs",
};

const GOALS_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "goals DB",
    filename: GOALS_DB_FILENAME,
    legacy_filename: "goals_1.sqlite",
    kind: DbKind::Goals,
    open_phase: "open_goals",
    migrate_phase: "migrate_goals",
};

const MEMORIES_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "memories DB",
    filename: MEMORIES_DB_FILENAME,
    legacy_filename: "memories_1.sqlite",
    kind: DbKind::Memories,
    open_phase: "open_memories",
    migrate_phase: "migrate_memories",
};

const QUEUE_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "queue DB",
    filename: QUEUE_DB_FILENAME,
    legacy_filename: "queue_1.sqlite",
    kind: DbKind::Queue,
    open_phase: "open_queue",
    migrate_phase: "migrate_queue",
};

const THREAD_HISTORY_DB: RuntimeDbSpec = RuntimeDbSpec {
    label: "thread history DB",
    filename: THREAD_HISTORY_DB_FILENAME,
    legacy_filename: "thread_history_1.sqlite",
    kind: DbKind::ThreadHistory,
    open_phase: "open_thread_history",
    migrate_phase: "migrate_thread_history",
};

const RUNTIME_DBS: [RuntimeDbSpec; 6] = [
    STATE_DB,
    LOGS_DB,
    GOALS_DB,
    MEMORIES_DB,
    QUEUE_DB,
    THREAD_HISTORY_DB,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDbPath {
    pub label: &'static str,
    pub path: PathBuf,
}

/// Resolved configuration shared by all Codex SQLite connections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteConfig {
    sqlite_home: AbsolutePathBuf,
    logs_db_path_override: Option<AbsolutePathBuf>,
}

impl SqliteConfig {
    pub fn from_sqlite_home(sqlite_home: AbsolutePathBuf) -> Self {
        Self {
            sqlite_home,
            logs_db_path_override: None,
        }
    }

    /// Override the logs database path while retaining the shared SQLite home for other runtime
    /// databases.
    pub fn with_logs_db_path(mut self, logs_db_path: AbsolutePathBuf) -> Self {
        self.logs_db_path_override = Some(logs_db_path);
        self
    }

    pub fn new_for_testing(sqlite_home: AbsolutePathBuf) -> Self {
        Self::from_sqlite_home(sqlite_home)
    }

    pub fn home(&self) -> &Path {
        self.sqlite_home.as_path()
    }

    /// Return the path to the primary state database.
    pub fn state_db_path(&self) -> PathBuf {
        STATE_DB.path(self.home())
    }

    /// Return whether this home already contains state that can supply runtime overlays.
    ///
    /// Configuration loading uses this check before opening the state runtime so read-only CLI
    /// commands do not create an otherwise absent state database. The legacy filename remains a
    /// valid source because runtime initialization adopts it into the PF namespace.
    pub fn has_existing_state_db(&self) -> bool {
        self.state_db_path().exists() || self.home().join(STATE_DB.legacy_filename).exists()
    }

    /// Return the path to the logs database.
    pub fn logs_db_path(&self) -> PathBuf {
        self.logs_db_path_override.as_ref().map_or_else(
            || LOGS_DB.path(self.home()),
            codex_utils_absolute_path::AbsolutePathBuf::to_path_buf,
        )
    }

    /// Return the path to the goals database.
    pub fn goals_db_path(&self) -> PathBuf {
        GOALS_DB.path(self.home())
    }

    /// Return the path to the memories database.
    pub fn memories_db_path(&self) -> PathBuf {
        MEMORIES_DB.path(self.home())
    }

    /// Return the path to the durable user-message queue database.
    pub fn queue_db_path(&self) -> PathBuf {
        QUEUE_DB.path(self.home())
    }

    /// Return the path to the paginated thread-history database.
    pub fn thread_history_db_path(&self) -> PathBuf {
        THREAD_HISTORY_DB.path(self.home())
    }

    /// Return the paths to every database managed by the state runtime.
    pub fn runtime_db_paths(&self) -> Vec<RuntimeDbPath> {
        RUNTIME_DBS
            .iter()
            .map(|spec| RuntimeDbPath {
                label: spec.label,
                path: spec.path(self.home()),
            })
            .collect()
    }

    /// Adopt pre-namespace PF Terminal databases once, while refusing to touch an upstream
    /// `.codex` home or a database whose applied migrations do not match this distribution.
    pub(crate) async fn migrate_legacy_runtime_db_names(&self) -> anyhow::Result<()> {
        let has_legacy_db = RUNTIME_DBS
            .iter()
            .any(|spec| self.home().join(spec.legacy_filename).exists());
        if !has_legacy_db {
            return Ok(());
        }
        if self.home().file_name().and_then(|name| name.to_str()) == Some(".codex") {
            anyhow::bail!(
                "refusing to rename upstream-named databases in {}: this database appears to belong to a different Codex/PFTerminal distribution; follow the repair recipe in pfterminal_codex_home_collision_incident_20260710.md",
                self.home().display()
            );
        }

        for spec in RUNTIME_DBS {
            let legacy_path = self.home().join(spec.legacy_filename);
            let namespaced_path = spec.path(self.home());
            if !tokio::fs::try_exists(&legacy_path).await?
                || tokio::fs::try_exists(&namespaced_path).await?
            {
                continue;
            }
            let migrator = match spec.kind {
                DbKind::State => runtime_state_migrator(),
                DbKind::Logs => runtime_logs_migrator(),
                DbKind::Goals => runtime_goals_migrator(),
                DbKind::Memories => runtime_memories_migrator(),
                DbKind::Queue => runtime_queue_migrator(),
                DbKind::ThreadHistory => runtime_thread_history_migrator(),
            };
            validate_applied_migrations(&legacy_path, &migrator).await?;
            rename_if_source_still_exists(&legacy_path, &namespaced_path).await?;
            for suffix in ["-wal", "-shm"] {
                let legacy_sidecar = PathBuf::from(format!("{}{suffix}", legacy_path.display()));
                let namespaced_sidecar =
                    PathBuf::from(format!("{}{suffix}", namespaced_path.display()));
                rename_if_source_still_exists(&legacy_sidecar, &namespaced_sidecar).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn open_state_db(
        &self,
        migrator: &Migrator,
        telemetry_override: Option<&dyn DbTelemetry>,
    ) -> anyhow::Result<SqlitePool> {
        // New state DBs should use incremental auto-vacuum, but retrofitting an
        // existing DB requires a full VACUUM. Do not attempt that during process
        // startup: it is maintenance work that can contend with foreground writers.
        self.open_runtime_db(STATE_DB, migrator, telemetry_override)
            .await
    }

    pub(super) async fn open_logs_db(
        &self,
        migrator: &Migrator,
        telemetry_override: Option<&dyn DbTelemetry>,
    ) -> anyhow::Result<SqlitePool> {
        self.open_runtime_db(LOGS_DB, migrator, telemetry_override)
            .await
    }

    pub(super) async fn open_goals_db(
        &self,
        migrator: &Migrator,
        telemetry_override: Option<&dyn DbTelemetry>,
    ) -> anyhow::Result<SqlitePool> {
        self.open_runtime_db(GOALS_DB, migrator, telemetry_override)
            .await
    }

    pub(super) async fn open_memories_db(
        &self,
        migrator: &Migrator,
        telemetry_override: Option<&dyn DbTelemetry>,
    ) -> anyhow::Result<SqlitePool> {
        self.open_runtime_db(MEMORIES_DB, migrator, telemetry_override)
            .await
    }

    pub(super) async fn open_queue_db(
        &self,
        migrator: &Migrator,
        telemetry_override: Option<&dyn DbTelemetry>,
    ) -> anyhow::Result<SqlitePool> {
        self.open_runtime_db(QUEUE_DB, migrator, telemetry_override)
            .await
    }

    pub(super) async fn open_thread_history_db(
        &self,
        migrator: &Migrator,
        telemetry_override: Option<&dyn DbTelemetry>,
    ) -> anyhow::Result<SqlitePool> {
        self.open_runtime_db(THREAD_HISTORY_DB, migrator, telemetry_override)
            .await
    }

    async fn open_runtime_db(
        &self,
        spec: RuntimeDbSpec,
        migrator: &Migrator,
        telemetry_override: Option<&dyn DbTelemetry>,
    ) -> anyhow::Result<SqlitePool> {
        let path = spec.path(self.home());
        let started = Instant::now();
        let pool_result = self
            .open_read_write_pool(&path)
            .await
            .map_err(anyhow::Error::from);
        telemetry::record_init_result(
            telemetry_override,
            spec.kind,
            spec.open_phase,
            started.elapsed(),
            &pool_result,
        );
        let pool = pool_result.map_err(|source| {
            RuntimeDbInitError::new(spec.label, "open", path.as_path(), source)
        })?;
        let started = Instant::now();
        let migrate_result = async {
            if matches!(spec.kind, DbKind::State) {
                repair_legacy_recency_migration_version(&pool, migrator).await?;
            }
            migrator.run(&pool).await.map_err(anyhow::Error::from)
        }
        .await;
        telemetry::record_init_result(
            telemetry_override,
            spec.kind,
            spec.migrate_phase,
            started.elapsed(),
            &migrate_result,
        );
        if let Err(source) = migrate_result {
            pool.close().await;
            return Err(
                RuntimeDbInitError::new(spec.label, "migrate", path.as_path(), source).into(),
            );
        }
        Ok(pool)
    }

    /// Open a writable Codex SQLite database, creating it if necessary.
    pub async fn open_read_write_pool(&self, path: &Path) -> Result<SqlitePool, Error> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .auto_vacuum(SqliteAutoVacuum::Incremental)
            .busy_timeout(Duration::from_secs(5))
            .log_statements(LevelFilter::Off);
        SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
    }

    /// Open an existing Codex SQLite database without creating or modifying it.
    pub async fn open_read_only_pool(&self, path: &Path) -> Result<SqlitePool, Error> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .read_only(true)
            .log_statements(LevelFilter::Off);
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
    }
}

async fn rename_if_source_still_exists(source: &Path, destination: &Path) -> anyhow::Result<()> {
    match tokio::fs::rename(source, destination).await {
        Ok(()) => Ok(()),
        Err(err)
            if err.kind() == std::io::ErrorKind::NotFound
                && tokio::fs::try_exists(destination).await? =>
        {
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}
