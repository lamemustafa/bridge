use super::{
    configure_encrypted_connection, connect_encrypted, encrypted_open_error,
    is_definitive_encrypted_open_error, is_definitive_encrypted_open_sqlite_code,
    is_forwardable_encrypted_open_sqlite_code, resolve_mirror_key, EncryptedOpenError,
    MirrorKeyStore, MIRROR_KEY_BYTES,
};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, SqliteConnection};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
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

async fn open_encrypted_connection(database: &Path, key: &[u8]) -> SqliteConnection {
    let options = SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(true)
        .disable_statement_logging();
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("open synthetic encrypted SQLite connection");
    configure_encrypted_connection(&mut connection, key)
        .await
        .expect("configure synthetic encrypted SQLite connection");
    connection
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
    let cipher_log_level = sqlx::query_scalar::<_, String>("PRAGMA cipher_log_level;")
        .fetch_one(&pool)
        .await
        .expect("read SQLCipher log level");
    assert_eq!(cipher_log_level, "ERROR");
    let cipher_memory_security = sqlx::query_scalar::<_, String>("PRAGMA cipher_memory_security;")
        .fetch_one(&pool)
        .await
        .expect("read SQLCipher memory security state");
    assert_eq!(cipher_memory_security, "1");
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
        let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys;")
            .fetch_one(&mut *connection)
            .await
            .expect("read replacement foreign-key setting");
        let secure_delete = sqlx::query_scalar::<_, i64>("PRAGMA secure_delete;")
            .fetch_one(&mut *connection)
            .await
            .expect("read replacement secure-delete setting");
        assert_eq!(foreign_keys, 1);
        assert_eq!(secure_delete, 1);
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
    let wrong_key_started = Instant::now();
    let wrong_key_error = connect_encrypted(&database, wrong_key)
        .await
        .expect_err("wrong key must refuse the encrypted mirror");
    assert!(
        wrong_key_started.elapsed() < Duration::from_secs(5),
        "wrong key must refuse before SQLx's pool retry deadline"
    );
    let source = match wrong_key_error.downcast_ref::<EncryptedOpenError>() {
        Some(EncryptedOpenError::UnreadableWithSuppliedKey { source }) => source,
        _ => panic!("wrong key must retain a typed SQLite refusal"),
    };
    assert!(matches!(
        source,
        sqlx::Error::Database(error) if error.code().as_deref() == Some("26")
    ));
    assert!(
        !format!("{wrong_key_error:?}")
            .contains("2222222222222222222222222222222222222222222222222222222222222222"),
        "wrong-key failure must not render key material"
    );
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

#[tokio::test]
async fn corrupt_encrypted_contents_fail_with_a_typed_refusal() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("mirror.db");
    let corrupt_database = directory.path().join("corrupt-mirror.db");
    let key = Zeroizing::new(vec![0x33_u8; MIRROR_KEY_BYTES]);
    let pool = connect_encrypted(&database, key.clone())
        .await
        .expect("open encrypted mirror");
    sqlx::query("CREATE TABLE proof(value BLOB NOT NULL);")
        .execute(&pool)
        .await
        .expect("create encrypted table");
    sqlx::query("INSERT INTO proof(value) VALUES (?1);")
        .bind(vec![0x5a_u8; 64 * 1024])
        .execute(&pool)
        .await
        .expect("write encrypted payload");
    pool.close().await;

    let mut corrupt_bytes = fs::read(&database).expect("read encrypted database");
    let middle = corrupt_bytes.len() / 2;
    corrupt_bytes[middle] ^= 0x01;
    fs::write(&corrupt_database, corrupt_bytes).expect("write corrupted encrypted database");

    let error = connect_encrypted(&corrupt_database, key)
        .await
        .expect_err("corrupt encrypted contents must refuse to open");
    assert!(matches!(
        error.downcast_ref::<EncryptedOpenError>(),
        Some(EncryptedOpenError::UnreadableWithSuppliedKey { .. })
            | Some(EncryptedOpenError::IntegrityFailed)
    ));
}

#[tokio::test]
async fn unrelated_storage_failure_is_not_a_key_refusal() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database_directory = directory.path().join("not-a-database");
    fs::create_dir(&database_directory).expect("create invalid database path");

    let error = connect_encrypted(
        &database_directory,
        Zeroizing::new(vec![0x44_u8; MIRROR_KEY_BYTES]),
    )
    .await
    .expect_err("directory path cannot open as a database");
    assert!(matches!(
        error.downcast_ref::<EncryptedOpenError>(),
        Some(EncryptedOpenError::Storage { .. })
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn readonly_directory_returns_typed_storage_without_resetting_the_mirror() {
    use std::os::unix::fs::PermissionsExt;

    struct RestoreDirectoryMode {
        directory: PathBuf,
        mode: u32,
    }

    impl Drop for RestoreDirectoryMode {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.directory, fs::Permissions::from_mode(self.mode));
        }
    }

    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("mirror.db");
    let key = Zeroizing::new(vec![0x66_u8; MIRROR_KEY_BYTES]);
    let pool = connect_encrypted(&database, key.clone())
        .await
        .expect("create synthetic encrypted mirror");
    sqlx::query("CREATE TABLE readonly_regression(value TEXT NOT NULL);")
        .execute(&pool)
        .await
        .expect("create synthetic readonly-regression table");
    sqlx::query("INSERT INTO readonly_regression(value) VALUES (?1);")
        .bind("READONLY_DIRECTORY_MARKER")
        .execute(&pool)
        .await
        .expect("write synthetic readonly-regression marker");
    pool.close().await;

    for artifact in mirror_artifacts(&database).into_iter().skip(1) {
        if artifact.exists() {
            fs::remove_file(artifact).expect("remove synthetic WAL artifact before restriction");
        }
    }
    let before = fs::read(&database).expect("read encrypted mirror before restriction");
    let original_mode = fs::metadata(directory.path())
        .expect("read temporary directory mode")
        .permissions()
        .mode();
    let restore_mode = RestoreDirectoryMode {
        directory: directory.path().to_path_buf(),
        mode: original_mode,
    };
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500))
        .expect("deny writes to synthetic mirror directory");

    let direct_options = SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(true)
        .disable_statement_logging();
    let mut direct_connection = SqliteConnection::connect_with(&direct_options)
        .await
        .expect("open synthetic encrypted mirror before callback configuration");
    let callback_error = configure_encrypted_connection(&mut direct_connection, &key)
        .await
        .expect_err("readonly directory must reject SQLCipher callback configuration");
    direct_connection
        .close()
        .await
        .expect("close synthetic direct encrypted connection");

    let opened_at = Instant::now();
    let open_error = connect_encrypted(&database, key.clone())
        .await
        .expect_err("readonly directory must not open the encrypted mirror");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(original_mode))
        .expect("restore synthetic mirror directory mode before assertions");
    drop(restore_mode);

    assert!(matches!(
        callback_error,
        sqlx::Error::Database(database_error)
            if database_error.code().is_some_and(|code|
                code == libsqlite3_sys::SQLITE_READONLY_DIRECTORY.to_string())
    ));
    assert!(
        opened_at.elapsed() < Duration::from_secs(5),
        "the verified readonly directory refusal must not wait for SQLx's pool retry deadline"
    );
    assert!(matches!(
        open_error.downcast_ref::<EncryptedOpenError>(),
        Some(EncryptedOpenError::Storage { source })
            if matches!(
                source,
                sqlx::Error::Database(database_error)
                    if database_error.code().is_some_and(|code|
                        code == libsqlite3_sys::SQLITE_READONLY_DIRECTORY.to_string())
            )
    ));
    assert_eq!(
        fs::read(&database).expect("read encrypted mirror after readonly refusal"),
        before,
        "readonly refusal must not reset or overwrite the encrypted mirror"
    );
    let reopened = connect_encrypted(&database, key)
        .await
        .expect("reopen the unchanged synthetic mirror after restoring its directory");
    let marker = sqlx::query_scalar::<_, String>("SELECT value FROM readonly_regression;")
        .fetch_one(&reopened)
        .await
        .expect("read marker after readonly refusal");
    assert_eq!(marker, "READONLY_DIRECTORY_MARKER");
    reopened.close().await;
}

#[tokio::test]
async fn encrypted_open_retries_a_transient_sqlite_lock() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("mirror.db");
    let key = Zeroizing::new(vec![0x55_u8; MIRROR_KEY_BYTES]);
    let initial_pool = connect_encrypted(&database, key.clone())
        .await
        .expect("create encrypted mirror for lock holder");
    sqlx::query("CREATE TABLE lock_regression(value TEXT NOT NULL);")
        .execute(&initial_pool)
        .await
        .expect("create synthetic encrypted lock-regression table");
    sqlx::query("INSERT INTO lock_regression(value) VALUES (?1);")
        .bind("LOCK_REGRESSION_MARKER")
        .execute(&initial_pool)
        .await
        .expect("write synthetic encrypted lock-regression marker");
    initial_pool.close().await;
    let mut holder = open_encrypted_connection(&database, &key).await;
    let rollback_journal = sqlx::query_scalar::<_, String>("PRAGMA journal_mode = DELETE;")
        .fetch_one(&mut holder)
        .await
        .expect("use rollback journal for the exclusive-lock regression");
    assert_eq!(rollback_journal.to_ascii_lowercase(), "delete");
    sqlx::query("BEGIN EXCLUSIVE;")
        .execute(&mut holder)
        .await
        .expect("hold synthetic exclusive lock");

    let opener_path = database.clone();
    let opener = tokio::spawn(async move { connect_encrypted(&opener_path, key).await });
    tokio::time::sleep(Duration::from_secs(6)).await;

    if opener.is_finished() {
        let early_result = opener.await;
        sqlx::query("COMMIT;")
            .execute(&mut holder)
            .await
            .expect("release synthetic exclusive lock after early encrypted-open result");
        holder
            .close()
            .await
            .expect("close synthetic lock holder after early encrypted-open result");
        let error = early_result
            .expect("encrypted opener task must not panic")
            .expect_err("transient lock must not complete encrypted open before release");
        assert!(matches!(
            error.downcast_ref::<EncryptedOpenError>(),
            Some(EncryptedOpenError::Storage { source })
                if matches!(
                    source,
                    sqlx::Error::Database(database_error)
                        if database_error.code().and_then(|code| code.parse::<i32>().ok())
                            .is_some_and(|code| {
                                matches!(
                                    code & 0xff,
                                    libsqlite3_sys::SQLITE_BUSY | libsqlite3_sys::SQLITE_LOCKED
                                )
                            })
                )
        ));
        panic!("transient SQLite lock must remain on SQLx's retry path");
    }

    sqlx::query("COMMIT;")
        .execute(&mut holder)
        .await
        .expect("release synthetic exclusive lock");
    holder.close().await.expect("close synthetic lock holder");

    let opened = tokio::time::timeout(Duration::from_secs(10), opener)
        .await
        .expect("encrypted open must finish before SQLx's pool deadline")
        .expect("encrypted opener task must not panic")
        .expect("encrypted open must succeed after the lock is released");
    let marker = sqlx::query_scalar::<_, String>("SELECT value FROM lock_regression;")
        .fetch_one(&opened)
        .await
        .expect("read marker after transient encrypted-open retry");
    assert_eq!(marker, "LOCK_REGRESSION_MARKER");
    let cipher_log_level = sqlx::query_scalar::<_, String>("PRAGMA cipher_log_level;")
        .fetch_one(&opened)
        .await
        .expect("read SQLCipher log level after transient encrypted-open retry");
    let cipher_memory_security = sqlx::query_scalar::<_, String>("PRAGMA cipher_memory_security;")
        .fetch_one(&opened)
        .await
        .expect("read SQLCipher memory security after transient encrypted-open retry");
    let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys;")
        .fetch_one(&opened)
        .await
        .expect("read foreign-key setting after transient encrypted-open retry");
    let secure_delete = sqlx::query_scalar::<_, i64>("PRAGMA secure_delete;")
        .fetch_one(&opened)
        .await
        .expect("read secure-delete setting after transient encrypted-open retry");
    let journal_mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode;")
        .fetch_one(&opened)
        .await
        .expect("read journal mode after transient encrypted-open retry");
    assert_eq!(cipher_log_level, "ERROR");
    assert_eq!(cipher_memory_security, "1");
    assert_eq!(foreign_keys, 1);
    assert_eq!(secure_delete, 1);
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    opened.close().await;
}

#[test]
fn pool_timeout_is_not_classified_as_an_encrypted_contents_refusal() {
    assert!(matches!(
        encrypted_open_error(sqlx::Error::PoolTimedOut),
        EncryptedOpenError::PoolTimedOut
    ));
}

#[test]
fn only_notadb_is_an_unreadable_key_or_ciphertext_refusal() {
    assert!(!is_definitive_encrypted_open_error(
        &sqlx::Error::PoolTimedOut
    ));
    assert!(is_definitive_encrypted_open_sqlite_code(
        libsqlite3_sys::SQLITE_NOTADB
    ));
    for extended_code in [
        libsqlite3_sys::SQLITE_BUSY,
        libsqlite3_sys::SQLITE_BUSY_RECOVERY,
        libsqlite3_sys::SQLITE_BUSY_SNAPSHOT,
        libsqlite3_sys::SQLITE_BUSY_TIMEOUT,
        libsqlite3_sys::SQLITE_LOCKED,
        libsqlite3_sys::SQLITE_LOCKED_SHAREDCACHE,
        libsqlite3_sys::SQLITE_LOCKED_VTAB,
    ] {
        assert!(
            !is_definitive_encrypted_open_sqlite_code(extended_code),
            "transient SQLite code must not be a definitive encrypted-open refusal"
        );
    }
}

#[test]
fn only_verified_sqlite_codes_are_forwarded_from_encrypted_initialization() {
    assert!(is_forwardable_encrypted_open_sqlite_code(
        libsqlite3_sys::SQLITE_NOTADB
    ));
    assert!(is_forwardable_encrypted_open_sqlite_code(
        libsqlite3_sys::SQLITE_READONLY_DIRECTORY
    ));
    for code in [
        libsqlite3_sys::SQLITE_READONLY,
        libsqlite3_sys::SQLITE_READONLY_RECOVERY,
        libsqlite3_sys::SQLITE_READONLY_CANTLOCK,
        libsqlite3_sys::SQLITE_READONLY_ROLLBACK,
        libsqlite3_sys::SQLITE_READONLY_DBMOVED,
        libsqlite3_sys::SQLITE_READONLY_CANTINIT,
        libsqlite3_sys::SQLITE_BUSY,
        libsqlite3_sys::SQLITE_BUSY_RECOVERY,
        libsqlite3_sys::SQLITE_BUSY_SNAPSHOT,
        libsqlite3_sys::SQLITE_BUSY_TIMEOUT,
        libsqlite3_sys::SQLITE_LOCKED,
        libsqlite3_sys::SQLITE_LOCKED_SHAREDCACHE,
        libsqlite3_sys::SQLITE_LOCKED_VTAB,
        libsqlite3_sys::SQLITE_PROTOCOL,
    ] {
        assert!(
            !is_forwardable_encrypted_open_sqlite_code(code),
            "unverified or transient SQLite code must remain on SQLx's retry path"
        );
    }
}
