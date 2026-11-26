use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::State;
use url::Url;
use zeroize::{Zeroize, Zeroizing};

use crate::storage::StorageService;

const MAX_ORIGIN_LENGTH: usize = 2048;
const MAX_USERNAME_LENGTH: usize = 512;
const MAX_PASSWORD_LENGTH: usize = 4096;
const MAX_SECRET_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordEntry {
    id: i64,
    origin: String,
    username: String,
    updated_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealedPassword {
    origin: String,
    username: String,
    password: String,
}

#[derive(Serialize, Deserialize)]
struct CredentialSecret {
    username: String,
    password: String,
}

impl Drop for CredentialSecret {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}

#[tauri::command]
pub async fn passwords_list(
    storage: State<'_, StorageService>,
) -> Result<Vec<PasswordEntry>, String> {
    storage
        .with_connection(|connection| list_credentials(connection))
        .await
}

#[tauri::command]
pub async fn passwords_save(
    origin: String,
    username: String,
    password: String,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    let origin = validate_origin(&origin)?;
    let username = validate_username(username)?;
    let password = validate_password(password)?;
    storage
        .with_connection(move |connection| {
            save_credential(connection, &origin, username, password, unix_timestamp())
        })
        .await
}

#[tauri::command]
pub async fn passwords_reveal(
    id: i64,
    storage: State<'_, StorageService>,
) -> Result<RevealedPassword, String> {
    storage
        .with_connection(move |connection| reveal_credential(connection, id))
        .await
}

#[tauri::command]
pub async fn passwords_delete(id: i64, storage: State<'_, StorageService>) -> Result<(), String> {
    storage
        .with_connection(move |connection| {
            let changed = connection
                .execute("DELETE FROM password_credentials WHERE id = ?1", [id])
                .map_err(db_error)?;
            if changed == 0 {
                return Err("The saved password no longer exists.".to_string());
            }
            Ok(())
        })
        .await
}

fn list_credentials(connection: &Connection) -> Result<Vec<PasswordEntry>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, origin, encrypted_secret, updated_at
             FROM password_credentials ORDER BY origin COLLATE NOCASE, updated_at DESC",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(db_error)?;
    let mut entries = Vec::new();
    for row in rows {
        let (id, origin, encrypted_secret, updated_at) = row.map_err(db_error)?;
        let mut secret = decrypt_secret(&encrypted_secret)?;
        entries.push(PasswordEntry {
            id,
            origin,
            username: std::mem::take(&mut secret.username),
            updated_at,
        });
    }
    Ok(entries)
}

fn save_credential(
    connection: &Connection,
    origin: &str,
    username: String,
    password: String,
    updated_at: i64,
) -> Result<(), String> {
    let username = Zeroizing::new(username);
    let password = Zeroizing::new(password);
    let encrypted_secret = encrypt_secret(&CredentialSecret {
        username: username.to_string(),
        password: password.to_string(),
    });
    let encrypted_secret = encrypted_secret?;
    let transaction =
        rusqlite::Transaction::new_unchecked(connection, rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
    let existing = {
        let mut statement = transaction
            .prepare("SELECT id, encrypted_secret FROM password_credentials WHERE origin = ?1")
            .map_err(db_error)?;
        let rows = statement
            .query_map([origin], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?
    };
    let mut existing_id = None;
    for (id, stored_secret) in existing {
        if decrypt_secret(&stored_secret)?.username == username.as_str() {
            if existing_id.is_some() {
                transaction
                    .execute("DELETE FROM password_credentials WHERE id = ?1", [id])
                    .map_err(db_error)?;
            } else {
                existing_id = Some(id);
            }
        }
    }
    if let Some(id) = existing_id {
        transaction
            .execute(
                "UPDATE password_credentials
                 SET encrypted_secret = ?1, updated_at = ?2 WHERE id = ?3",
                params![encrypted_secret, updated_at, id],
            )
            .map_err(db_error)?;
    } else {
        transaction
            .execute(
                "INSERT INTO password_credentials(origin, encrypted_secret, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![origin, encrypted_secret, updated_at],
            )
            .map_err(db_error)?;
    }
    transaction.commit().map_err(db_error)
}

fn reveal_credential(connection: &Connection, id: i64) -> Result<RevealedPassword, String> {
    let (origin, encrypted_secret) = connection
        .query_row(
            "SELECT origin, encrypted_secret FROM password_credentials WHERE id = ?1",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                "The saved password no longer exists.".to_string()
            }
            error => db_error(error),
        })?;
    let secret = decrypt_secret(&encrypted_secret)?;
    let mut secret = secret;
    Ok(RevealedPassword {
        origin,
        username: std::mem::take(&mut secret.username),
        password: std::mem::take(&mut secret.password),
    })
}

fn validate_origin(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| "Enter a valid HTTPS website origin.".to_string())?;
    if url.scheme() != "https"
        || url.host().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("Passwords can only be saved for an HTTPS website origin.".to_string());
    }
    let origin = url.origin().ascii_serialization();
    if origin.len() > MAX_ORIGIN_LENGTH {
        return Err("The website origin is too long.".to_string());
    }
    Ok(origin)
}

fn validate_username(username: String) -> Result<String, String> {
    let username = username.trim().to_string();
    if username.is_empty() || username.chars().count() > MAX_USERNAME_LENGTH {
        return Err("Username must contain between 1 and 512 characters.".to_string());
    }
    Ok(username)
}

fn validate_password(password: String) -> Result<String, String> {
    if password.is_empty() || password.chars().count() > MAX_PASSWORD_LENGTH {
        return Err("Password must contain between 1 and 4096 characters.".to_string());
    }
    Ok(password)
}

fn encrypt_secret(secret: &CredentialSecret) -> Result<Vec<u8>, String> {
    let mut plaintext = serde_json::to_vec(secret)
        .map_err(|error| format!("could not serialize password entry: {error}"))?;
    if plaintext.len() > MAX_SECRET_BYTES {
        plaintext.fill(0);
        return Err("Password entry exceeds the protected storage limit.".to_string());
    }
    let protected = protect_with_os(&plaintext);
    plaintext.fill(0);
    protected
}

fn decrypt_secret(ciphertext: &[u8]) -> Result<CredentialSecret, String> {
    let mut plaintext = unprotect_with_os(ciphertext)?;
    let secret = serde_json::from_slice(&plaintext)
        .map_err(|error| format!("could not decode protected password entry: {error}"));
    plaintext.fill(0);
    secret
}

#[cfg(windows)]
fn protect_with_os(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    use std::slice;

    use windows::{
        core::w,
        Win32::{
            Foundation::{LocalFree, HLOCAL},
            Security::Cryptography::{
                CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
            },
        },
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plaintext.len())
            .map_err(|_| "Password entry is too large to protect.".to_string())?,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            w!("Eefi Browser password vault"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| format!("Windows could not protect the password entry: {error}"))?;
    }
    if output.pbData.is_null() || output.cbData == 0 {
        if !output.pbData.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            }
        }
        return Err("Windows returned an empty protected password entry.".to_string());
    }
    let protected =
        unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
    }
    Ok(protected)
}

#[cfg(not(windows))]
fn protect_with_os(_plaintext: &[u8]) -> Result<Vec<u8>, String> {
    Err("The password vault requires Windows Data Protection.".to_string())
}

#[cfg(windows)]
fn unprotect_with_os(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    use std::slice;

    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };

    if ciphertext.len() > MAX_SECRET_BYTES + 1024 {
        return Err("Protected password entry exceeds the storage limit.".to_string());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(ciphertext.len())
            .map_err(|_| "Protected password entry is too large.".to_string())?,
        pbData: ciphertext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| format!("Windows could not unlock the password entry: {error}"))?;
    }
    if output.pbData.is_null() || output.cbData == 0 {
        if !output.pbData.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            }
        }
        return Err("Windows returned an empty unlocked password entry.".to_string());
    }
    let plaintext =
        unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
    }
    Ok(plaintext)
}

#[cfg(not(windows))]
fn unprotect_with_os(_ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    Err("The password vault requires Windows Data Protection.".to_string())
}

fn db_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_https_origins_and_rejects_credentials_and_paths() {
        assert_eq!(
            validate_origin("https://example.com/").expect("valid origin"),
            "https://example.com"
        );
        assert!(validate_origin("http://example.com/").is_err());
        assert!(validate_origin("https://user:pass@example.com/").is_err());
        assert!(validate_origin("https://example.com/login").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn operating_system_protection_round_trips_and_rejects_tampering() {
        let secret = CredentialSecret {
            username: "user@example.com".to_string(),
            password: "a long secret".to_string(),
        };
        let ciphertext = encrypt_secret(&secret).expect("protect secret");
        assert_ne!(
            ciphertext,
            serde_json::to_vec(&secret).expect("serialize secret")
        );
        let restored = decrypt_secret(&ciphertext).expect("unprotect secret");
        assert_eq!(restored.username, secret.username);
        assert_eq!(restored.password, secret.password);

        let mut corrupted = ciphertext;
        corrupted[0] ^= 0xff;
        assert!(decrypt_secret(&corrupted).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn vault_crud_keeps_credentials_encrypted_at_rest() {
        let connection = Connection::open_in_memory().expect("create test database");
        connection
            .execute_batch(
                "CREATE TABLE password_credentials (
                    id INTEGER PRIMARY KEY,
                    origin TEXT NOT NULL,
                    encrypted_secret BLOB NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            )
            .expect("create password table");
        save_credential(
            &connection,
            "https://example.com",
            "alice".to_string(),
            "correct horse battery staple".to_string(),
            100,
        )
        .expect("save initial credential");

        let stored: Vec<u8> = connection
            .query_row(
                "SELECT encrypted_secret FROM password_credentials",
                [],
                |row| row.get(0),
            )
            .expect("read encrypted bytes");
        assert!(!stored
            .windows(b"correct horse battery staple".len())
            .any(|window| window == b"correct horse battery staple"));

        let listed = list_credentials(&connection).expect("list credentials");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].username, "alice");
        let updated_id = listed[0].id;
        save_credential(
            &connection,
            "https://example.com",
            "alice".to_string(),
            "new secret".to_string(),
            200,
        )
        .expect("update existing credential");
        let updated = list_credentials(&connection).expect("list updated credentials");
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].id, updated_id);
        assert_eq!(
            reveal_credential(&connection, updated[0].id)
                .unwrap()
                .password,
            "new secret"
        );
    }
}
