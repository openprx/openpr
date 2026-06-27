use std::{
    env,
    path::{Component, Path, PathBuf},
};

use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::{Client, Method, StatusCode, Url, header::HeaderMap};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::error::ApiError;

const DEFAULT_OBJECT_STORAGE_DIR: &str = "./uploads";
const DEFAULT_S3_REGION: &str = "us-east-1";

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
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    client: Client,
}

impl ObjectStorage {
    pub fn from_env() -> Result<Self, ApiError> {
        let backend = env::var("OPENPR_OBJECT_STORAGE_BACKEND")
            .or_else(|_| env::var("OBJECT_STORAGE_BACKEND"))
            .unwrap_or_else(|_| "local".to_string());
        match normalize_backend(&backend)? {
            ObjectStorageBackendKind::Local => Self::local(
                env::var("OPENPR_OBJECT_STORAGE_DIR")
                    .or_else(|_| env::var("UPLOAD_DIR"))
                    .unwrap_or_else(|_| DEFAULT_OBJECT_STORAGE_DIR.to_string()),
                "local",
            ),
            ObjectStorageBackendKind::S3 => Self::s3_from_env(),
        }
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

    fn s3_from_env() -> Result<Self, ApiError> {
        let endpoint = required_env_any(&[
            "OPENPR_OBJECT_STORAGE_S3_ENDPOINT",
            "OBJECT_STORAGE_S3_ENDPOINT",
            "AWS_ENDPOINT_URL_S3",
            "AWS_ENDPOINT_URL",
        ])?;
        let bucket = required_env_any(&[
            "OPENPR_OBJECT_STORAGE_S3_BUCKET",
            "OBJECT_STORAGE_S3_BUCKET",
            "AWS_S3_BUCKET",
        ])?;
        validate_s3_bucket(&bucket)?;
        let region = env_any(&[
            "OPENPR_OBJECT_STORAGE_S3_REGION",
            "OBJECT_STORAGE_S3_REGION",
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
        ])
        .unwrap_or_else(|| DEFAULT_S3_REGION.to_string());
        let access_key_id = required_env_any(&[
            "OPENPR_OBJECT_STORAGE_S3_ACCESS_KEY_ID",
            "OBJECT_STORAGE_S3_ACCESS_KEY_ID",
            "AWS_ACCESS_KEY_ID",
        ])?;
        let secret_access_key = required_env_any(&[
            "OPENPR_OBJECT_STORAGE_S3_SECRET_ACCESS_KEY",
            "OBJECT_STORAGE_S3_SECRET_ACCESS_KEY",
            "AWS_SECRET_ACCESS_KEY",
        ])?;
        let session_token = env_any(&[
            "OPENPR_OBJECT_STORAGE_S3_SESSION_TOKEN",
            "OBJECT_STORAGE_S3_SESSION_TOKEN",
            "AWS_SESSION_TOKEN",
        ]);

        Ok(Self {
            backend: ObjectStorageBackend::S3(S3ObjectStorage {
                endpoint: endpoint.trim().trim_end_matches('/').to_string(),
                bucket,
                region,
                access_key_id,
                secret_access_key,
                session_token,
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
            fs::create_dir_all(parent).await.map_err(|_| ApiError::Internal)?;
        }
        fs::write(path, data).await.map_err(|_| ApiError::Internal)
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, ApiError> {
        let path = self.object_path(key)?;
        fs::read(path)
            .await
            .map_err(|_| ApiError::NotFound("object not found".to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), ApiError> {
        let path = self.object_path(key)?;
        match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ApiError::Internal),
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
            access_key_id: &self.access_key_id,
            secret_access_key: &self.secret_access_key,
            region: &self.region,
            amz_date: &amz_date,
            date: &date,
            session_token: self.session_token.as_deref(),
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
                session_token.parse().map_err(|_| ApiError::Internal)?,
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
            "OPENPR_OBJECT_STORAGE_BACKEND supports local/filesystem or s3-compatible".to_string(),
        ))
    }
}

fn env_any(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn required_env_any(keys: &[&str]) -> Result<String, ApiError> {
    env_any(keys).ok_or_else(|| ApiError::BadRequest(format!("{} is required", keys[0])))
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
    #[ignore = "requires a reachable S3-compatible service such as MinIO and OPENPR_OBJECT_STORAGE_S3_* env vars"]
    async fn s3_backend_round_trips_against_minio_when_configured() {
        let storage = ObjectStorage::from_env().expect("S3 object storage env should be configured");
        assert_eq!(storage.reference("acceptance/probe.txt").backend, "s3");
        if env::var("OPENPR_OBJECT_STORAGE_S3_CREATE_BUCKET_FOR_TEST")
            .ok()
            .as_deref()
            == Some("1")
        {
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
