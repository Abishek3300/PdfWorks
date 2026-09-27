//! Encrypted File_Store addressed by Job_Token (Task 16.1, Req 43.3, 43.4,
//! 43.5, 44.1, 50.2).
//!
//! Source_Files and Output_Files are stored **encrypted at rest** (AES-256-GCM
//! with a per-deployment key, Req 44.1) and addressed only by an opaque key
//! that is scoped to the Job_Token (Req 43.3). The store never exposes a
//! directory listing (Req 43.5). On download, callers receive a
//! `Content-Disposition: attachment` header and a non-executable `Content-Type`
//! (Req 50.2).
//!
//! The backing store is behind the [`ObjectStore`] trait so tests use an
//! in-memory implementation and production uses an S3-compatible client; the
//! encryption layer ([`EncryptedFileStore`]) wraps either one so at-rest
//! ciphertext is identical across backends.

use std::collections::HashMap;
use std::sync::Mutex;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use async_trait::async_trait;

/// Errors from the File_Store layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The requested object key does not exist.
    NotFound,
    /// A directory listing was requested and refused (Req 43.5).
    ListingForbidden,
    /// The backing store is out of space (Req 49.4).
    StorageExhausted,
    /// Decryption/authentication failed (tampered or wrong-key ciphertext).
    Crypto,
    /// The backing store failed for another reason.
    Backend(String),
}

/// A stored object's on-download presentation metadata (Req 50.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadHeaders {
    /// Always `attachment; filename="..."` with a sanitized name.
    pub content_disposition: String,
    /// A non-executable content type (Req 50.2).
    pub content_type: String,
}

impl DownloadHeaders {
    /// Build attachment headers for `display_name`, forcing a non-executable
    /// content type so the browser cannot interpret the body as HTML/script
    /// (Req 50.2). The name is expected to already be sanitized by the engine's
    /// `sanitize_filename` (Req 50.1).
    #[must_use]
    pub fn for_download(display_name: &str, content_type: &str) -> Self {
        // Refuse to serve an executable/HTML content type; coerce to a safe one.
        let safe_type = if is_executable_content_type(content_type) {
            "application/octet-stream"
        } else {
            content_type
        };
        Self {
            content_disposition: format!("attachment; filename=\"{display_name}\""),
            content_type: safe_type.to_string(),
        }
    }
}

/// Whether a content type would let a browser execute the response as markup.
fn is_executable_content_type(ct: &str) -> bool {
    let lower = ct.to_ascii_lowercase();
    lower.starts_with("text/html")
        || lower.starts_with("application/xhtml")
        || lower.contains("javascript")
        || lower.starts_with("image/svg")
}

/// An opaque store key scoped to a Job_Token, so one Job cannot address
/// another's files (Req 43.3). The Job_Token is never a filesystem path.
#[must_use]
pub fn scoped_key(job_token: &str, file_id: &str) -> String {
    // Prefixing with the token namespaces every object under its Job. Both
    // components are opaque identifiers (no path separators), so the composed
    // key cannot traverse the store (Req 50.1 defense-in-depth).
    format!("{job_token}/{file_id}")
}

/// A raw byte-addressed object store (the backend the encryption layer wraps).
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// Store `bytes` under `key`, overwriting any existing value.
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), StoreError>;
    /// Fetch the bytes stored under `key`.
    async fn get(&self, key: &str) -> Result<Vec<u8>, StoreError>;
    /// Delete `key`. Succeeds even if the key was absent.
    async fn delete(&self, key: &str) -> Result<(), StoreError>;
    /// Delete every object whose key begins with `prefix`; returns the count.
    async fn delete_prefix(&self, prefix: &str) -> Result<usize, StoreError>;
    /// Explicitly reject directory listing (Req 43.5). Always errors.
    async fn list(&self, _prefix: &str) -> Result<Vec<String>, StoreError> {
        Err(StoreError::ListingForbidden)
    }
}

/// An in-memory [`ObjectStore`] for tests and single-node dev.
#[derive(Default)]
pub struct InMemoryObjectStore {
    map: Mutex<HashMap<String, Vec<u8>>>,
    /// Optional cap to simulate storage exhaustion (Req 49.4 testing).
    capacity_bytes: Option<usize>,
}

impl InMemoryObjectStore {
    /// A store with unbounded capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that rejects writes once total stored bytes exceed `cap`.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            capacity_bytes: Some(cap),
        }
    }

    fn total_bytes(map: &HashMap<String, Vec<u8>>) -> usize {
        map.values().map(Vec::len).sum()
    }
}

#[async_trait]
impl ObjectStore for InMemoryObjectStore {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), StoreError> {
        let mut map = self
            .map
            .lock()
            .map_err(|_| StoreError::Backend("lock poisoned".to_string()))?;
        if let Some(cap) = self.capacity_bytes {
            let existing = map.get(key).map_or(0, Vec::len);
            let projected = Self::total_bytes(&map) - existing + bytes.len();
            if projected > cap {
                return Err(StoreError::StorageExhausted);
            }
        }
        map.insert(key.to_string(), bytes);
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StoreError> {
        let map = self
            .map
            .lock()
            .map_err(|_| StoreError::Backend("lock poisoned".to_string()))?;
        map.get(key).cloned().ok_or(StoreError::NotFound)
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        let mut map = self
            .map
            .lock()
            .map_err(|_| StoreError::Backend("lock poisoned".to_string()))?;
        map.remove(key);
        Ok(())
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<usize, StoreError> {
        let mut map = self
            .map
            .lock()
            .map_err(|_| StoreError::Backend("lock poisoned".to_string()))?;
        let keys: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        let n = keys.len();
        for k in keys {
            map.remove(&k);
        }
        Ok(n)
    }
}

/// AES-256-GCM encryption layer over any [`ObjectStore`].
///
/// Each object is sealed with a fresh 96-bit nonce prepended to the ciphertext,
/// so identical plaintexts produce distinct ciphertexts and tampering is
/// detected on read (Req 44.1). The nonce is supplied by the caller (drawn from
/// the engine's CSPRNG helper at the call site) to keep this layer pure.
pub struct EncryptedFileStore<S: ObjectStore> {
    backend: S,
    cipher: Aes256Gcm,
}

impl<S: ObjectStore> EncryptedFileStore<S> {
    /// Wrap `backend`, encrypting with the per-deployment 256-bit `key`
    /// (Req 44.1).
    #[must_use]
    pub fn new(backend: S, key: &[u8; 32]) -> Self {
        let key = Key::<Aes256Gcm>::from_slice(key);
        Self {
            backend,
            cipher: Aes256Gcm::new(key),
        }
    }

    /// Encrypt `plaintext` under a fresh `nonce` and store it at `key`.
    ///
    /// The stored blob is `nonce (12 bytes) || ciphertext`. Reusing a nonce
    /// with the same key would be catastrophic for GCM, so callers must pass a
    /// unique random nonce per write.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Crypto`] on encryption failure, or a backend error.
    pub async fn put_encrypted(
        &self,
        key: &str,
        nonce: &[u8; 12],
        plaintext: &[u8],
    ) -> Result<(), StoreError> {
        let nonce_obj = Nonce::from_slice(nonce);
        let ciphertext = self
            .cipher
            .encrypt(nonce_obj, plaintext)
            .map_err(|_| StoreError::Crypto)?;
        let mut blob = Vec::with_capacity(12 + ciphertext.len());
        blob.extend_from_slice(nonce);
        blob.extend_from_slice(&ciphertext);
        self.backend.put(key, blob).await
    }

    /// Fetch and decrypt the object at `key`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Crypto`] if the blob is malformed or fails
    /// authentication (tampering / wrong key), or a backend error.
    pub async fn get_decrypted(&self, key: &str) -> Result<Vec<u8>, StoreError> {
        let blob = self.backend.get(key).await?;
        if blob.len() < 12 {
            return Err(StoreError::Crypto);
        }
        let (nonce_bytes, ciphertext) = blob.split_at(12);
        let nonce_obj = Nonce::from_slice(nonce_bytes);
        self.cipher
            .decrypt(nonce_obj, ciphertext)
            .map_err(|_| StoreError::Crypto)
    }

    /// Delete a single object.
    ///
    /// # Errors
    ///
    /// Propagates backend errors.
    pub async fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.backend.delete(key).await
    }

    /// Securely delete every object scoped to `job_token` (Req 44.2, 44.3).
    ///
    /// # Errors
    ///
    /// Propagates backend errors.
    pub async fn delete_job(&self, job_token: &str) -> Result<usize, StoreError> {
        // The trailing '/' ensures we match only this Job's namespace.
        self.backend.delete_prefix(&format!("{job_token}/")).await
    }

    /// Directory listing is never permitted (Req 43.5).
    ///
    /// # Errors
    ///
    /// Always returns [`StoreError::ListingForbidden`].
    pub async fn list(&self) -> Result<Vec<String>, StoreError> {
        Err(StoreError::ListingForbidden)
    }
}

/// The low-level byte operations an S3-compatible backend must provide. This is
/// the seam a concrete S3 client (AWS SDK, `rusty-s3`, Cloudflare R2 over the S3
/// API, etc.) implements at the deployment edge; keeping it a narrow trait means
/// the [`EncryptedFileStore`] and every guarantee above are backend-agnostic and
/// the same at-rest ciphertext is produced regardless of backend.
#[async_trait]
pub trait S3Backend: Send + Sync {
    /// PUT an object.
    async fn put_object(&self, bucket: &str, key: &str, body: Vec<u8>) -> Result<(), StoreError>;
    /// GET an object.
    async fn get_object(&self, bucket: &str, key: &str) -> Result<Vec<u8>, StoreError>;
    /// DELETE an object.
    async fn delete_object(&self, bucket: &str, key: &str) -> Result<(), StoreError>;
    /// DELETE every object under a prefix; returns the count deleted.
    async fn delete_objects_with_prefix(
        &self,
        bucket: &str,
        prefix: &str,
    ) -> Result<usize, StoreError>;
}

/// An [`ObjectStore`] backed by an S3-compatible service (Req 44.1 backing
/// store; design "Temporary File_Store").
///
/// This holds only the bucket name and the injected [`S3Backend`]. Directory
/// listing is inherited-refused from the [`ObjectStore`] default (Req 43.5), and
/// the S3 bucket itself is configured with server-side encryption + a
/// 60-minute lifecycle rule matching `Retention_Period` (design), complementing
/// the application-level [`EncryptedFileStore`] envelope encryption.
pub struct S3ObjectStore<B: S3Backend> {
    backend: B,
    bucket: String,
}

impl<B: S3Backend> S3ObjectStore<B> {
    /// Build an S3-backed object store for `bucket`.
    #[must_use]
    pub fn new(backend: B, bucket: String) -> Self {
        Self { backend, bucket }
    }
}

#[async_trait]
impl<B: S3Backend> ObjectStore for S3ObjectStore<B> {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), StoreError> {
        self.backend.put_object(&self.bucket, key, bytes).await
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, StoreError> {
        self.backend.get_object(&self.bucket, key).await
    }
    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.backend.delete_object(&self.bucket, key).await
    }
    async fn delete_prefix(&self, prefix: &str) -> Result<usize, StoreError> {
        self.backend
            .delete_objects_with_prefix(&self.bucket, prefix)
            .await
    }
    // `list` inherits the default that refuses directory listing (Req 43.5).
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn store() -> EncryptedFileStore<InMemoryObjectStore> {
        EncryptedFileStore::new(InMemoryObjectStore::new(), &[7u8; 32])
    }

    #[tokio::test]
    async fn round_trips_encrypted() {
        let s = store();
        let key = scoped_key("tokenA", "file1");
        s.put_encrypted(&key, &[1u8; 12], b"secret bytes").await.unwrap();
        let out = s.get_decrypted(&key).await.unwrap();
        assert_eq!(out, b"secret bytes");
    }

    #[tokio::test]
    async fn at_rest_bytes_are_not_plaintext() {
        let backend = InMemoryObjectStore::new();
        // Peek at the raw stored blob after encryption.
        let s = EncryptedFileStore::new(backend, &[9u8; 32]);
        let key = scoped_key("tok", "f");
        s.put_encrypted(&key, &[2u8; 12], b"PLAINTEXT").await.unwrap();
        let raw = s.backend.get(&key).await.unwrap();
        assert!(!raw.windows(9).any(|w| w == b"PLAINTEXT"));
        // First 12 bytes are the nonce we supplied.
        assert_eq!(&raw[..12], &[2u8; 12]);
    }

    #[tokio::test]
    async fn tampering_is_detected() {
        let s = store();
        let key = scoped_key("t", "f");
        s.put_encrypted(&key, &[3u8; 12], b"data").await.unwrap();
        // Corrupt one ciphertext byte.
        {
            let mut map = s.backend.map.lock().unwrap();
            if let Some(blob) = map.get_mut(&key) {
                let last = blob.len() - 1;
                blob[last] ^= 0xFF;
            }
        }
        assert_eq!(s.get_decrypted(&key).await, Err(StoreError::Crypto));
    }

    #[tokio::test]
    async fn delete_job_removes_only_that_job() {
        let s = store();
        s.put_encrypted(&scoped_key("A", "1"), &[0u8; 12], b"a1").await.unwrap();
        s.put_encrypted(&scoped_key("A", "2"), &[1u8; 12], b"a2").await.unwrap();
        s.put_encrypted(&scoped_key("B", "1"), &[2u8; 12], b"b1").await.unwrap();
        let n = s.delete_job("A").await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(s.get_decrypted(&scoped_key("A", "1")).await, Err(StoreError::NotFound));
        assert_eq!(s.get_decrypted(&scoped_key("B", "1")).await.unwrap(), b"b1");
    }

    #[tokio::test]
    async fn listing_is_forbidden() {
        let s = store();
        assert_eq!(s.list().await, Err(StoreError::ListingForbidden));
    }

    #[tokio::test]
    async fn storage_exhaustion_surfaces() {
        let s = EncryptedFileStore::new(InMemoryObjectStore::with_capacity(16), &[0u8; 32]);
        // Ciphertext of a 100-byte payload plus nonce exceeds the 16-byte cap.
        let err = s
            .put_encrypted(&scoped_key("t", "f"), &[0u8; 12], &[0u8; 100])
            .await;
        assert_eq!(err, Err(StoreError::StorageExhausted));
    }

    #[test]
    fn download_headers_force_attachment_and_safe_type() {
        let h = DownloadHeaders::for_download("report.pdf", "application/pdf");
        assert_eq!(h.content_disposition, "attachment; filename=\"report.pdf\"");
        assert_eq!(h.content_type, "application/pdf");

        // An HTML content type is coerced so the browser cannot execute it.
        let h2 = DownloadHeaders::for_download("x.html", "text/html");
        assert_eq!(h2.content_type, "application/octet-stream");
    }
}
