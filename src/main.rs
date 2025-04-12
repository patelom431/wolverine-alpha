use std::sync::Arc;
use tokio;

mod models;
mod utilities;
mod routes;
mod router;
mod prelude;

use crate::prelude::*;

#[tokio::main]
async fn main() {
    let app_state = AppState::create().await;
    let app = router::app_router(Arc::clone(&app_state));

    let address = format!("{}:{}", app_state.config.server_ip, app_state.config.server_port);
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();

    tokio::spawn(async move {
        tasks::start_tasks(app_state).await;
    });

    axum::serve(listener, app.into_make_service()).await.unwrap();
}