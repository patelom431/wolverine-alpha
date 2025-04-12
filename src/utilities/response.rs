use axum::{
    http::{header, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use serde_json::{to_string, to_value, Value};

#[derive(Serialize)]
pub struct JsonResponse<T> where T: Serialize, {
    success: bool,
    response: T,
    #[serde(skip)]
    status_code: StatusCode,
}

impl JsonResponse<Value> {
    pub fn success<T: Serialize>(response: T, status_code: StatusCode) -> Self {
        JsonResponse {
            success: true,
            response: to_value(response).unwrap(),
            status_code,
        }
    }

    pub fn error(msg: impl Into<String>, status_code: StatusCode) -> Self {
        JsonResponse {
            success: false,
            response: to_value(msg.into()).unwrap(),
            status_code,
        }
    }
}

impl IntoResponse for JsonResponse<Value> {
    fn into_response(self) -> axum::response::Response {
        let body = to_string(&self).unwrap();
        (self.status_code, [(header::CONTENT_TYPE, "application/json")], body).into_response()
    }
}