use crate::config::Database;
use crate::models::user::{AccountType, AuthResponse, RegisterRequest, User, UserResponse};
use anyhow::{anyhow, Result};
use bcrypt::{hash, DEFAULT_COST};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub account_type: AccountType,
}

pub struct AuthService {
    db: Database,
    jwt_secret: String,
}

impl AuthService {
    pub fn new(db: Database, jwt_secret: String) -> Self {
        Self { db, jwt_secret }
    }

    pub async fn register(&self, req: RegisterRequest) -> Result<AuthResponse> {
        // Check if user already exists
        let exists = sqlx::query(
            "SELECT id FROM users WHERE email = $1"
        )
        .bind(&req.email)
        .fetch_optional(self.db.sqlx_pool())
        .await?;

        if exists.is_some() {
            return Err(anyhow!("User with this email already exists"));
        }

        // Hash password
        let password_hash = hash(req.password, DEFAULT_COST)?;

        // Insert user
        let user = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (email, password_hash, account_type)
            VALUES ($1, $2, $3)
            RETURNING id, email, password_hash, account_type, is_verified, created_at, updated_at
            "#
        )
        .bind(&req.email)
        .bind(password_hash)
        .bind(req.account_type)
        .fetch_one(self.db.sqlx_pool())
        .await?;

        // Generate JWT
        let token = self.generate_token(&user)?;

        Ok(AuthResponse {
            user: UserResponse::from(user),
            token,
        })
    }

    fn generate_token(&self, user: &User) -> Result<String> {
        let expiration = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() + (24 * 3600); // 24 hours

        let claims = Claims {
            sub: user.id.to_string(),
            exp: expiration as usize,
            account_type: user.account_type,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret.as_bytes()),
        )?;

        Ok(token)
    }
}
