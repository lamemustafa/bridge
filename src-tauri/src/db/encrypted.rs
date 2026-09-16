use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{ConnectOptions, Executor, SqlitePool};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

const MIRROR_KEY_BYTES: usize = 32;
const KEYRING_SERVICE: &str = "com.complyeaze.bridge.tally-mirror";
const KEYRING_ACCOUNT_PREFIX: &str = "database-key-v1";

pub trait MirrorKeyStore: Send + Sync {
    fn load(&self) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>>;
    fn save(&self, key: &[u8]) -> anyhow::Result<()>;
    fn delete(&self) -> anyhow::Result<()>;
}

#[derive(Clone)]
pub struct OsMirrorKeyStore {
    account: String,
}

impl OsMirrorKeyStore {
    pub fn for_database(database_path: &Path) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(database_path.as_os_str().to_string_lossy().as_bytes());
        let path_id = hex_key(&hasher.finalize());
        Self {
            account: format!("{KEYRING_ACCOUNT_PREFIX}:{path_id}"),
        }
    }

    fn entry(&self) -> anyhow::Result<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, &self.account)
            .map_err(|_| anyhow::anyhow!("The operating-system credential store is unavailable"))
    }
}

impl MirrorKeyStore for OsMirrorKeyStore {
    fn load(&self) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
        match self.entry()?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => anyhow::bail!(
                "The Tally mirror key could not be read from the operating-system credential store"
            ),
        }
    }

    fn save(&self, key: &[u8]) -> anyhow::Result<()> {
        self.entry()?
            .set_secret(key)
            .map_err(|_| anyhow::anyhow!("The Tally mirror key could not be stored securely"))
    }

    fn delete(&self) -> anyhow::Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => anyhow::bail!("The Tally mirror key could not be removed from the operating-system credential store"),
        }
    }
}

pub struct MirrorInitializationLock {
    _file: File,
}

pub fn lock_mirror_initialization(
    database_path: &Path,
) -> anyhow::Result<MirrorInitializationLock> {
    let file_name = database_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("The Tally mirror path has no file name"))?;
    let mut lock_name = file_name.to_os_string();
    lock_name.push(".init.lock");
    let lock_path = database_path.with_file_name(lock_name);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|_| anyhow::anyhow!("The Tally mirror initialization lock could not be opened"))?;
    file.lock()
        .map_err(|_| anyhow::anyhow!("The Tally mirror is being initialized by another process"))?;
    Ok(MirrorInitializationLock { _file: file })
}

pub struct ResolvedMirrorKey {
    pub key: Zeroizing<Vec<u8>>,
    pub created: bool,
}

pub fn resolve_mirror_key(
    database_path: &Path,
    key_store: &dyn MirrorKeyStore,
) -> anyhow::Result<ResolvedMirrorKey> {
    if let Some(key) = key_store.load()? {
        validate_key(&key)?;
        return Ok(ResolvedMirrorKey {
            key,
            created: false,
        });
    }

    if database_path.exists() {
        anyhow::bail!(
            "The encrypted Tally mirror exists but its operating-system key is missing; an explicit local reset is required"
        );
    }

    let mut key = Zeroizing::new(vec![0_u8; MIRROR_KEY_BYTES]);
    getrandom::fill(&mut key).map_err(|_| {
        anyhow::anyhow!("The operating system could not generate a Tally mirror key")
    })?;
    key_store.save(&key)?;
    Ok(ResolvedMirrorKey { key, created: true })
}

fn validate_key(key: &[u8]) -> anyhow::Result<()> {
    if key.len() != MIRROR_KEY_BYTES {
        anyhow::bail!(
            "The stored Tally mirror key is invalid; an explicit local reset is required"
        );
    }
    Ok(())
}

pub async fn connect_encrypted(
    database_path: &Path,
    key: Zeroizing<Vec<u8>>,
) -> anyhow::Result<SqlitePool> {
    validate_key(&key)?;
    // SqliteConnectOptions owns PRAGMA values as ordinary strings and the pool keeps those
    // options for its lifetime so that it can replace connections. Keeping SQLCipher's key in
    // `.pragma("key", ...)` would therefore leave an unprotected copy in the pool. Retain the
    // original bytes only in zeroizing storage and apply them to every new SQLite handle through
    // SQLCipher's C API instead.
    let connection_key = Arc::new(key);
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(true)
        .disable_statement_logging();

    let key_for_connections = Arc::clone(&connection_key);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _metadata| {
            let key = Arc::clone(&key_for_connections);
            Box::pin(async move {
                {
                    let mut handle = connection.lock_handle().await?;
                    let key_length = i32::try_from(key.len())
                        .map_err(|_| sqlx::Error::Protocol("SQLCipher key is too long".into()))?;
                    // SAFETY: `lock_handle()` excludes the SQLx worker for the duration of this
                    // call, the handle is live, and `key` remains allocated for the whole call.
                    // SQLCipher copies the key into its per-connection codec state.
                    let status = unsafe {
                        libsqlite3_sys::sqlite3_key(
                            handle.as_raw_handle().as_ptr(),
                            key.as_ptr().cast(),
                            key_length,
                        )
                    };
                    if status != libsqlite3_sys::SQLITE_OK {
                        return Err(sqlx::Error::Protocol(
                            "SQLCipher rejected the Tally mirror key".into(),
                        ));
                    }
                }

                // These settings must be applied after sqlite3_key(). Executing them here also
                // means replacement connections receive the same hardened configuration without
                // putting key material in SqliteConnectOptions.
                // Keep actionable SQLCipher errors while suppressing the per-allocation warning
                // emitted when an operating-system page lock cannot be acquired.
                connection
                    .execute("PRAGMA cipher_log_level = ERROR;")
                    .await?;
                connection
                    .execute("PRAGMA cipher_memory_security = ON;")
                    .await?;
                connection.execute("PRAGMA secure_delete = ON;").await?;
                connection.execute("PRAGMA foreign_keys = ON;").await?;
                connection.execute("PRAGMA journal_mode = WAL;").await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await?;

    let validation = validate_encrypted_connection(&pool).await;
    if let Err(error) = validation {
        pool.close().await;
        return Err(error);
    }
    Ok(pool)
}

async fn validate_encrypted_connection(pool: &SqlitePool) -> anyhow::Result<()> {
    let cipher_version = sqlx::query_scalar::<_, String>("PRAGMA cipher_version;")
        .fetch_one(pool)
        .await
        .map_err(|_| {
            anyhow::anyhow!("SQLCipher is unavailable or the Tally mirror key is invalid")
        })?;
    if cipher_version.trim().is_empty() {
        anyhow::bail!("SQLCipher did not report an active cipher version");
    }
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_master;")
        .fetch_one(pool)
        .await
        .map_err(|_| {
            anyhow::anyhow!("The encrypted Tally mirror could not be opened with its stored key")
        })?;
    let integrity_errors = sqlx::query_scalar::<_, String>("PRAGMA cipher_integrity_check;")
        .fetch_all(pool)
        .await
        .map_err(|_| anyhow::anyhow!("The encrypted Tally mirror could not be verified"))?;
    if !integrity_errors.is_empty() {
        anyhow::bail!("The encrypted Tally mirror failed its integrity check");
    }

    let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys;")
        .fetch_one(pool)
        .await?;
    let secure_delete = sqlx::query_scalar::<_, i64>("PRAGMA secure_delete;")
        .fetch_one(pool)
        .await?;
    if foreign_keys != 1 || secure_delete != 1 {
        anyhow::bail!("The encrypted Tally mirror security settings were not applied");
    }
    Ok(())
}

fn hex_key(key: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(key.len() * 2);
    for byte in key {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
#[path = "encrypted_tests.rs"]
mod tests;
