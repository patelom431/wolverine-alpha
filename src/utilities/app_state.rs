use serde::Deserialize;
use std::{sync::Arc, env};
use sqlx::{Postgres, Pool, PgPool};
use oauth2::{
    basic::BasicClient,
    AuthUrl, TokenUrl, RedirectUrl, ClientId, ClientSecret,
};

#[derive(Deserialize, Clone)]
pub struct Config {
    pub server_ip: String,
    pub server_port: u16,
    pub base_url: String,
    pub database_url: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub api_key: String,
    pub api_url: String,
    pub tatum_api_key: String,
    pub tatum_api_url: String,
    pub solana_gas_address: String,
    pub solana_gas_secret: String,
}

impl Config {
    pub fn from_env() -> Self {
        dotenv::dotenv().ok();

        let server_ip = env::var("SERVER_IP").expect("SERVER_IP must be set");
        let server_port = env::var("SERVER_PORT").expect("SERVER_PORT must be set")
            .parse::<u16>()
            .expect("SERVER_PORT must be a valid port number");
        let base_url = env::var("BASE_URL").expect("BASE_URL must be set");
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let google_client_id = env::var("GOOGLE_CLIENT_ID").expect("GOOGLE_CLIENT_ID must be set");
        let google_client_secret = env::var("GOOGLE_CLIENT_SECRET").expect("GOOGLE_CLIENT_SECRET must be set");
        let api_key = env::var("API_KEY").expect("API_KEY must be set");
        let api_url = env::var("API_URL").expect("API_URL must be set");
        let tatum_api_key = env::var("TATUM_API_KEY").expect("TATUM_API_KEY must be set");
        let tatum_api_url = env::var("TATUM_API_URL").expect("TATUM_API_URL must be set");
        let solana_gas_address = env::var("SOLANA_GAS_ADDRESS").expect("SOLANA_GAS_ADDRESS must be set");
        let solana_gas_secret = env::var("SOLANA_GAS_SECRET").expect("SOLANA_GAS_SECRET must be set");

        Config {
            server_ip,
            server_port,
            base_url,
            database_url,
            google_client_id,
            google_client_secret,
            api_key,
            api_url,
            tatum_api_key,
            tatum_api_url,
            solana_gas_address,
            solana_gas_secret,
        }
    }
}

pub struct AppState {
    pub config: Arc<Config>,
    pub pool: Arc<PgPool>,
    pub oauth: Arc<BasicClient>,
}

impl AppState {
    pub async fn create() -> Arc<Self> {
        let config = Arc::new(Config::from_env());
        let pool = Arc::new(Self::establish_connection(&config).await);
        let oauth = Arc::new(Self::create_oauth_client(&config));

        Arc::new(AppState {
            config,
            pool,
            oauth,
        })
    }

    async fn establish_connection(config: &Config) -> PgPool {
        Pool::<Postgres>::connect(&config.database_url).await.expect("Failed to connect to the database")
    }

    fn create_oauth_client(config: &Config) -> BasicClient {
        BasicClient::new(
            ClientId::new(config.google_client_id.clone()),
            Some(ClientSecret::new(config.google_client_secret.clone())),
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).unwrap(),
            Some(TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).unwrap()),
        ).set_redirect_uri(RedirectUrl::new(format!("{}/auth/google/callback", config.base_url)).unwrap())
    }
}