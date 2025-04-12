use std::sync::Arc;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::{json, Value};

use crate::prelude::*;

pub async fn create_markets(State(state): State<Arc<AppState>>, auth: Auth) -> impl IntoResponse {
    let mut fetched = 0;
    let mut inserted = 0;
    let mut next_cursor = "MzUwMDA=".to_string();
    let mut markets: Vec<Value> = Vec::new();
    let client = Client::new();
    let now = Utc::now();

    let allowed_tags = vec![
        "Crypto", "Memecoins", "Politics", "Geopolitics", "Foreign Policy",
        "Breaking News", "Elon Musk", "Twitter", "Tech", "Business", "AI"
    ];
    let disallowed_tags = vec!["German Election", "Tweet Markets"];

    for _ in 0..50 {
        println!("Fetching page with next_cursor {}", next_cursor);

        let url = format!("https://clob.polymarket.com/markets?next_cursor={}", next_cursor);
        let response = match client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Error fetching data: {}", e);
                return JsonResponse::error("Failed to fetch market data", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

        let json: Value = match response.json().await {
            Ok(json) => json,
            Err(e) => {
                eprintln!("Error parsing response: {}", e);
                return JsonResponse::error("Failed to parse market data", StatusCode::BAD_REQUEST);
            }
        };

        if !json.get("data").is_some() {
            break;
        }

        if let Some(data) = json.get("data").and_then(|v| v.as_array()) {
            for item in data {
                markets.push(item.clone());
            }
        }

        match json.get("next_cursor").and_then(|v| v.as_str()) {
            Some(cursor) if cursor != next_cursor => {
                next_cursor = cursor.to_string();
            }
            _ => break,
        }
    }

    let markets: Vec<&Value> = markets.iter()
        .filter(|market| market.get("end_date_iso").is_some())
        .collect();

    for market in markets {
        let active = market.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
        let closed = market.get("closed").and_then(|v| v.as_bool()).unwrap_or(true);
        let archived = market.get("archived").and_then(|v| v.as_bool()).unwrap_or(true);

        if !active || closed || archived {
            continue;
        }

        let condition_id = match market.get("condition_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };

        let question = match market.get("question").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => continue,
        };

        let description = market.get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<&str>>()
            .join(" ");

        let tags = match market.get("tags").and_then(|v| v.as_array()) {
            Some(tags_arr) => tags_arr
                .iter()
                .filter_map(|t| t.as_str())
                .collect::<Vec<&str>>(),
            None => continue,
        };

        if !tags.iter().any(|tag| allowed_tags.contains(tag)) {
            continue;
        }

        if tags.iter().any(|tag| disallowed_tags.contains(tag)) {
            continue;
        }

        let tags = format!("[{}]", tags.join(", "));

        let mut yes_token_id = None;
        let mut no_token_id = None;

        if let Some(tokens) = market.get("tokens").and_then(|v| v.as_array()) {
            for token in tokens {
                if let Some(outcome) = token.get("outcome").and_then(|v| v.as_str()) {
                    if let Some(token_id) = token.get("token_id").and_then(|v| v.as_str()) {
                        match outcome.to_lowercase().as_str() {
                            "yes" | "up" => yes_token_id = Some(token_id.to_string()),
                            "no" | "down" => no_token_id = Some(token_id.to_string()),
                            _ => {}
                        }
                    }
                }
            }
        }

        let end_date = match market.get("end_date_iso").and_then(|v| v.as_str()) {
            Some(date) => date,
            None => continue,
        };

        if let Ok(date) = DateTime::parse_from_rfc3339(end_date) {
            if date < now {
                continue;
            }
        } else {
            continue;
        }

        fetched += 1;

        let result = sqlx::query(
            "INSERT INTO markets (condition_id, question, description, tags, yes_token_id, no_token_id, end_date, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7::timestamp, NOW())")
            .bind(&condition_id)
            .bind(&question)
            .bind(&description)
            .bind(&tags)
            .bind(&yes_token_id)
            .bind(&no_token_id)
            .bind(&end_date)
            .execute(&*state.pool)
            .await;

        match result {
            Ok(_) => {
                inserted += 1;
            },
            Err(e) => {
                if let Some(db_error) = e.as_database_error() {
                    if db_error.is_unique_violation() {
                        // Already exists, just continue
                    }
                }
            }
        }
    }

    JsonResponse::success(format!("{} markets fetched, {} markets inserted", fetched, inserted), StatusCode::OK)
}

pub async fn get_market_price(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> impl IntoResponse {
    let condition_id = match sqlx::query!(
        "SELECT m.condition_id FROM markets m
        JOIN predictions p ON m.condition_id = p.condition_id
        WHERE m.condition_id = $1 OR p.prediction_id = $1
        LIMIT 1",
        id)
        .fetch_optional(&*state.pool)
        .await {
            Ok(Some(record)) => record.condition_id,
            Ok(None) => return JsonResponse::error("Market not found", StatusCode::NOT_FOUND),
            Err(_) => return JsonResponse::error("Failed to fetch price", StatusCode::INTERNAL_SERVER_ERROR)
        };

    let market = match sqlx::query!(
        "SELECT yes_token_id FROM markets
        WHERE condition_id = $1 AND end_date > NOW() AND yes_token_id IS NOT NULL",
        condition_id)
        .fetch_optional(&*state.pool)
        .await {
            Ok(Some(market)) => market,
            Ok(None) => return JsonResponse::error("Market not found", StatusCode::NOT_FOUND),
            Err(_) => return JsonResponse::error("Failed to fetch price", StatusCode::INTERNAL_SERVER_ERROR)
        };

    let yes_token_id = match market.yes_token_id {
        Some(token_id) => token_id,
        None => return JsonResponse::error("Market not found", StatusCode::NOT_FOUND)
    };

    let client = Client::new();
    let body = vec![json!({"token_id": yes_token_id, "side": "BUY"})];

    let response = match client
        .post("https://clob.polymarket.com/prices")
        .json(&body)
        .send()
        .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Error fetching price: {}", e);
                return JsonResponse::error("Failed to fetch price", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    if !response.status().is_success() {
        let error = match response.text().await {
            Ok(text) => format!("API request failed: {}", text),
            Err(_) => "API request failed: Unable to read error response".to_string(),
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to fetch price", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let prices_json = match response.json::<Value>().await {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Error parsing price response: {}", e);
            return JsonResponse::error("Failed to fetch price", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if let Some(token) = prices_json.get(&yes_token_id) {
        if let Some(buy_price) = token.get("BUY").and_then(|v| v.as_str()) {
            return JsonResponse::success(json!({"condition_id": condition_id, "price": buy_price}), StatusCode::OK);
        }
    }

    JsonResponse::error("Failed to fetch price", StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn get_market_prices(State(state): State<Arc<AppState>>, auth: Auth) -> impl IntoResponse {
    let markets = match sqlx::query!(
        "SELECT condition_id, yes_token_id FROM markets
        WHERE end_date > NOW() AND yes_token_id IS NOT NULL
        ORDER BY end_date ASC
        LIMIT 500")
        .fetch_all(&*state.pool)
        .await {
            Ok(markets) => markets,
            Err(_) => return JsonResponse::error("Failed to fetch prices", StatusCode::INTERNAL_SERVER_ERROR)
        };

    if markets.is_empty() {
        return JsonResponse::success(Vec::<Value>::new(), StatusCode::OK);
    }

    let mut body = Vec::new();
    for market in &markets {
        if let Some(token_id) = &market.yes_token_id {
            body.push(json!({"token_id": token_id, "side": "BUY"}));
        }
    }

    let client = Client::new();
    let response = match client
        .post("https://clob.polymarket.com/prices")
        .json(&body)
        .send()
        .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Error fetching prices: {}", e);
                return JsonResponse::error("Failed to fetch prices", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    if !response.status().is_success() {
        let error = match response.text().await {
            Ok(text) => format!("API request failed: {}", text),
            Err(_) => "API request failed: Unable to read error response".to_string(),
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to fetch prices", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let prices_json = match response.json::<Value>().await {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Error parsing prices response: {}", e);
            return JsonResponse::error("Failed to fetch prices", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let mut response = Vec::new();
    for market in &markets {
        if let Some(token_id) = &market.yes_token_id {
            if let Some(token) = prices_json.get(token_id) {
                if let Some(buy_price) = token.get("BUY").and_then(|v| v.as_str()) {
                    response.push(json!({"condition_id": market.condition_id, "price": buy_price}));
                }
            }
        }
    }

    JsonResponse::success(response, StatusCode::OK)
}