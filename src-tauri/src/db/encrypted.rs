use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{ConnectOptions, Executor, SqliteConnection, SqlitePool};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

const MIRROR_KEY_BYTES: usize = 32;
const KEYRING_SERVICE: &str = "com.complyeaze.bridge.tally-mirror";
const KEYRING_ACCOUNT_PREFIX: &str = "database-key-v1";

#[derive(Debug, thiserror::Error)]
pub enum EncryptedOpenError {
    #[error("The encrypted Tally mirror rejected the supplied key or its encrypted contents")]
    UnreadableWithSuppliedKey {
        #[source]
        source: sqlx::Error,
    },
    #[error("The encrypted Tally mirror failed its integrity check")]
    IntegrityFailed,
    #[error("SQLCipher is unavailable for the encrypted Tally mirror")]
    CipherUnavailable,
    #[error("The encrypted Tally mirror security settings were not applied")]
    SecuritySettings,
    #[error("The encrypted Tally mirror storage could not be opened")]
    Storage {
        #[source]
        source: sqlx::Error,
    },
    #[error("The encrypted Tally mirror connection pool timed out")]
    PoolTimedOut,
}

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

    // SQLx retries `after_connect` failures until its pool deadline and then drops their cause.
    // Keep the first callback error in-band while it is still owned, but return an error to SQLx
    // as well so an uninitialized connection can never enter the pool.
    let (startup_error_sender, mut startup_error_receiver) = oneshot::channel();
    let startup_error_sender = Arc::new(Mutex::new(Some(startup_error_sender)));
    let key_for_connections = Arc::clone(&connection_key);
    let startup_error_for_connections = Arc::clone(&startup_error_sender);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .min_connections(0)
        .after_connect(move |connection, _metadata| {
            let key = Arc::clone(&key_for_connections);
            let startup_error_sender = Arc::clone(&startup_error_for_connections);
            Box::pin(async move {
                match configure_encrypted_connection(connection, &key).await {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        let sender = startup_error_sender
                            .lock()
                            .ok()
                            .and_then(|mut sender| sender.take());
                        match sender {
                            Some(sender) => match sender.send(error) {
                                Ok(()) => Err(sqlx::Error::Protocol(
                                    "encrypted Tally mirror initialization failed".into(),
                                )),
                                Err(error) => Err(error),
                            },
                            None => Err(error),
                        }
                    }
                }
            })
        })
        .connect_lazy_with(options);

    let mut initial_acquire = Box::pin(pool.acquire());
    let mut connection = tokio::select! {
        biased;
        startup_error = &mut startup_error_receiver => {
            drop(initial_acquire);
            pool.close().await;
            return match startup_error {
                Ok(error) => Err(encrypted_open_error(error).into()),
                Err(_) => Err(EncryptedOpenError::PoolTimedOut.into()),
            };
        }
        acquired = &mut initial_acquire => match acquired {
            Ok(connection) => connection,
            Err(error) => return Err(encrypted_open_error(error).into()),
        },
    };
    drop(startup_error_receiver);

    if let Err(error) = validate_encrypted_connection(&mut connection).await {
        let _ = connection.close().await;
        pool.close().await;
        return Err(error.into());
    }
    drop(connection);
    Ok(pool)
}

async fn configure_encrypted_connection(
    connection: &mut SqliteConnection,
    key: &[u8],
) -> Result<(), sqlx::Error> {
    {
        let mut handle = connection.lock_handle().await?;
        let key_length = i32::try_from(key.len())
            .map_err(|_| sqlx::Error::Protocol("SQLCipher key is too long".into()))?;
        // SAFETY: `lock_handle()` excludes the SQLx worker for the duration of this call, the
        // handle is live, and `key` remains allocated for the whole call. SQLCipher copies the
        // key into its per-connection codec state.
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
    // These settings must be applied after sqlite3_key(). Keeping them here means replacement
    // connections receive the same configuration without putting key material in connect options.
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
}

async fn validate_encrypted_connection(
    connection: &mut SqliteConnection,
) -> Result<(), EncryptedOpenError> {
    let cipher_version = sqlx::query_scalar::<_, String>("PRAGMA cipher_version;")
        .fetch_one(&mut *connection)
        .await
        .map_err(encrypted_open_error)?;
    if cipher_version.trim().is_empty() {
        return Err(EncryptedOpenError::CipherUnavailable);
    }
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_master;")
        .fetch_one(&mut *connection)
        .await
        .map_err(encrypted_open_error)?;
    let integrity_errors = sqlx::query_scalar::<_, String>("PRAGMA cipher_integrity_check;")
        .fetch_all(&mut *connection)
        .await
        .map_err(encrypted_open_error)?;
    if !integrity_errors.is_empty() {
        return Err(EncryptedOpenError::IntegrityFailed);
    }

    let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys;")
        .fetch_one(&mut *connection)
        .await
        .map_err(encrypted_open_error)?;
    let secure_delete = sqlx::query_scalar::<_, i64>("PRAGMA secure_delete;")
        .fetch_one(&mut *connection)
        .await
        .map_err(encrypted_open_error)?;
    if foreign_keys != 1 || secure_delete != 1 {
        return Err(EncryptedOpenError::SecuritySettings);
    }
    Ok(())
}

fn encrypted_open_error(error: sqlx::Error) -> EncryptedOpenError {
    if let sqlx::Error::Database(database_error) = &error {
        let code = database_error
            .code()
            .and_then(|code| code.parse::<i32>().ok());
        if code.is_some_and(|code| code & 0xff == libsqlite3_sys::SQLITE_NOTADB) {
            return EncryptedOpenError::UnreadableWithSuppliedKey { source: error };
        }
    }
    if matches!(error, sqlx::Error::PoolTimedOut) {
        return EncryptedOpenError::PoolTimedOut;
    }
    EncryptedOpenError::Storage { source: error }
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
