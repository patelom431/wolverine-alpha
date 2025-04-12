use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
};
use std::ops::Deref;
use sqlx::FromRow;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::{rng, RngCore};
use headers::{Cookie, HeaderMapExt};

use crate::prelude::*;

#[derive(Clone, FromRow)]
pub struct Auth {
    pub account_id: String,
}

impl<S> FromRequestParts<S> for Auth where S: Send + Sync + Deref<Target = AppState> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // First try API key authentication
        if let Some(api_key) = parts.headers.get("X-API-KEY").and_then(|v| v.to_str().ok()) {
            match get_account_api_key(&state.pool, api_key).await {
                Ok(Some(account_id)) => return Ok(Auth { account_id }),
                Ok(None) => return Err((StatusCode::UNAUTHORIZED, JsonResponse::error("Invalid API key", StatusCode::UNAUTHORIZED)).into_response()),
                Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, JsonResponse::error("Database error", StatusCode::INTERNAL_SERVER_ERROR)).into_response()),
            }
        }

        // Then try session authentication
        let cookies = match parts.headers.typed_get::<Cookie>() {
            Some(cookies) => cookies,
            None => return Err((StatusCode::UNAUTHORIZED, JsonResponse::error("Invalid authentication", StatusCode::UNAUTHORIZED)).into_response()),
        };

        let session_value = match cookies.get("session") {
            Some(value) => value,
            None => return Err((StatusCode::UNAUTHORIZED, JsonResponse::error("Invalid authentication", StatusCode::UNAUTHORIZED)).into_response()),
        };

        match get_account_session(&state.pool, session_value).await {
            Ok(Some(account_id)) => Ok(Auth { account_id }),
            Ok(None) => Err((StatusCode::UNAUTHORIZED, JsonResponse::error("Invalid session", StatusCode::UNAUTHORIZED)).into_response()),
            Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR)).into_response()),
        }
    }
}

async fn get_account_api_key(pool: &sqlx::PgPool, api_key: &str) -> Result<Option<String>, sqlx::Error> {
    let result = sqlx::query_as!(
        Auth,
        "SELECT account_id FROM accounts WHERE api_key = $1 AND active = true",
        api_key
    )
    .fetch_optional(pool)
    .await;

    match result {
        Ok(Some(record)) => Ok(Some(record.account_id)),
        Ok(None) => Ok(None),
        Err(e) => Err(e),
    }
}

async fn get_account_session(pool: &sqlx::PgPool, session_id: &str) -> Result<Option<String>, sqlx::Error> {
    let result = sqlx::query_as!(
        Auth,
        "SELECT account_id FROM accounts WHERE session_id = $1 AND active = true",
        session_id
    )
    .fetch_optional(pool)
    .await;

    match result {
        Ok(Some(record)) => Ok(Some(record.account_id)),
        Ok(None) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn generate_session_id() -> String {
    let mut rng = rng();
    let mut bytes = [0u8; 32];
    rng.fill_bytes(&mut bytes);
    BASE64.encode(&bytes)
}