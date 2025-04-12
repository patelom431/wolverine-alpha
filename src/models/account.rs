use chrono::{NaiveDateTime};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Serialize, Deserialize, FromRow)]
pub struct Account {
    pub account_id: String,
    pub api_key: String,
    pub google_id: String,
    pub session_id: Option<String>,
    pub email: String,
    pub active: bool,
    pub wallet_address: Option<String>,
    pub wallet_secret: Option<String>,
    pub created_at: NaiveDateTime,
}