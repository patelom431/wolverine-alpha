use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use std::sync::Arc;

use crate::prelude::*;

pub async fn start_tasks(app_state: Arc<AppState>) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3900));

    interval.tick().await;
    interval.tick().await;
    create_markets(app_state.clone()).await;
    track_predictions(app_state.clone()).await;
    create_predictions(app_state.clone()).await;

    loop {
        interval.tick().await;

        let result = sqlx::query!(
            "SELECT kv_key FROM kv WHERE kv_key = $1 AND kv_ts < NOW()",
            "predictions_task")
            .fetch_optional(&*app_state.pool)
            .await;

        match result {
            Ok(Some(_)) => {
                println!("Task: Starting tasks");
                create_markets(app_state.clone()).await;
                track_predictions(app_state.clone()).await;
                create_predictions(app_state.clone()).await;
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("Error checking last task run time: {}", e);
                continue;
            }
        }

        let _ = sqlx::query!(
            "INSERT INTO kv (kv_key, kv_ts)
            VALUES ($1, NOW() + INTERVAL '1 hour')
            ON CONFLICT (kv_key) DO UPDATE
            SET kv_ts = NOW() + INTERVAL '1 hour'",
            "predictions_task")
            .execute(&*app_state.pool)
            .await;
    }
}

async fn create_markets(app_state: Arc<AppState>) -> () {
    let mut fetched = 0;
    let mut inserted = 0;
    let mut next_cursor = "MzUwMDA=".to_string();
    let mut markets: Vec<Value> = Vec::new();
    let client = reqwest::Client::new();
    let now = Utc::now();

    let allowed_tags = vec![
        "Crypto", "Memecoins", "Politics", "Geopolitics", "Foreign Policy",
        "Breaking News", "Elon Musk", "Twitter", "Tech", "Business", "AI"
    ];
    let disallowed_tags = vec!["German Election", "Tweet Markets"];

    for _ in 0..50 {
        println!("Task: Fetching page with next_cursor {}", next_cursor);

        let url = format!("https://clob.polymarket.com/markets?next_cursor={}", next_cursor);
        let response = match client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Task: Error fetching data: {}", e);
                return;
            }
        };

        let json: Value = match response.json().await {
            Ok(json) => json,
            Err(e) => {
                eprintln!("Task: Error parsing response: {}", e);
                return;
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
            .execute(&*app_state.pool)
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

    println!("Task: {} markets fetched, {} markets inserted", fetched, inserted);
}

async fn track_predictions(app_state: Arc<AppState>) -> () {
    let predictions = match sqlx::query!(
        "SELECT p.* FROM predictions p
        WHERE p.end_date > NOW()")
        .fetch_all(&*app_state.pool)
        .await {
            Ok(predictions) => predictions,
            Err(e) => {
                eprintln!("Task: Error fetching predictions: {}", e);
                return;
            }
        };

    for prediction in predictions {
        let client = reqwest::Client::new();

        let weighted_url = format!("{}/api/v2/validator/events/{}/predictions", app_state.config.api_url, prediction.prediction_id);
        let weighted_response = match client
            .get(&weighted_url)
            .header("X-API-Key", app_state.config.api_key.clone())
            .send()
            .await {
                Ok(resp) => resp,
                Err(e) => {
                    eprintln!("Task: API request failed for weighted prediction for {}: {}", prediction.prediction_id, e);
                    continue;
                }
            };

        if !weighted_response.status().is_success() {
            let error = match weighted_response.text().await {
                Ok(text) => format!("Task: API request failed for weighted prediction for {}: {}", prediction.prediction_id, text),
                Err(_) => "Task: API request failed for weighted prediction".to_string(),
            };
            eprintln!("{}", error);
            continue;
        }

        let weighted_json = match weighted_response.json::<Value>().await {
            Ok(json) => json,
            Err(e) => {
                eprintln!("Task: Error parsing weighted predictions response for {}: {}", prediction.prediction_id, e);
                continue;
            }
        };

        let outcomes = match weighted_json.get("predictions").and_then(|v| v.as_array()) {
            Some(preds) => preds,
            None => {
                eprintln!("Task: No predictions available yet for {}", prediction.prediction_id);
                continue;
            }
        };

        if outcomes.is_empty() {
            eprintln!("Task: No predictions available yet for {}", prediction.prediction_id);
            continue;
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
            eprintln!("Task: No predictions available yet {}", prediction.prediction_id);
            continue;
        }

        let weighted = (sum / count as f64 * 10000.0).round() / 10000.0;

        let community_url = format!("{}/api/v2/validator/events/{}/community_prediction", app_state.config.api_url, prediction.prediction_id);
        let community_response = match client
            .get(&community_url)
            .header("X-API-Key", app_state.config.api_key.clone())
            .send()
            .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Task: API request failed for community prediction for {}: {}", prediction.prediction_id, e);
                continue;
            }
        };

        if !community_response.status().is_success() {
            let error = match community_response.text().await {
                Ok(text) => format!("Task: API request failed for community prediction for {}: {}", prediction.prediction_id, text),
                Err(_) => "Task: API request failed for community prediction".to_string(),
            };
            eprintln!("{}", error);
            continue;
        }

        let community = match community_response.json::<Value>().await {
            Ok(json) => match json.get("community_prediction").and_then(|v| v.as_f64()) {
                Some(value) => (value * 10000.0).round() / 10000.0,
                None => {
                    eprintln!("Task: No community prediction available for {}", prediction.prediction_id);
                    continue;
                }
            },
            Err(e) => {
                eprintln!("Task: Error parsing community prediction response for {}: {}", prediction.prediction_id, e);
                continue;
            }
        };

        let result = sqlx::query!(
            "INSERT INTO outcomes (prediction_id, weighted, community, raw) VALUES ($1, $2, $3, $4)",
            prediction.prediction_id,
            weighted,
            community,
            weighted_json)
            .execute(&*app_state.pool)
            .await;

        match result {
            Ok(_) => println!("Task: Stored new outcome for prediction {} (weighted: {}, community: {})", prediction.prediction_id, weighted, community),
            Err(e) => eprintln!("Task: Error storing outcome for prediction {}: {}", prediction.prediction_id, e),
        }
    }
}

async fn create_predictions(app_state: Arc<AppState>) -> () {
    let markets = match sqlx::query_as!(
        Market,
        "SELECT m.*
        FROM markets m
        WHERE m.end_date > NOW() + INTERVAL '1 day'
        AND NOT EXISTS (
            SELECT 1 FROM predictions p
            WHERE p.condition_id = m.condition_id
        )
        ORDER BY m.end_date ASC
        LIMIT 5")
        .fetch_all(&*app_state.pool)
        .await {
            Ok(market) => market,
            Err(e) => {
                eprintln!("Task: Error fetching markets: {}", e);
                return;
            }
        };

    for market in markets {
        let cutoff = market.end_date.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true);

        let request_body = serde_json::json!({
            "title": market.question,
            "description": market.description,
            "cutoff": cutoff,
        });

        let client = reqwest::Client::new();
        let api_url = format!("{}/api/v2/events", app_state.config.api_url);

        let response = match client
            .post(api_url)
            .header("X-API-Key", app_state.config.api_key.clone())
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await {
                Ok(resp) => resp,
                Err(e) => {
                    eprintln!("Task: Error making API request: {}", e);
                    continue;
                }
            };

        if !response.status().is_success() {
            let error = match response.text().await {
                Ok(text) => format!("Task: API request failed for prediction: {}\nRequest: {}\nResponse: {}", market.condition_id, request_body, text),
                Err(_) => format!("Task: API request failed for prediction: {}\nRequest: {}\nError: Unable to read response", market.question, request_body),
            };
            eprintln!("{}", error);
            continue;
        }

        let prediction_response = match response.json::<Value>().await {
            Ok(json) => json,
            Err(e) => {
                eprintln!("Task: Failed to parse API response for prediction: {}\nRequest: {}\nError: {}", market.condition_id, request_body, e);
                continue;
            }
        };

        let prediction_id = match prediction_response.get("event_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => {
                eprintln!("Task: No event_id in API response for prediction: {}\nRequest: {}\nResponse: {}", market.condition_id, request_body, prediction_response);
                continue;
            }
        };

        let end_date = market.end_date;

        let _ = sqlx::query!(
            "INSERT INTO predictions (prediction_id, condition_id, end_date, created_at) VALUES ($1, $2, $3, NOW())",
            prediction_id,
            market.condition_id,
            end_date)
            .execute(&*app_state.pool)
            .await;

        println!("Task: Created prediction {} for market \"{}\"", prediction_id, market.question);
        return;
    }
}