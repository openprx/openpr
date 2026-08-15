use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use hmac::{Hmac, Mac};
use platform::config::{S3Config, Secret, StorageBackend, StorageConfig};
use reqwest::{Client, Method, StatusCode, Url, header::HeaderMap};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::error::ApiError;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ObjectReference {
    pub backend: String,
    pub key: String,
}

#[derive(Clone, Debug)]
pub struct ObjectStorage {
    backend: ObjectStorageBackend,
}

#[derive(Clone, Debug)]
enum ObjectStorageBackend {
    Local(LocalObjectStorage),
    S3(S3ObjectStorage),
}

#[derive(Clone, Debug)]
struct LocalObjectStorage {
    root: PathBuf,
    backend: String,
}

#[derive(Clone, Debug)]
struct S3ObjectStorage {
    endpoint: String,
    bucket: String,
    region: String,
    /// Held as [`Secret`] so the derived `Debug` on this struct, and on every struct that carries
    /// an [`ObjectStorage`], cannot print the signing credentials.
    access_key_id: Secret,
    secret_access_key: Secret,
    session_token: Option<Secret>,
    client: Client,
}

impl ObjectStorage {
    /// Builds the backend the `[storage]` section selects.
    ///
    /// The backend used to be assembled from a dozen environment variables, several of them
    /// generic names (`AWS_*`, `UPLOAD_DIR`) that anything in the surrounding environment could
    /// set. It now comes from the configuration file, which has already normalised the backend
    /// name and validated the S3 section.
    pub fn from_config(storage: &StorageConfig) -> Result<Self, ApiError> {
        match storage.backend {
            StorageBackend::Local => Self::local(storage.dir.clone(), "local"),
            StorageBackend::S3 => {
                let s3 = storage.s3.as_ref().ok_or_else(|| {
                    ApiError::BadRequest("storage.backend is s3 but the [storage.s3] section is missing".to_string())
                })?;
                Self::s3(s3)
            }
        }
    }

    /// Builds the backend from the configuration this process installed at startup.
    pub fn from_runtime_config() -> Result<Self, ApiError> {
        Self::from_config(&crate::config::runtime().storage)
    }

    pub fn local(root: impl Into<PathBuf>, backend: impl Into<String>) -> Result<Self, ApiError> {
        if normalize_backend(&backend.into())? != ObjectStorageBackendKind::Local {
            return Err(ApiError::BadRequest(
                "local object storage constructor requires local/filesystem backend".to_string(),
            ));
        }
        Ok(Self {
            backend: ObjectStorageBackend::Local(LocalObjectStorage {
                root: root.into(),
                backend: "local".to_string(),
            }),
        })
    }

    /// Builds the S3 backend from an already validated `[storage.s3]` section.
    ///
    /// The bucket is re-checked here rather than trusted: it is interpolated into every request
    /// path, so the one rule that keeps a key from escaping its bucket is enforced at the point
    /// of use as well as at load time.
    fn s3(s3: &S3Config) -> Result<Self, ApiError> {
        validate_s3_bucket(&s3.bucket)?;
        Ok(Self {
            backend: ObjectStorageBackend::S3(S3ObjectStorage {
                endpoint: s3.endpoint.trim().trim_end_matches('/').to_string(),
                bucket: s3.bucket.clone(),
                region: s3.region.clone(),
                access_key_id: s3.access_key_id.clone(),
                secret_access_key: s3.secret_access_key.clone(),
                session_token: s3.session_token.clone(),
                client: Client::new(),
            }),
        })
    }

    pub fn reference(&self, key: impl Into<String>) -> ObjectReference {
        let backend = match &self.backend {
            ObjectStorageBackend::Local(local) => local.backend.clone(),
            ObjectStorageBackend::S3(_) => "s3".to_string(),
        };
        ObjectReference {
            backend,
            key: key.into(),
        }
    }

    pub async fn put(&self, key: &str, data: &[u8]) -> Result<ObjectReference, ApiError> {
        validate_object_key(key)?;
        match &self.backend {
            ObjectStorageBackend::Local(local) => local.put(key, data).await?,
            ObjectStorageBackend::S3(s3) => s3.put(key, data).await?,
        }
        Ok(self.reference(key.to_string()))
    }

    pub async fn get(&self, key: &str) -> Result<Vec<u8>, ApiError> {
        validate_object_key(key)?;
        match &self.backend {
            ObjectStorageBackend::Local(local) => local.get(key).await,
            ObjectStorageBackend::S3(s3) => s3.get(key).await,
        }
    }

    pub async fn delete(&self, key: &str) -> Result<(), ApiError> {
        validate_object_key(key)?;
        match &self.backend {
            ObjectStorageBackend::Local(local) => local.delete(key).await,
            ObjectStorageBackend::S3(s3) => s3.delete(key).await,
        }
    }

    #[cfg(test)]
    pub async fn create_bucket_for_test(&self) -> Result<(), ApiError> {
        match &self.backend {
            ObjectStorageBackend::S3(s3) => s3.create_bucket_for_test().await,
            ObjectStorageBackend::Local(_) => Err(ApiError::BadRequest(
                "S3 test bucket creation requires the s3-compatible backend".to_string(),
            )),
        }
    }
}

impl LocalObjectStorage {
    async fn put(&self, key: &str, data: &[u8]) -> Result<(), ApiError> {
        let path = self.object_path(key)?;
        if let Some(parent) = path.parent() {
            // The caller only ever sees `internal server error`, so without this the most
            // common local-backend failure -- a storage directory the container user cannot
            // write, which is what a bind mount owned by another uid produces -- reaches the
            // operator as a 500 with nothing in the log to act on.
            fs::create_dir_all(parent).await.map_err(|error| {
                tracing::error!(%error, path = %parent.display(), "local object directory create failed");
                ApiError::Internal
            })?;
        }
        fs::write(&path, data).await.map_err(|error| {
            tracing::error!(%error, path = %path.display(), "local object put failed");
            ApiError::Internal
        })
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, ApiError> {
        let path = self.object_path(key)?;
        fs::read(path)
            .await
            .map_err(|_| ApiError::NotFound("object not found".to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), ApiError> {
        let path = self.object_path(key)?;
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                tracing::error!(%error, path = %path.display(), "local object delete failed");
                Err(ApiError::Internal)
            }
        }
    }

    fn object_path(&self, key: &str) -> Result<PathBuf, ApiError> {
        validate_object_key(key)?;
        Ok(self.root.join(key))
    }
}

impl S3ObjectStorage {
    async fn put(&self, key: &str, data: &[u8]) -> Result<(), ApiError> {
        let response = self.send(Method::PUT, key, data).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            tracing::error!(status = %response.status(), "s3 object put failed");
            Err(ApiError::Internal)
        }
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, ApiError> {
        let response = self.send(Method::GET, key, &[]).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(ApiError::NotFound("object not found".to_string()));
        }
        if !response.status().is_success() {
            tracing::error!(status = %response.status(), "s3 object get failed");
            return Err(ApiError::Internal);
        }
        response.bytes().await.map(|bytes| bytes.to_vec()).map_err(|error| {
            tracing::error!(%error, "s3 object get body failed");
            ApiError::Internal
        })
    }

    async fn delete(&self, key: &str) -> Result<(), ApiError> {
        let response = self.send(Method::DELETE, key, &[]).await?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            tracing::error!(status = %response.status(), "s3 object delete failed");
            Err(ApiError::Internal)
        }
    }

    async fn send(&self, method: Method, key: &str, body: &[u8]) -> Result<reqwest::Response, ApiError> {
        let canonical_uri = s3_canonical_uri(&self.bucket, key);
        self.send_canonical_uri(method, &canonical_uri, body).await
    }

    async fn send_canonical_uri(
        &self,
        method: Method,
        canonical_uri: &str,
        body: &[u8],
    ) -> Result<reqwest::Response, ApiError> {
        let url = format!("{}{}", self.endpoint, canonical_uri);
        let parsed_url =
            Url::parse(&url).map_err(|_| ApiError::BadRequest("S3 endpoint URL is invalid".to_string()))?;
        let host = s3_host_header(&parsed_url)?;
        let payload_hash = hex_sha256(body);
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let authorization = s3_authorization(S3AuthorizationInput {
            method: method.as_str(),
            canonical_uri: &canonical_uri,
            host: &host,
            payload_hash: &payload_hash,
            access_key_id: self.access_key_id.expose(),
            secret_access_key: self.secret_access_key.expose(),
            region: &self.region,
            amz_date: &amz_date,
            date: &date,
            session_token: self.session_token.as_ref().map(Secret::expose),
        })?;
        let mut headers = HeaderMap::new();
        headers.insert("host", host.parse().map_err(|_| ApiError::Internal)?);
        headers.insert(
            "x-amz-content-sha256",
            payload_hash.parse().map_err(|_| ApiError::Internal)?,
        );
        headers.insert("x-amz-date", amz_date.parse().map_err(|_| ApiError::Internal)?);
        if let Some(session_token) = &self.session_token {
            headers.insert(
                "x-amz-security-token",
                session_token.expose().parse().map_err(|_| ApiError::Internal)?,
            );
        }
        headers.insert("authorization", authorization.parse().map_err(|_| ApiError::Internal)?);

        let request = self.client.request(method, url).headers(headers);
        let request = if body.is_empty() {
            request
        } else {
            request.body(body.to_vec())
        };
        request.send().await.map_err(|error| {
            tracing::error!(%error, "s3 object request failed");
            ApiError::Internal
        })
    }

    #[cfg(test)]
    async fn create_bucket_for_test(&self) -> Result<(), ApiError> {
        let response = self
            .send_canonical_uri(Method::PUT, &s3_bucket_canonical_uri(&self.bucket), &[])
            .await?;
        if response.status().is_success() || response.status() == StatusCode::CONFLICT {
            Ok(())
        } else {
            tracing::error!(status = %response.status(), "s3 test bucket create failed");
            Err(ApiError::Internal)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ObjectStorageBackendKind {
    Local,
    S3,
}

fn normalize_backend(raw: &str) -> Result<ObjectStorageBackendKind, ApiError> {
    let backend = raw.trim().to_ascii_lowercase();
    if matches!(backend.as_str(), "" | "local" | "filesystem" | "fs") {
        Ok(ObjectStorageBackendKind::Local)
    } else if matches!(backend.as_str(), "s3" | "s3-compatible" | "s3_compatible") {
        Ok(ObjectStorageBackendKind::S3)
    } else {
        Err(ApiError::BadRequest(
            "storage.backend supports local/filesystem or s3-compatible".to_string(),
        ))
    }
}

pub fn validate_object_key(key: &str) -> Result<(), ApiError> {
    let path = Path::new(key);
    if key.trim().is_empty() || path.is_absolute() || key.contains('\\') {
        return Err(ApiError::BadRequest("object key is invalid".to_string()));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) | Component::CurDir
        )
    }) {
        return Err(ApiError::BadRequest("object key is invalid".to_string()));
    }
    Ok(())
}

fn validate_s3_bucket(bucket: &str) -> Result<(), ApiError> {
    let bucket = bucket.trim();
    if bucket.is_empty() || bucket.contains('/') || bucket.contains('\\') || bucket.contains("..") {
        return Err(ApiError::BadRequest("S3 bucket is invalid".to_string()));
    }
    Ok(())
}

fn s3_canonical_uri(bucket: &str, key: &str) -> String {
    format!(
        "/{}/{}",
        percent_encode_path_segment(bucket),
        key.split('/')
            .map(percent_encode_path_segment)
            .collect::<Vec<_>>()
            .join("/")
    )
}

#[cfg(test)]
fn s3_bucket_canonical_uri(bucket: &str) -> String {
    format!("/{}", percent_encode_path_segment(bucket))
}

fn percent_encode_path_segment(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![char::from(*byte)]
            }
            _ => format!("%{byte:02X}").chars().collect::<Vec<_>>(),
        })
        .collect()
}

fn s3_host_header(url: &Url) -> Result<String, ApiError> {
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::BadRequest("S3 endpoint host is invalid".to_string()))?;
    Ok(url
        .port()
        .map_or_else(|| host.to_string(), |port| format!("{host}:{port}")))
}

struct S3AuthorizationInput<'a> {
    method: &'a str,
    canonical_uri: &'a str,
    host: &'a str,
    payload_hash: &'a str,
    access_key_id: &'a str,
    secret_access_key: &'a str,
    region: &'a str,
    amz_date: &'a str,
    date: &'a str,
    session_token: Option<&'a str>,
}

fn s3_authorization(input: S3AuthorizationInput<'_>) -> Result<String, ApiError> {
    let credential_scope = format!("{}/{}/s3/aws4_request", input.date, input.region);
    let mut canonical_headers = format!(
        "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
        input.host, input.payload_hash, input.amz_date
    );
    let signed_headers = if let Some(session_token) = input.session_token {
        canonical_headers.push_str(&format!("x-amz-security-token:{session_token}\n"));
        "host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
    } else {
        "host;x-amz-content-sha256;x-amz-date"
    };
    let canonical_request = format!(
        "{}\n{}\n\n{}\n{}\n{}",
        input.method, input.canonical_uri, canonical_headers, signed_headers, input.payload_hash
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        input.amz_date,
        credential_scope,
        hex_sha256(canonical_request.as_bytes())
    );
    let signing_key = s3_signing_key(input.secret_access_key, input.date, input.region)?;
    let signature = hex::encode(hmac_sha256(&signing_key, &string_to_sign)?);
    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        input.access_key_id, credential_scope, signed_headers, signature
    ))
}

fn s3_signing_key(secret_access_key: &str, date: &str, region: &str) -> Result<Vec<u8>, ApiError> {
    let date_key = hmac_sha256(format!("AWS4{secret_access_key}").as_bytes(), date)?;
    let region_key = hmac_sha256(&date_key, region)?;
    let service_key = hmac_sha256(&region_key, "s3")?;
    hmac_sha256(&service_key, "aws4_request")
}

fn hmac_sha256(key: &[u8], data: &str) -> Result<Vec<u8>, ApiError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| ApiError::Internal)?;
    mac.update(data.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Environment variable naming the configuration file the S3 acceptance tests read.
///
/// Test scaffolding in the same spirit as `OPENPR_TEST_DATABASE_URL`, and compiled only into the
/// test binary: those tests need a reachable S3 compatible service, which no committed
/// configuration file can describe. The service itself reads nothing from the environment.
#[cfg(test)]
pub const TEST_CONFIG_PATH_ENV: &str = "OPENPR_TEST_CONFIG";

/// Set to `1` to have the S3 acceptance tests create their bucket before using it.
#[cfg(test)]
pub const TEST_S3_CREATE_BUCKET_ENV: &str = "OPENPR_TEST_S3_CREATE_BUCKET";

/// The object storage described by the configuration file [`TEST_CONFIG_PATH_ENV`] names.
#[cfg(test)]
pub fn test_object_storage() -> Result<ObjectStorage, ApiError> {
    let path = std::env::var(TEST_CONFIG_PATH_ENV).map_err(|_| {
        ApiError::BadRequest(format!(
            "{TEST_CONFIG_PATH_ENV} must name a configuration file whose [storage] section selects the s3 backend"
        ))
    })?;
    let config = platform::config::OpenPrConfig::load(Some(Path::new(&path)))
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    ObjectStorage::from_config(&config.storage)
}

/// Whether the S3 acceptance tests should create their bucket first.
#[cfg(test)]
pub fn test_should_create_bucket() -> bool {
    std::env::var(TEST_S3_CREATE_BUCKET_ENV).is_ok_and(|value| value.trim() == "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_object_keys() {
        assert!(validate_object_key("file.csv").is_ok());
        assert!(validate_object_key("packages/file.zip").is_ok());
        assert!(validate_object_key("").is_err());
        assert!(validate_object_key("../file.zip").is_err());
        assert!(validate_object_key("/file.zip").is_err());
        assert!(validate_object_key("nested\\file.zip").is_err());
        assert!(validate_object_key("./file.zip").is_err());
    }

    #[test]
    fn rejects_unknown_backends() {
        assert!(ObjectStorage::local("/tmp/openpr-test", "local").is_ok());
        assert!(ObjectStorage::local("/tmp/openpr-test", "filesystem").is_ok());
        assert!(ObjectStorage::local("/tmp/openpr-test", "s3").is_err());
        assert!(matches!(
            normalize_backend("s3-compatible").unwrap(),
            ObjectStorageBackendKind::S3
        ));
    }

    fn s3_config() -> S3Config {
        S3Config {
            endpoint: "https://s3.example.test/".to_string(),
            bucket: "openpr-uploads".to_string(),
            region: "eu-central-1".to_string(),
            access_key_id: Secret::new("AKIDEXAMPLE"),
            secret_access_key: Secret::new("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"),
            session_token: Some(Secret::new("session-token-value")),
        }
    }

    fn storage_config(backend: StorageBackend, s3: Option<S3Config>) -> StorageConfig {
        StorageConfig {
            backend,
            dir: PathBuf::from("/tmp/openpr-test"),
            s3,
        }
    }

    #[test]
    fn builds_the_backend_the_storage_section_names() {
        let local = ObjectStorage::from_config(&storage_config(StorageBackend::Local, None))
            .expect("local backend should build");
        assert_eq!(local.reference("file.csv").backend, "local");

        let s3 = ObjectStorage::from_config(&storage_config(StorageBackend::S3, Some(s3_config())))
            .expect("s3 backend should build");
        assert_eq!(s3.reference("file.csv").backend, "s3");
    }

    #[test]
    fn refuses_the_s3_backend_without_its_section() {
        // Falling back to local storage here would silently write uploads to the container's
        // filesystem on a deployment that asked for S3.
        assert!(ObjectStorage::from_config(&storage_config(StorageBackend::S3, None)).is_err());
    }

    #[test]
    fn debug_output_never_contains_the_s3_credentials() {
        let storage = ObjectStorage::from_config(&storage_config(StorageBackend::S3, Some(s3_config())))
            .expect("s3 backend should build");
        let rendered = format!("{storage:?}");
        assert!(!rendered.contains("wJalrXUtnFEMI"), "{rendered}");
        assert!(!rendered.contains("session-token-value"), "{rendered}");
    }

    #[test]
    fn builds_path_style_s3_canonical_uri() {
        assert_eq!(
            s3_canonical_uri("openpr-forms", "packages/export 1.zip"),
            "/openpr-forms/packages/export%201.zip"
        );
        assert_eq!(
            s3_canonical_uri("bucket", "unicode/附件.csv"),
            "/bucket/unicode/%E9%99%84%E4%BB%B6.csv"
        );
    }

    #[test]
    fn builds_s3_authorization_header() {
        let payload_hash = hex_sha256(b"hello");
        let authorization = s3_authorization(S3AuthorizationInput {
            method: "PUT",
            canonical_uri: "/openpr-forms/packages/test.txt",
            host: "s3.example.test",
            payload_hash: &payload_hash,
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            region: "us-east-1",
            amz_date: "20260603T010203Z",
            date: "20260603",
            session_token: None,
        })
        .expect("authorization should build");
        assert!(
            authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260603/us-east-1/s3/aws4_request")
        );
        assert!(authorization.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        assert_eq!(authorization.rsplit_once('=').unwrap().1.len(), 64);
    }

    #[tokio::test]
    #[ignore = "requires a reachable S3-compatible service such as MinIO and OPENPR_TEST_CONFIG pointing at an s3 configuration file"]
    async fn s3_backend_round_trips_against_minio_when_configured() {
        let storage = test_object_storage().expect("OPENPR_TEST_CONFIG should describe the s3 backend");
        assert_eq!(storage.reference("acceptance/probe.txt").backend, "s3");
        if test_should_create_bucket() {
            storage
                .create_bucket_for_test()
                .await
                .expect("bucket create should succeed");
        }

        let key = format!(
            "acceptance/minio-roundtrip-{}.txt",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let body = b"openpr minio acceptance";

        let reference = storage.put(&key, body).await.expect("put should succeed");
        assert_eq!(reference.backend, "s3");
        assert_eq!(reference.key, key);

        let stored = storage.get(&key).await.expect("get should succeed");
        assert_eq!(stored, body);

        storage.delete(&key).await.expect("delete should succeed");
        assert!(storage.get(&key).await.is_err());
    }
}
