use std::sync::Arc;
use axum::{
    extract::{Path, State, Json},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::{json, Value};
use reqwest::Client;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};

use crate::prelude::*;

pub async fn get_address(State(state): State<Arc<AppState>>, auth: Auth) -> impl IntoResponse {
    let existing_wallet = sqlx::query_as!(
        Wallet,
        "SELECT * FROM wallets WHERE account_id = $1",
        auth.account_id)
        .fetch_optional(&*state.pool)
        .await;

    match existing_wallet {
        Ok(Some(wallet)) => JsonResponse::success(json!({"USDC": wallet.address}), StatusCode::OK),
        Ok(None) => {
            let client = Client::new();
            let response = match client
                .get(&format!("{}/v3/solana/wallet", state.config.tatum_api_url))
                .header("X-API-KEY", &state.config.tatum_api_key)
                .send()
                .await {
                    Ok(resp) => resp,
                    Err(e) => {
                        eprintln!("Error creating wallet: {}", e);
                        return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
                    }
                };

            if !response.status().is_success() {
                let error = match response.text().await {
                    Ok(text) => format!("API request failed: {}", text),
                    Err(_) => "API request failed: Unable to read error response".to_string(),
                };
                eprintln!("{}", error);
                return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
            }

            let wallet_data = match response.json::<Value>().await {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("Error parsing wallet response: {}", e);
                    return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            let address = match wallet_data.get("address").and_then(|v| v.as_str()) {
                Some(addr) => addr.to_string(),
                None => {
                    eprintln!("No address in wallet response");
                    return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            let secret = match wallet_data.get("privateKey").and_then(|v| v.as_str()) {
                Some(key) => key.to_string(),
                None => {
                    eprintln!("No private key in wallet response");
                    return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            let subscription = json!({
                "type": "INCOMING_FUNGIBLE_TX",
                "attr": {
                    "address": address,
                    "chain": "solana-mainnet",
                    "url": format!("{}/api/v1/webhook/tatum", state.config.base_url)
                }
            });

            let subscription_response = match client
                .post(&format!("{}/v4/subscription", state.config.tatum_api_url))
                .header("X-API-KEY", &state.config.tatum_api_key)
                .header("Content-Type", "application/json")
                .json(&subscription)
                .send()
                .await {
                    Ok(resp) => resp,
                    Err(e) => {
                        eprintln!("Error creating subscription: {}", e);
                        return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
                    }
                };

            if !subscription_response.status().is_success() {
                let error = match subscription_response.text().await {
                    Ok(text) => format!("Subscription API request failed: {}", text),
                    Err(_) => "Subscription API request failed: Unable to read error response".to_string(),
                };
                eprintln!("{}", error);
                return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
            }

            let subscription_data = match subscription_response.json::<Value>().await {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("Error parsing subscription response: {}", e);
                    return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            let subscription_id = match subscription_data.get("id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => {
                    eprintln!("No subscription ID in response");
                    return JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR);
                }
            };

            let result = sqlx::query!(
                "INSERT INTO wallets (account_id, subscription_id, address, secret) VALUES ($1, $2, $3, $4)",
                auth.account_id,
                subscription_id,
                address,
                secret)
                .execute(&*state.pool)
                .await;

            match result {
                Ok(_) => JsonResponse::success(json!({"USDC": address}), StatusCode::CREATED),
                Err(_) => JsonResponse::error("Failed to create wallet", StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
        Err(_) => JsonResponse::error("Failed to fetch wallet", StatusCode::INTERNAL_SERVER_ERROR)
    }
}

pub async fn get_balance(State(state): State<Arc<AppState>>, auth: Auth) -> impl IntoResponse {
    const USDC_CONTRACT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    let wallet = sqlx::query_as!(
        Wallet,
        "SELECT * FROM wallets WHERE account_id = $1",
        auth.account_id)
        .fetch_optional(&*state.pool)
        .await;

    let address = match wallet {
        Ok(Some(wallet)) => wallet.address,
        Ok(None) => return JsonResponse::success(json!({"USDC": "0.00"}), StatusCode::OK),
        Err(e) => {
            eprintln!("Error fetching wallet: {}", e);
            return JsonResponse::error("Failed to fetch balance", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let client = Client::new();
    let response = match client
        .get(&format!("{}/v3/blockchain/token/address/SOL/{}", state.config.tatum_api_url, address))
        .header("X-API-KEY", &state.config.tatum_api_key)
        .send()
        .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Error fetching balance: {}", e);
                return JsonResponse::error("Failed to fetch balance", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    if !response.status().is_success() {
        let error = match response.text().await {
            Ok(text) => format!("API request failed: {}", text),
            Err(_) => "API request failed: Unable to read error response".to_string(),
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to fetch balance", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let balances = match response.json::<Vec<Value>>().await {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Error parsing balance response: {}", e);
            return JsonResponse::error("Failed to fetch balance", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    for token in balances {
        if let Some(contract_address) = token.get("contractAddress").and_then(|v| v.as_str()) {
            if contract_address == USDC_CONTRACT {
                if let Some(amount) = token.get("amount").and_then(|v| v.as_str()) {
                    if let Ok(amount_float) = amount.parse::<f64>() {
                        let balance = (amount_float * 100.0).round() / 100.0;
                        return JsonResponse::success(json!({"USDC": format!("{}", balance)}), StatusCode::OK);
                    }
                }
            }
        }
    }

    JsonResponse::success(json!({"USDC": "0.00"}), StatusCode::OK)
}

pub async fn create_withdraw(State(state): State<Arc<AppState>>, auth: Auth, Json(payload): Json<Value>) -> impl IntoResponse {
    let address = match payload.get("address").and_then(|v| v.as_str()) {
        Some(address) => address,
        None => return JsonResponse::error("Invalid address", StatusCode::BAD_REQUEST)
    };

    if address.len() < 32 || address.len() > 44 {
        return JsonResponse::error("Invalid address", StatusCode::BAD_REQUEST);
    }

    let amount = match payload.get("amount").and_then(|v| v.as_str()) {
        Some(amount) => amount,
        None => return JsonResponse::error("Invalid amount", StatusCode::BAD_REQUEST)
    };

    match amount.parse::<f64>() {
        Ok(value) if value > 0.0 => value,
        _ => return JsonResponse::error("Invalid amount", StatusCode::BAD_REQUEST)
    };

    let wallet = match sqlx::query_as!(
        Wallet,
        "SELECT * FROM wallets WHERE account_id = $1",
        auth.account_id)
        .fetch_optional(&*state.pool)
        .await {
            Ok(Some(wallet)) => wallet,
            _ => return JsonResponse::error("Failed to create withdrawal", StatusCode::INTERNAL_SERVER_ERROR)
        };
    
    let transaction = json!({
        "chain": "SOL",
        "from": wallet.address,
        "to": address,
        "amount": amount,
        "contractAddress": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        "digits": 6,
        "fromPrivateKey": wallet.secret,
        "feePayer": state.config.solana_gas_address,
        "feePayerPrivateKey": state.config.solana_gas_secret
    });

    let client = Client::new();
    let response = match client
        .post(&format!("{}/v3/blockchain/token/transaction", state.config.tatum_api_url))
        .header("X-API-KEY", &state.config.tatum_api_key)
        .header("Content-Type", "application/json")
        .json(&transaction)
        .send()
        .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Error making withdrawal request: {}", e);
                return JsonResponse::error("Failed to create withdrawal", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    if !response.status().is_success() {
        let error = match response.text().await {
            Ok(text) => format!("API request failed: {}", text),
            Err(_) => "API request failed: Unable to read error response".to_string()
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to create withdrawal", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let response_data = match response.json::<Value>().await {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Error parsing response: {}", e);
            return JsonResponse::error("Failed to create withdrawal", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    match response_data.get("txId").and_then(|v| v.get("txId")).and_then(|v| v.as_str()) {
        Some(tx_id) => JsonResponse::success(tx_id, StatusCode::CREATED),
        None => {
            eprintln!("API request failed: {:?}", response_data);
            return JsonResponse::error("Failed to create withdrawal", StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
}

pub async fn tatum_webhook(State(state): State<Arc<AppState>>, Json(payload): Json<Value>) -> impl IntoResponse {
    JsonResponse::success(json!(payload), StatusCode::OK)
}

pub async fn create_swap(State(state): State<Arc<AppState>>, auth: Auth, Json(payload): Json<Value>) -> impl IntoResponse {
    const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const CBBTC_MINT: &str = "cbbtcf3aa214zXHbiAZQwf4122FBYbraNdFqgw4iMij";
    
    let amount = match payload.get("amount").and_then(|v| v.as_str()) {
        Some(amount) => amount,
        None => return JsonResponse::error("Invalid amount", StatusCode::BAD_REQUEST)
    };

    let amount_usdc = match amount.parse::<f64>() {
        Ok(value) if value > 0.0 => (value * 1000000.0) as u64,
        _ => return JsonResponse::error("Invalid amount", StatusCode::BAD_REQUEST)
    };

    let wallet = match sqlx::query_as!(
        Wallet,
        "SELECT * FROM wallets WHERE account_id = $1",
        auth.account_id)
        .fetch_optional(&*state.pool)
        .await {
            Ok(Some(wallet)) => wallet,
            _ => return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR)
        };

    // Get order from Jupiter Ultra API
    let client = Client::new();
    let order_url = format!(
        "https://lite-api.jup.ag/ultra/v1/order?inputMint={}&outputMint={}&amount={}&taker={}&feePayer={}",
        USDC_MINT, CBBTC_MINT, amount_usdc, wallet.address, state.config.solana_gas_address
    );

    let order_response = match client.get(&order_url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("Error getting order: {}", e);
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if !order_response.status().is_success() {
        let error = match order_response.text().await {
            Ok(text) => format!("Order API request failed: {}", text),
            Err(_) => "Order API request failed: Unable to read error response".to_string(),
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let order_data = match order_response.json::<Value>().await {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Error parsing order response: {}", e);
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let transaction = match order_data.get("transaction").and_then(|v| v.as_str()) {
        Some(tx) => tx,
        None => {
            eprintln!("No transaction in order response");
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let request_id = match order_data.get("requestId").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => {
            eprintln!("No request ID in order response");
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Decode the base64 transaction
    let transaction_bytes = match BASE64.decode(transaction) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Error decoding transaction: {}", e);
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Create keypair from private key
    let private_key_bytes: Vec<u8> = match bs58::decode(&wallet.secret).into_vec() {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Error decoding private key: {}", e);
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let keypair = match solana_sdk::signer::keypair::Keypair::from_bytes(&private_key_bytes) {
        Ok(kp) => kp,
        Err(e) => {
            eprintln!("Error creating keypair: {}", e);
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Deserialize the versioned transaction
    let mut transaction = match bincode::deserialize::<solana_sdk::transaction::VersionedTransaction>(&transaction_bytes) {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("Error deserializing transaction: {}", e);
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Sign the versioned transaction
    transaction.sign(&[&keypair], solana_sdk::hash::Hash::default());

    // Serialize the signed transaction
    let signed_transaction = match bincode::serialize(&transaction) {
        Ok(bytes) => BASE64.encode(bytes),
        Err(e) => {
            eprintln!("Error serializing transaction: {}", e);
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Execute the swap
    let execute_url = "https://lite-api.jup.ag/ultra/v1/execute";
    let execute_payload = json!({
        "signedTransaction": signed_transaction,
        "requestId": request_id
    });

    let execute_response = match client
        .post(execute_url)
        .header("Content-Type", "application/json")
        .json(&execute_payload)
        .send()
        .await {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Error executing swap: {}", e);
                return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

    if !execute_response.status().is_success() {
        let error = match execute_response.text().await {
            Ok(text) => format!("Execute API request failed: {}", text),
            Err(_) => "Execute API request failed: Unable to read error response".to_string(),
        };
        eprintln!("{}", error);
        return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
    }

    let execute_data = match execute_response.json::<Value>().await {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Error parsing execute response: {}", e);
            return JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    match execute_data.get("status").and_then(|v| v.as_str()) {
        Some("Success") => {
            if let Some(signature) = execute_data.get("signature").and_then(|v| v.as_str()) {
                JsonResponse::success(signature, StatusCode::CREATED)
            } else {
                JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR)
            }
        },
        Some("Failed") => {
            let error = execute_data.get("error").and_then(|v| v.as_str()).unwrap_or("Failed to create swap");
            JsonResponse::error(error, StatusCode::INTERNAL_SERVER_ERROR)
        },
        _ => JsonResponse::error("Failed to create swap", StatusCode::INTERNAL_SERVER_ERROR)
    }
}