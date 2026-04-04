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

    fn issue_token(&self, sub: &str, email: &str, token_type: TokenType, ttl_seconds: i64) -> AuthResult<String> {
        let now = Utc::now().timestamp();
        let iat = usize::try_from(now).unwrap_or(0);
        let exp = usize::try_from(now + ttl_seconds).unwrap_or(0);
        let claims = JwtClaims {
            sub: sub.to_string(),
            email: email.to_string(),
            token_type,
            iat,
            exp,
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::*;

    /// 创建测试用 JwtManager，TTL 均为 3600 秒
    fn make_manager() -> JwtManager {
        JwtManager::new("test-secret-key", 3600, 86400)
    }

    // ── 正常流程 ──────────────────────────────────────────────────────────────

    #[test]
    fn test_issue_and_verify_access_token() {
        // 正常签发 access token 后应能通过 verify_access_token 验证
        let mgr = make_manager();
        let token = mgr.issue_access_token("user-1", "user@example.com").expect("签发失败");
        let claims = mgr.verify_access_token(&token).expect("验证失败");
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.email, "user@example.com");
        assert_eq!(claims.token_type, TokenType::Access);
    }

    #[test]
    fn test_issue_and_verify_refresh_token() {
        // 正常签发 refresh token 后应能通过 verify_refresh_token 验证
        let mgr = make_manager();
        let token = mgr.issue_refresh_token("user-2", "user2@example.com").expect("签发失败");
        let claims = mgr.verify_refresh_token(&token).expect("验证失败");
        assert_eq!(claims.sub, "user-2");
        assert_eq!(claims.email, "user2@example.com");
        assert_eq!(claims.token_type, TokenType::Refresh);
    }

    #[test]
    fn test_access_token_claims_fields() {
        // 验证 access token 的 sub/email/token_type/iat/exp 字段均正确写入
        let mgr = make_manager();
        let before = chrono::Utc::now().timestamp() as usize;
        let token = mgr.issue_access_token("uid-42", "check@fields.com").expect("签发失败");
        let after = chrono::Utc::now().timestamp() as usize;

        let claims = mgr.verify_access_token(&token).expect("验证失败");

        assert_eq!(claims.sub, "uid-42");
        assert_eq!(claims.email, "check@fields.com");
        assert_eq!(claims.token_type, TokenType::Access);
        // iat 应在调用前后时间戳之间（含边界）
        assert!(claims.iat >= before, "iat 不应早于签发前");
        assert!(claims.iat <= after, "iat 不应晚于签发后");
        // exp 应比 iat 大约多 3600 秒（允许 ±2 秒误差）
        let delta = claims.exp.saturating_sub(claims.iat);
        assert!(
            (3598..=3602).contains(&delta),
            "exp-iat 偏差过大: {delta}"
        );
    }

    // ── Token 类型混用拒绝 ────────────────────────────────────────────────────

    #[test]
    fn test_verify_access_as_refresh_fails() {
        // access token 不能被 verify_refresh_token 接受，应返回 InvalidTokenType
        let mgr = make_manager();
        let token = mgr.issue_access_token("u", "u@x.com").expect("签发失败");
        let result = mgr.verify_refresh_token(&token);
        assert!(
            matches!(result, Err(AuthError::InvalidTokenType)),
            "预期 InvalidTokenType，实际: {result:?}"
        );
    }

    #[test]
    fn test_verify_refresh_as_access_fails() {
        // refresh token 不能被 verify_access_token 接受，应返回 InvalidTokenType
        let mgr = make_manager();
        let token = mgr.issue_refresh_token("u", "u@x.com").expect("签发失败");
        let result = mgr.verify_access_token(&token);
        assert!(
            matches!(result, Err(AuthError::InvalidTokenType)),
            "预期 InvalidTokenType，实际: {result:?}"
        );
    }

    // ── 签名完整性 ────────────────────────────────────────────────────────────

    #[test]
    fn test_verify_tampered_token() {
        // 修改 token 签名部分后验证应失败
        let mgr = make_manager();
        let token = mgr.issue_access_token("u", "u@x.com").expect("签发失败");

        // JWT 由三段 base64 组成，篡改最后一段（签名）
        let mut parts: Vec<&str> = token.splitn(3, '.').collect();
        assert_eq!(parts.len(), 3, "JWT 格式异常");
        let tampered_sig = "dGFtcGVyZWRzaWduYXR1cmU"; // base64("tamperedsignature")
        if let Some(sig_slot) = parts.get_mut(2) {
            *sig_slot = tampered_sig;
        }
        let tampered = parts.join(".");

        let result = mgr.verify_access_token(&tampered);
        assert!(result.is_err(), "篡改签名后应验证失败");
    }

    #[test]
    fn test_verify_garbage_string() {
        // 传入随机垃圾字符串时验证应返回错误
        let mgr = make_manager();
        let result = mgr.verify_access_token("not.a.jwt.at.all");
        assert!(result.is_err(), "垃圾字符串应验证失败");
    }

    #[test]
    fn test_verify_empty_string() {
        // 传入空字符串时验证应返回错误
        let mgr = make_manager();
        let result = mgr.verify_access_token("");
        assert!(result.is_err(), "空字符串应验证失败");
    }

    // ── Secret 隔离 ───────────────────────────────────────────────────────────

    #[test]
    fn test_different_secret_rejects() {
        // 用 secret-A 签发的 token，不能被 secret-B 的 manager 验证
        let mgr_a = JwtManager::new("secret-A", 3600, 86400);
        let mgr_b = JwtManager::new("secret-B", 3600, 86400);

        let token = mgr_a.issue_access_token("u", "u@x.com").expect("签发失败");
        let result = mgr_b.verify_access_token(&token);
        assert!(result.is_err(), "不同 secret 应验证失败");
    }

    // ── TokenType 序列化 ──────────────────────────────────────────────────────

    #[test]
    fn test_token_type_serialization() {
        // TokenType 序列化应为小写字符串 "access" / "refresh"（serde rename_all = "lowercase"）
        let access_json = serde_json::to_string(&TokenType::Access).expect("序列化失败");
        let refresh_json = serde_json::to_string(&TokenType::Refresh).expect("序列化失败");

        assert_eq!(access_json, "\"access\"");
        assert_eq!(refresh_json, "\"refresh\"");

        // 反序列化也应正确
        let access: TokenType = serde_json::from_str("\"access\"").expect("反序列化失败");
        let refresh: TokenType = serde_json::from_str("\"refresh\"").expect("反序列化失败");
        assert_eq!(access, TokenType::Access);
        assert_eq!(refresh, TokenType::Refresh);
    }

    // ── 边界 TTL ──────────────────────────────────────────────────────────────

    #[test]
    fn test_zero_ttl_behavior() {
        // TTL=0 时签发的 token exp == iat，jsonwebtoken 的 leeway 默认为 60s，
        // 所以在签发后立即验证时可能通过也可能因已过期失败；
        // 此处只断言：签发不会 panic，且 verify 返回 Result（不 panic）
        let mgr = JwtManager::new("sec", 0, 0);
        let result_access = mgr.issue_access_token("u", "u@x.com");
        assert!(result_access.is_ok(), "TTL=0 签发不应 panic/错误");

        let token = result_access.expect("已断言 ok");
        // verify 结果允许 Ok 或 Err，但必须不 panic
        let _verify_result = mgr.verify_access_token(&token);
    }

    #[test]
    fn test_large_ttl_token() {
        // 非常大的 TTL（100 年）下签发和验证均应正常工作
        let hundred_years_secs: i64 = 100 * 365 * 24 * 3600;
        let mgr = JwtManager::new("sec", hundred_years_secs, hundred_years_secs);
        let token = mgr.issue_access_token("u", "u@x.com").expect("签发失败");
        let claims = mgr.verify_access_token(&token).expect("验证失败");
        // exp 应远大于 iat
        assert!(
            claims.exp > claims.iat,
            "大 TTL 下 exp 应大于 iat"
        );
    }

    // ── Unicode / 特殊字符 round-trip ─────────────────────────────────────────

    #[test]
    fn test_unicode_sub_email() {
        // Unicode 字符（中文、Emoji）在 sub/email 中能正确 round-trip
        let mgr = make_manager();
        let sub = "用户-😀-αβγ";
        let email = "测试@例子.中文";
        let token = mgr.issue_access_token(sub, email).expect("签发失败");
        let claims = mgr.verify_access_token(&token).expect("验证失败");
        assert_eq!(claims.sub, sub, "Unicode sub round-trip 失败");
        assert_eq!(claims.email, email, "Unicode email round-trip 失败");
    }

    #[test]
    fn test_special_chars_in_email() {
        // 邮箱中含有合法特殊字符（+、.、-、_）时能正确 round-trip
        let mgr = make_manager();
        let email = "user+tag.name-ext_123@sub.domain.co.uk";
        let token = mgr.issue_access_token("uid-special", email).expect("签发失败");
        let claims = mgr.verify_access_token(&token).expect("验证失败");
        assert_eq!(claims.email, email, "特殊字符邮箱 round-trip 失败");
    }
}
