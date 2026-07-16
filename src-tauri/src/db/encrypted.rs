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
mod tests {
    use super::{connect_encrypted, resolve_mirror_key, MirrorKeyStore, MIRROR_KEY_BYTES};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct FakeKeyStore {
        key: Mutex<Option<Vec<u8>>>,
    }

    impl FakeKeyStore {
        fn with_key(key: Vec<u8>) -> Self {
            Self {
                key: Mutex::new(Some(key)),
            }
        }
    }

    impl MirrorKeyStore for FakeKeyStore {
        fn load(&self) -> anyhow::Result<Option<Zeroizing<Vec<u8>>>> {
            Ok(self
                .key
                .lock()
                .expect("fake key store lock")
                .clone()
                .map(Zeroizing::new))
        }

        fn save(&self, key: &[u8]) -> anyhow::Result<()> {
            *self.key.lock().expect("fake key store lock") = Some(key.to_vec());
            Ok(())
        }

        fn delete(&self) -> anyhow::Result<()> {
            *self.key.lock().expect("fake key store lock") = None;
            Ok(())
        }
    }

    fn mirror_artifacts(database: &Path) -> Vec<PathBuf> {
        let mut wal = database.as_os_str().to_os_string();
        wal.push("-wal");
        let mut shared_memory = database.as_os_str().to_os_string();
        shared_memory.push("-shm");
        vec![
            database.to_path_buf(),
            PathBuf::from(wal),
            PathBuf::from(shared_memory),
        ]
    }

    fn assert_no_plaintext_artifacts(database: &Path) {
        for artifact in mirror_artifacts(database) {
            if !artifact.exists() {
                continue;
            }
            let bytes = fs::read(&artifact).expect("read mirror artifact");
            for marker in [
                b"SQLite format 3".as_slice(),
                b"SENSITIVE_TALLY_MARKER".as_slice(),
                b"1111111111111111111111111111111111111111111111111111111111111111".as_slice(),
            ] {
                assert!(
                    !bytes.windows(marker.len()).any(|window| window == marker),
                    "plaintext marker found in {}",
                    artifact.display()
                );
            }
        }
    }

    #[test]
    fn creates_a_key_only_for_a_new_mirror() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("mirror.db");
        let store = FakeKeyStore::default();
        let resolved = resolve_mirror_key(&database, &store).expect("resolve new key");
        assert!(resolved.created);
        assert_eq!(resolved.key.len(), MIRROR_KEY_BYTES);
        assert!(store.load().expect("reload key").is_some());
    }

    #[test]
    fn missing_or_invalid_key_for_existing_mirror_fails_closed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("mirror.db");
        fs::write(&database, b"existing mirror marker").expect("create existing mirror");
        let missing = match resolve_mirror_key(&database, &FakeKeyStore::default()) {
            Ok(_) => panic!("missing key must not be replaced"),
            Err(error) => error,
        };
        assert!(missing.to_string().contains("explicit local reset"));

        let invalid_store = FakeKeyStore::with_key(vec![7_u8; 16]);
        let invalid = match resolve_mirror_key(&database, &invalid_store) {
            Ok(_) => panic!("invalid key must not be replaced"),
            Err(error) => error,
        };
        assert!(invalid.to_string().contains("explicit local reset"));
    }

    #[tokio::test]
    async fn sqlcipher_encrypts_contents_and_rejects_the_wrong_key() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("mirror.db");
        let key = Zeroizing::new(vec![0x11_u8; MIRROR_KEY_BYTES]);
        let pool = connect_encrypted(&database, key.clone())
            .await
            .expect("open encrypted mirror");
        let retained_options = format!("{:?}", pool.connect_options());
        assert!(
            !retained_options
                .contains("1111111111111111111111111111111111111111111111111111111111111111"),
            "pool connection options must not retain a hexadecimal copy of the key"
        );
        sqlx::query("CREATE TABLE proof(value TEXT NOT NULL);")
            .execute(&pool)
            .await
            .expect("create encrypted table");
        sqlx::query("INSERT INTO proof(value) VALUES (?1);")
            .bind("SENSITIVE_TALLY_MARKER")
            .execute(&pool)
            .await
            .expect("insert encrypted value");

        // Make SQLx open the full pool, close every live connection, then require a replacement.
        // This proves that per-connection keying works without retaining the key in connect
        // options, including after the initial connection has gone away.
        let mut connections = Vec::new();
        for _ in 0..5 {
            connections.push(pool.acquire().await.expect("acquire encrypted connection"));
        }
        for mut connection in connections {
            let value = sqlx::query_scalar::<_, String>("SELECT value FROM proof;")
                .fetch_one(&mut *connection)
                .await
                .expect("read through encrypted pooled connection");
            assert_eq!(value, "SENSITIVE_TALLY_MARKER");
            connection
                .close()
                .await
                .expect("close encrypted pooled connection");
        }
        let replacement_value = sqlx::query_scalar::<_, String>("SELECT value FROM proof;")
            .fetch_one(&pool)
            .await
            .expect("read through replacement encrypted connection");
        assert_eq!(replacement_value, "SENSITIVE_TALLY_MARKER");

        assert_no_plaintext_artifacts(&database);
        pool.close().await;

        assert_no_plaintext_artifacts(&database);
        let before_wrong_key = fs::read(&database).expect("read encrypted database before retry");

        let wrong_key = Zeroizing::new(vec![0x22_u8; MIRROR_KEY_BYTES]);
        assert!(connect_encrypted(&database, wrong_key).await.is_err());
        assert_eq!(
            fs::read(&database).expect("read encrypted database after wrong key"),
            before_wrong_key,
            "a wrong-key attempt must not mutate the encrypted mirror"
        );
        assert_no_plaintext_artifacts(&database);

        let reopened = connect_encrypted(&database, key)
            .await
            .expect("reopen with correct key");
        let value = sqlx::query_scalar::<_, String>("SELECT value FROM proof;")
            .fetch_one(&reopened)
            .await
            .expect("read encrypted value");
        assert_eq!(value, "SENSITIVE_TALLY_MARKER");
        reopened.close().await;
    }
}
