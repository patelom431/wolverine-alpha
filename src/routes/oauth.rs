use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
};
use oauth2::{
    AuthorizationCode, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, Scope, TokenResponse,
    reqwest::async_http_client,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::prelude::*;

#[derive(Deserialize)]
pub struct Callback {
    code: String,
    state: String,
}

pub async fn google_login(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let csrf_token = CsrfToken::new_random();
    
    let session_id = generate_session_id();
    let result = sqlx::query!(
        "INSERT INTO oauth_states (session_id, pkce_challenge, csrf_token) VALUES ($1, $2, $3)",
        session_id,
        pkce_verifier.secret().to_string(),
        csrf_token.secret().to_string())
        .execute(&*state.pool)
        .await;

    if let Err(_) = result {
        return JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR).into_response();
    }

    let (auth_url, _csrf_token) = state.oauth
        .authorize_url(|| csrf_token)
        .add_scope(Scope::new("https://www.googleapis.com/auth/userinfo.email".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    (
        StatusCode::FOUND,
        [
            (header::LOCATION, auth_url.to_string()),
            (header::SET_COOKIE, format!("oauth_session={}; Path=/; HttpOnly; SameSite=Lax", session_id)),
        ],
    ).into_response()
}

pub async fn google_callback(Query(params): Query<Callback>, State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let cookie_header = match headers.get(header::COOKIE) {
        Some(val) => val.to_str().unwrap_or(""),
        None => return JsonResponse::error("No cookie header", StatusCode::BAD_REQUEST).into_response(),
    };

    let session_id = cookie_header
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with("oauth_session="))
        .and_then(|s| s.strip_prefix("oauth_session="))
        .unwrap_or("");

    if session_id.is_empty() {
        return JsonResponse::error("No OAuth session found", StatusCode::BAD_REQUEST).into_response();
    }

    let oauth_state = sqlx::query!(
        "SELECT pkce_challenge, csrf_token FROM oauth_states WHERE session_id = $1",
        session_id)
        .fetch_optional(&*state.pool)
        .await;

    let oauth_state = match oauth_state {
        Ok(Some(state)) => state,
        Ok(None) => return JsonResponse::error("Invalid OAuth session", StatusCode::BAD_REQUEST).into_response(),
        Err(_) => {
            return JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR).into_response();
        }
    };

    if params.state != oauth_state.csrf_token {
        return JsonResponse::error("Invalid CSRF token", StatusCode::BAD_REQUEST).into_response();
    }

    let token = match state.oauth
        .exchange_code(AuthorizationCode::new(params.code))
        .set_pkce_verifier(PkceCodeVerifier::new(oauth_state.pkce_challenge))
        .request_async(async_http_client)
        .await {
            Ok(token) => token,
            Err(e) => {
                eprintln!("Failed to exchange code: {}", e);
                return JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR).into_response();
            }
        };

    let client = reqwest::Client::new();
    let user_info = match client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(token.access_token().secret())
        .send()
        .await {
            Ok(response) => response.json::<serde_json::Value>().await,
            Err(e) => {
                eprintln!("Failed to get user info: {}", e);
                return JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR).into_response();
            }
        };

    let user_info = match user_info {
        Ok(info) => info,
        Err(e) => {
            eprintln!("Failed to parse user info: {}", e);
            return JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR).into_response();
        }
    };

    let email = match user_info.get("email").and_then(|v| v.as_str()) {
        Some(email) => email,
        None => return JsonResponse::error("No email in user info", StatusCode::BAD_REQUEST).into_response(),
    };

    let google_id = match user_info.get("id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return JsonResponse::error("No ID in user info", StatusCode::BAD_REQUEST).into_response(),
    };

    let result = sqlx::query!(
        "INSERT INTO accounts (account_id, api_key, google_id, email)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (email) DO UPDATE
        SET google_id = $4
        RETURNING account_id",
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        google_id,
        email)
        .fetch_one(&*state.pool)
        .await;

    let account_id = match result {
        Ok(record) => record.account_id,
        Err(_) => {
            return JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR).into_response();
        }
    };

    let session_id = generate_session_id();
    let result = sqlx::query!(
        "UPDATE accounts SET session_id = $1 WHERE account_id = $2",
        session_id,
        account_id)
        .execute(&*state.pool)
        .await;

    if let Err(_) = result {
        return JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR).into_response();
    }

    let _ = sqlx::query!(
        "DELETE FROM oauth_states WHERE session_id = $1",
        session_id)
        .execute(&*state.pool)
        .await;

    (
        StatusCode::FOUND,
        [
            (header::LOCATION, "/".to_string()),
            (header::SET_COOKIE, format!("session={}; Path=/; HttpOnly; SameSite=Lax", session_id)),
        ],
    ).into_response()
}

pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let cookie_header = match headers.get(header::COOKIE) {
        Some(val) => val.to_str().unwrap_or(""),
        None => return (
            StatusCode::OK,
            [
                (header::LOCATION, "/".to_string()),
                (header::SET_COOKIE, "session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".to_string()),
            ],
        ).into_response(),
    };

    let session_id = cookie_header
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with("session="))
        .and_then(|s| s.strip_prefix("session="))
        .unwrap_or("");

    if !session_id.is_empty() {
        let _ = sqlx::query!(
            "UPDATE accounts SET session_id = NULL WHERE session_id = $1",
            session_id)
            .execute(&*state.pool)
            .await;
    }

    (
        StatusCode::FOUND,
        [
            (header::LOCATION, "/".to_string()),
            (header::SET_COOKIE, "session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".to_string()),
        ],
    ).into_response()
}

pub async fn get_session(State(state): State<Arc<AppState>>, auth: Auth) -> impl IntoResponse {
    let result = sqlx::query!(
        "SELECT email FROM accounts WHERE account_id = $1",
        auth.account_id)
        .fetch_one(&*state.pool)
        .await;

    match result {
        Ok(user) => JsonResponse::success(json!({"authenticated": true, "email": user.email}), StatusCode::OK),
        Err(e) => {
            eprintln!("Database error fetching user email: {}", e);
            JsonResponse::error("Server error", StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}