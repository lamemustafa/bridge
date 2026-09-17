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
