use chrono::{NaiveDateTime};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Serialize, Deserialize, FromRow)]
pub struct Market {
    pub condition_id: String,
    pub question: String,
    pub description: String,
    pub tags: String,
    pub yes_token_id: Option<String>,
    pub no_token_id: Option<String>,
    pub end_date: NaiveDateTime,
    pub created_at: NaiveDateTime,
}