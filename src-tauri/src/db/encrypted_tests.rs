use super::{
    connect_encrypted, encrypted_open_error, resolve_mirror_key, EncryptedOpenError,
    MirrorKeyStore, MIRROR_KEY_BYTES,
};
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

#[test]
fn pool_timeout_is_not_classified_as_an_encrypted_contents_refusal() {
    assert!(matches!(
        encrypted_open_error(sqlx::Error::PoolTimedOut),
        EncryptedOpenError::PoolTimedOut
    ));
}
