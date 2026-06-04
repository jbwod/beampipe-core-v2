use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub typ: TokenType,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenType {
    Access,
    Refresh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("password hash failed: {0}")]
    Hash(#[from] bcrypt::BcryptError),
    #[error("jwt failed: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
}

pub fn hash_password(password: &str) -> Result<String, AuthError> {
    Ok(bcrypt::hash(password, bcrypt::DEFAULT_COST)?)
}

pub fn verify_password(password: &str, hashed: &str) -> bool {
    bcrypt::verify(password, hashed).unwrap_or(false)
}

pub fn issue_token_pair(
    subject: &str,
    secret: &str,
    access_minutes: i64,
    refresh_days: i64,
) -> Result<TokenPair, AuthError> {
    Ok(TokenPair {
        access_token: issue_token(
            subject,
            secret,
            TokenType::Access,
            Duration::minutes(access_minutes),
        )?,
        refresh_token: issue_token(
            subject,
            secret,
            TokenType::Refresh,
            Duration::days(refresh_days),
        )?,
        token_type: "bearer".into(),
    })
}

pub fn issue_token(
    subject: &str,
    secret: &str,
    typ: TokenType,
    ttl: Duration,
) -> Result<String, AuthError> {
    let now = Utc::now();
    let claims = Claims {
        sub: subject.to_string(),
        typ,
        iat: now.timestamp() as usize,
        exp: (now + ttl).timestamp() as usize,
    };
    Ok(encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?)
}

pub fn decode_token(token: &str, secret: &str) -> Result<Claims, AuthError> {
    Ok(decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?
    .claims)
}

pub fn decode_access_token(token: &str, secret: &str) -> Result<Claims, AuthError> {
    let claims = decode_token(token, secret)?;
    if !matches!(claims.typ, TokenType::Access) {
        return Err(AuthError::Jwt(jsonwebtoken::errors::Error::from(
            jsonwebtoken::errors::ErrorKind::InvalidToken,
        )));
    }
    Ok(claims)
}

pub fn decode_refresh_token(token: &str, secret: &str) -> Result<Claims, AuthError> {
    let claims = decode_token(token, secret)?;
    if !matches!(claims.typ, TokenType::Refresh) {
        return Err(AuthError::Jwt(jsonwebtoken::errors::Error::from(
            jsonwebtoken::errors::ErrorKind::InvalidToken,
        )));
    }
    Ok(claims)
}

pub fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
