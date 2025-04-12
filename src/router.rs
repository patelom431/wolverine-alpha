use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use chrono::SecondsFormat;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use tower_http::{trace, trace::TraceLayer};
use tracing::Level;

use crate::prelude::*;

pub fn app_router(app_state: Arc<AppState>) -> Router {
    tracing_subscriber::fmt()
        .with_target(false)
        //.json()
        .init();

    Router::new()
        .route("/", get(index))
        .route("/prediction/{id}", get(prediction))
        .route("/auth/google", get(google_login))
        .route("/auth/google/callback", get(google_callback))
        .route("/auth/logout", get(logout))
        .route("/auth/session", get(get_session))
        .route("/api/v1/markets", get(get_markets).post(create_markets))
        .route("/api/v1/market/{id}/price", get(get_market_price))
        .route("/api/v1/markets/prices", get(get_market_prices))
        .route("/api/v1/prediction/{id}", get(get_prediction).post(create_prediction))
        .route("/api/v1/prediction/{id}/result", get(get_prediction_result))
        .route("/api/v1/prediction/{id}/historical", get(get_prediction_historical))
        .route("/api/v1/predictions/results", get(get_prediction_results))
        .route("/api/v1/rates", get(get_rates))
        .route("/api/v1/wallet/address", get(get_address))
        .route("/api/v1/wallet/balance", get(get_balance))
        .route("/api/v1/wallet/withdraw", post(create_withdraw))
        .route("/api/v1/wallet/swap", post(create_swap))
        .route("/api/v1/webhook/tatum", post(tatum_webhook))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
                .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(app_state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../frontend/index.html"))
}

async fn prediction() -> Html<&'static str> {
    Html(include_str!("../frontend/prediction.html"))
}

async fn get_markets(State(state): State<Arc<AppState>>, auth: Auth) -> impl IntoResponse {
    let result = sqlx::query_as!(
        Market,
        "SELECT * FROM markets WHERE end_date >= NOW() ORDER BY end_date ASC")
        .fetch_all(&*state.pool)
        .await;

    match result {
        Ok(markets) => JsonResponse::success(markets, StatusCode::OK),
        Err(_) => JsonResponse::error("Failed to fetch markets", StatusCode::INTERNAL_SERVER_ERROR)
    }
}

async fn get_prediction(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> impl IntoResponse {
    let result = sqlx::query_as!(
        PredictionResponse,
        "SELECT p.prediction_id, m.condition_id, m.question, m.description, m.tags, m.end_date, m.created_at
        FROM markets m
        JOIN predictions p ON m.condition_id = p.condition_id
        WHERE p.prediction_id = $1 OR m.condition_id = $1
        LIMIT 1",
        id)
        .fetch_optional(&*state.pool)
        .await;

    match result {
        Ok(Some(prediction)) => JsonResponse::success(prediction, StatusCode::OK),
        Ok(None) => JsonResponse::error("No prediction found", StatusCode::NOT_FOUND),
        Err(_) => JsonResponse::error("Failed to fetch prediction", StatusCode::INTERNAL_SERVER_ERROR)
    }
}

async fn create_prediction(State(state): State<Arc<AppState>>, auth: Auth, Path(condition_id): Path<String>) -> impl IntoResponse {
    let result = sqlx::query_as!(
        Market,
        "SELECT * FROM markets WHERE condition_id = $1 AND end_date > NOW()",
        condition_id)
        .fetch_optional(&*state.pool)
        .await;

    let market = match result {
        Ok(Some(market)) => market,
        Ok(None) => return JsonResponse::error("Market not found", StatusCode::NOT_FOUND),
        Err(_) => return JsonResponse::error("Failed to create prediction", StatusCode::INTERNAL_SERVER_ERROR)
    };

    let existing_prediction = sqlx::query!(
        "SELECT prediction_id FROM predictions WHERE condition_id = $1",
        condition_id)
        .fetch_optional(&*state.pool)
        .await;

    match existing_prediction {
        Ok(None) => (),
        Ok(Some(_)) => return JsonResponse::error("Prediction already exists for this market", StatusCode::CONFLICT),
        Err(_) => return JsonResponse::error("Failed to create prediction", StatusCode::INTERNAL_SERVER_ERROR)
    }

    let cutoff = market.end_date.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true);

    let request_body = json!({
        "title": market.question,
        "description": market.description,
        "cutoff": cutoff,
    });

    let client = Client::new();
    let api_url = format!("{}/api/v2/events", &state.config.api_url);

    let response = match client
        .post(api_url)
        .header("X-API-Key", &state.config.api_key)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Error making API request: {}", e);
                return JsonResponse::error("Failed to create prediction", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    if !response.status().is_success() {
        let error = match response.text().await {
            Ok(text) => format!("API request failed for prediction: {}\nRequest: {}\nResponse: {}", condition_id, request_body, text),
            Err(_) => format!("API request failed for prediction: {}\nRequest: {}\nError: Unable to read response", condition_id, request_body)
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to create prediction", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let prediction_response = match response.json::<Value>().await {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Failed to parse API response for prediction: {}\nRequest: {}\nError: {}", condition_id, request_body, e);
            return JsonResponse::error("Failed to create prediction", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let prediction_id = match prediction_response.get("event_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            eprintln!("No event_id in API response for prediction: {}\nRequest: {}\nResponse: {}", condition_id, request_body, prediction_response);
            return JsonResponse::error("Failed to create prediction", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let end_date = market.end_date;

    let result = sqlx::query!(
        "INSERT INTO predictions (prediction_id, condition_id, end_date, created_at) VALUES ($1, $2, $3, NOW())",
        prediction_id,
        condition_id,
        end_date)
        .execute(&*state.pool)
        .await;

    match result {
        Ok(_) => JsonResponse::success(Prediction { prediction_id, condition_id, end_date }, StatusCode::CREATED),
        Err(_) => JsonResponse::error("Failed to create prediction", StatusCode::INTERNAL_SERVER_ERROR)
    }
}

async fn get_prediction_result(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> impl IntoResponse {
    let prediction = match sqlx::query!(
        "SELECT prediction_id FROM predictions WHERE prediction_id = $1 OR condition_id = $1",
        id)
        .fetch_optional(&*state.pool)
        .await {
            Ok(Some(pred)) => pred,
            Ok(None) => return JsonResponse::error("Prediction not found", StatusCode::NOT_FOUND),
            Err(_) => return JsonResponse::error("Failed to create prediction", StatusCode::INTERNAL_SERVER_ERROR)
        };

    let client = Client::new();

    let weighted_url = format!("{}/api/v2/validator/events/{}/predictions", &state.config.api_url, prediction.prediction_id);
    let weighted_response = match client
        .get(&weighted_url)
        .header("X-API-Key", &state.config.api_key)
        .send()
        .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Task: API request failed for weighted prediction for {}: {}", prediction.prediction_id, e);
                return JsonResponse::error("Failed to fetch predictions", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    if !weighted_response.status().is_success() {
        let error = match weighted_response.text().await {
            Ok(text) => format!("Task: API request failed for weighted prediction for {}: {}", prediction.prediction_id, text),
            Err(_) => "API request failed for weighted prediction. Error: Unable to read error response".to_string()
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to fetch predictions", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let weighted_json = match weighted_response.json::<Value>().await {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Task: Error parsing weighted predictions response for {}: {}", prediction.prediction_id, e);
            return JsonResponse::error("Failed to fetch predictions", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let outcomes = match weighted_json.get("predictions").and_then(|v| v.as_array()) {
        Some(preds) => preds,
        None => return JsonResponse::error("No predictions available yet", StatusCode::UNPROCESSABLE_ENTITY)
    };

    if outcomes.is_empty() {
        return JsonResponse::error("No predictions available yet", StatusCode::UNPROCESSABLE_ENTITY);
    }

    let mut sum = 0.0;
    let mut count = 0;

    for outcome in outcomes {
        if let Some(pred) = outcome.get("predictedOutcome").and_then(|v| v.as_str()) {
            if let Ok(value) = pred.parse::<f64>() {
                sum += value;
                count += 1;
            }
        }
    }

    if count == 0 {
        return JsonResponse::error("No predictions available yet", StatusCode::UNPROCESSABLE_ENTITY);
    }

    let weighted = (sum / count as f64 * 10000.0).round() / 10000.0;

    let community_url = format!("{}/api/v2/validator/events/{}/community_prediction", &state.config.api_url, prediction.prediction_id);
    let community_response = match client
        .get(&community_url)
        .header("X-API-Key", &state.config.api_key)
        .send()
        .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Task: API request failed for community prediction for {}: {}", prediction.prediction_id, e);
                return JsonResponse::error("Failed to fetch community prediction", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    if !community_response.status().is_success() {
        let error = match community_response.text().await {
            Ok(text) => format!("Task: API request failed for community prediction for {}: {}", prediction.prediction_id, text),
            Err(_) => "API request failed for community prediction. Error: Unable to read error response".to_string(),
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to fetch community prediction", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let community = match community_response.json::<Value>().await {
        Ok(json) => match json.get("community_prediction").and_then(|v| v.as_f64()) {
            Some(value) => (value * 10000.0).round() / 10000.0,
            None => return JsonResponse::error("Failed to fetch community prediction", StatusCode::INTERNAL_SERVER_ERROR)
        },
        Err(e) => {
            eprintln!("Task: Error parsing community prediction response for {}: {}", prediction.prediction_id, e);
            return JsonResponse::error("Failed to fetch community prediction", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let _ = sqlx::query!(
        "INSERT INTO outcomes (prediction_id, weighted, community, raw) VALUES ($1, $2, $3, $4)",
        prediction.prediction_id,
        weighted,
        community,
        weighted_json)
        .execute(&*state.pool)
        .await;

    JsonResponse::success(json!({"weighted": weighted,"community": community}), StatusCode::OK)
}

async fn get_prediction_historical(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> impl IntoResponse {

}

async fn get_prediction_results(State(state): State<Arc<AppState>>, auth: Auth) -> impl IntoResponse {
    let result = sqlx::query_as!(
        PredictionResultResponse,
        "WITH latest_outcomes AS (
            SELECT DISTINCT ON (prediction_id) prediction_id, weighted, community
            FROM outcomes
            ORDER BY prediction_id, created_at DESC
        )
        SELECT o.prediction_id, p.condition_id, o.weighted, o.community
        FROM latest_outcomes o
        JOIN predictions p ON o.prediction_id = p.prediction_id
        WHERE p.end_date >= NOW()
        ORDER BY p.end_date ASC")
        .fetch_all(&*state.pool)
        .await;

    match result {
        Ok(outcomes) => JsonResponse::success(outcomes, StatusCode::OK),
        Err(_) => JsonResponse::error("Failed to fetch prediction results", StatusCode::INTERNAL_SERVER_ERROR)
    }
}

async fn get_rates() -> impl IntoResponse {
    const CURRENCIES: [&str; 3] = ["BTC", "SOL", "TAO"];
    
    let client = Client::new();
    let url = "https://api.coinbase.com/v2/exchange-rates?currency=USDC";

    match client.get(url).send().await {
        Ok(response) => {
            if !response.status().is_success() {
                return JsonResponse::error("Failed to fetch rates", StatusCode::INTERNAL_SERVER_ERROR);
            }

            match response.json::<Value>().await {
                Ok(data) => {
                    if let Some(rates) = data.get("data").and_then(|d| d.get("rates")) {
                        let mut response = json!({});
                        
                        for currency in CURRENCIES.iter() {
                            if let Some(rate_str) = rates.get(currency).and_then(|v| v.as_str()) {
                                if let Ok(rate) = rate_str.parse::<f64>() {
                                    response[currency] = json!(((1.0 / rate) * 100.0).round() / 100.0);
                                }
                            }
                        }
                        
                        return JsonResponse::success(response, StatusCode::OK);
                    }

                    JsonResponse::error("Failed to fetch rates", StatusCode::INTERNAL_SERVER_ERROR)
                }
                Err(_) => JsonResponse::error("Failed to parse rates", StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
        Err(_) => JsonResponse::error("Failed to fetch rates", StatusCode::INTERNAL_SERVER_ERROR)
    }
}