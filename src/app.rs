use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use tower_http::{services::ServeDir, trace::TraceLayer};

use crate::{
    error::AppError,
    models::{AvailableTagsResponse, SearchParams, SearchResponse, SuggestionResponse},
    search::SearchService,
};

#[derive(Clone)]
pub(crate) struct AppState {
    service: SearchService,
}

impl AppState {
    pub(crate) fn new(service: SearchService) -> Self {
        Self { service }
    }
}

pub(crate) fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/search", get(search_handler))
        .route("/api/suggest", get(suggest_handler))
        .route("/api/tags", get(tags_handler))
        .with_state(state)
        .fallback_service(ServeDir::new("frontend").append_index_html_on_directories(true))
        .layer(TraceLayer::new_for_http())
}

async fn search_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, AppError> {
    let response = state.service.search(&params.q).await?;
    Ok(Json(response))
}

async fn tags_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AvailableTagsResponse>, AppError> {
    let tags = state.service.available_hidden_tags().await?;
    Ok(Json(AvailableTagsResponse { tags }))
}

async fn suggest_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SuggestionResponse>, AppError> {
    let suggestions = state.service.suggest_tags(&params.q).await?;
    Ok(Json(SuggestionResponse { suggestions }))
}
