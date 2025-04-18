SET timezone TO 'UTC';

CREATE TABLE IF NOT EXISTS accounts (
    account_id VARCHAR(255) PRIMARY KEY,
    api_key VARCHAR(255) NOT NULL UNIQUE,
    google_id VARCHAR(255) NOT NULL UNIQUE,
    session_id VARCHAR(255) DEFAULT NULL UNIQUE,
    email VARCHAR(255) NOT NULL UNIQUE,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS oauth_states (
    session_id VARCHAR(255) PRIMARY KEY,
    pkce_challenge VARCHAR(255) NOT NULL,
    csrf_token VARCHAR(255) NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMP NOT NULL DEFAULT NOW() + INTERVAL '10 minutes'
);

CREATE TABLE IF NOT EXISTS markets (
    condition_id VARCHAR(255) PRIMARY KEY,
    question TEXT NOT NULL,
    description TEXT NOT NULL,
    tags TEXT NOT NULL,
    yes_token_id VARCHAR(255) DEFAULT NULL,
    no_token_id VARCHAR(255) DEFAULT NULL,
    end_date TIMESTAMP NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS predictions (
    prediction_id VARCHAR(255) PRIMARY KEY,
    condition_id VARCHAR(255) NOT NULL UNIQUE,
    end_date TIMESTAMP NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS outcomes (
    prediction_id VARCHAR(255) NOT NULL,
    weighted FLOAT NOT NULL,
    community FLOAT NOT NULL,
    raw JSONB NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (prediction_id, created_at)
);

CREATE TABLE IF NOT EXISTS kv (
    kv_key VARCHAR(255) PRIMARY KEY,
    kv_value VARCHAR(255) DEFAULT NULL,
    kv_ts TIMESTAMP DEFAULT NULL
);

CREATE TABLE IF NOT EXISTS wallets (
    account_id VARCHAR(255) PRIMARY KEY,
    subscription_id VARCHAR(255) NOT NULL UNIQUE,
    address VARCHAR(255) NOT NULL UNIQUE,
    secret VARCHAR(255) NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);