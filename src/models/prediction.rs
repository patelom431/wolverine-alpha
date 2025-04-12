use chrono::{NaiveDateTime};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Serialize, Deserialize, FromRow)]
pub struct Prediction {
    pub prediction_id: String,
    pub condition_id: String,
    pub end_date: NaiveDateTime,
}

#[derive(Serialize, Deserialize, FromRow)]
pub struct PredictionResponse {
    pub prediction_id: String,
    pub condition_id: String,
    pub question: String,
    pub description: String,
    pub tags: String,
    pub end_date: NaiveDateTime,
    pub created_at: NaiveDateTime,
}

#[derive(Serialize, Deserialize)]
pub struct PredictionResultResponse {
    pub prediction_id: String,
    pub condition_id: String,
    pub weighted: f64,
    pub community: f64,
}