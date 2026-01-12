use axum::{Json, Router, extract::State, routing::post};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    state::AppState,
    types::{Token, UserId},
};

#[derive(serde::Serialize)]
pub struct LoginResponse {
    pub token: Token,
    pub user_id: UserId,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/login", post(login))
}

async fn login(State(state): State<Arc<AppState>>) -> Json<LoginResponse> {
    // MVP: fake user
    let user_id = UserId::from(Uuid::new_v4());
    let token = Token::new();

    state.auth_tokens.insert(token.clone(), user_id);

    Json(LoginResponse { token, user_id })
}
