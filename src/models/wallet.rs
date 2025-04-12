use serde::{Serialize, Deserialize};
use sqlx::FromRow;
use chrono::{NaiveDateTime};

#[derive(Serialize, Deserialize, FromRow)]
pub struct Wallet {
    pub account_id: String,
    pub subscription_id: String,
    pub address: String,
    pub secret: String,
    pub created_at: NaiveDateTime,
}