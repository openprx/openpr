use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TokenType {
    Access,
    Refresh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,
    pub email: String,
    pub token_type: TokenType,
    pub iat: usize,
    pub exp: usize,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("token error: {0}")]
    Token(#[from] jsonwebtoken::errors::Error),
    #[error("invalid token type")]
    InvalidTokenType,
}

pub type AuthResult<T> = Result<T, AuthError>;

#[derive(Clone)]
pub struct JwtManager {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    access_ttl_seconds: i64,
    refresh_ttl_seconds: i64,
}

impl JwtManager {
    pub fn new(secret: &str, access_ttl_seconds: i64, refresh_ttl_seconds: i64) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            access_ttl_seconds,
            refresh_ttl_seconds,
        }
    }

    pub fn issue_access_token(&self, sub: &str, email: &str) -> AuthResult<String> {
        self.issue_token(sub, email, TokenType::Access, self.access_ttl_seconds)
    }

    pub fn issue_refresh_token(&self, sub: &str, email: &str) -> AuthResult<String> {
        self.issue_token(sub, email, TokenType::Refresh, self.refresh_ttl_seconds)
    }

    pub fn verify_access_token(&self, token: &str) -> AuthResult<JwtClaims> {
        self.verify_token(token, TokenType::Access)
    }

    pub fn verify_refresh_token(&self, token: &str) -> AuthResult<JwtClaims> {
        self.verify_token(token, TokenType::Refresh)
    }

    fn issue_token(
        &self,
        sub: &str,
        email: &str,
        token_type: TokenType,
        ttl_seconds: i64,
    ) -> AuthResult<String> {
        let now = Utc::now().timestamp();
        let claims = JwtClaims {
            sub: sub.to_string(),
            email: email.to_string(),
            token_type,
            iat: now as usize,
            exp: (now + ttl_seconds) as usize,
        };

        Ok(encode(&Header::default(), &claims, &self.encoding_key)?)
    }

    fn verify_token(&self, token: &str, expected_type: TokenType) -> AuthResult<JwtClaims> {
        let validation = Validation::new(Algorithm::HS256);
        let data = decode::<JwtClaims>(token, &self.decoding_key, &validation)?;

        if data.claims.token_type != expected_type {
            return Err(AuthError::InvalidTokenType);
        }

        Ok(data.claims)
    }
}
