use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write as _,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use futures::{StreamExt, TryStreamExt, stream};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize, de::Deserializer};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    fs,
    sync::{Mutex, RwLock},
};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::{error, info};

const LEETCODE_GRAPHQL_URL: &str = "https://leetcode.com/graphql/";
const LEETCODE_CATALOG_URL: &str = "https://leetcode.com/api/problems/algorithms/";
const CACHE_MAX_AGE: Duration = Duration::from_secs(60 * 60 * 24);
const GRAPHQL_BATCH_SIZE: usize = 20;
const GRAPHQL_CONCURRENCY: usize = 8;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "reverse_tag_lookup=info,tower_http=info".to_string()),
        )
        .init();

    let cache_path = std::env::var("CACHE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data/problem-cache.json"));

    let state = Arc::new(AppState {
        service: SearchService::new(cache_path).await?,
    });

    let app = Router::new()
        .route("/api/search", get(search_handler))
        .route("/api/suggest", get(suggest_handler))
        .route("/api/tags", get(tags_handler))
        .with_state(state)
        .fallback_service(ServeDir::new("frontend").append_index_html_on_directories(true))
        .layer(TraceLayer::new_for_http());

    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let host = std::env::var("HOST")
        .ok()
        .and_then(|value| value.parse::<IpAddr>().ok())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let address = SocketAddr::from((host, port));

    info!("listening on http://{}", address);

    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Clone)]
struct AppState {
    service: SearchService,
}

#[derive(Clone)]
struct SearchService {
    client: reqwest::Client,
    cache_path: PathBuf,
    problems: Arc<RwLock<HashMap<String, ProblemRecord>>>,
    hidden_tags: Arc<RwLock<Option<Vec<TagRecord>>>>,
    refresh_lock: Arc<Mutex<()>>,
    cache_meta: Arc<RwLock<CacheMeta>>,
}

impl SearchService {
    async fn new(cache_path: PathBuf) -> Result<Self, AppError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("reverse-tag-lookup/0.1 (+https://github.com/openai/codex)"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .brotli(true)
            .gzip(true)
            .deflate(true)
            .build()
            .map_err(AppError::HttpClient)?;

        let mut problems = HashMap::new();
        let mut cache_meta = CacheMeta::default();

        if let Ok(contents) = fs::read(&cache_path).await {
            match serde_json::from_slice::<CacheFile>(&contents) {
                Ok(cache_file) => {
                    for problem in cache_file.problems {
                        problems.insert(problem.slug.clone(), problem);
                    }
                    cache_meta.saved_at_unix = cache_file.saved_at_unix;
                    cache_meta.catalog_total = cache_file.catalog_total;
                }
                Err(error) => {
                    error!(%error, "failed to parse local cache; starting cold");
                }
            }
        }

        Ok(Self {
            client,
            cache_path,
            problems: Arc::new(RwLock::new(problems)),
            hidden_tags: Arc::new(RwLock::new(None)),
            refresh_lock: Arc::new(Mutex::new(())),
            cache_meta: Arc::new(RwLock::new(cache_meta)),
        })
    }

    async fn search(&self, raw_query: &str) -> Result<SearchResponse, AppError> {
        let query = raw_query.trim();
        if query.is_empty() {
            return Err(AppError::BadRequest("Query cannot be empty.".to_string()));
        }

        self.ensure_problem_cache().await?;

        let normalized_query = normalize(query);
        let problems = self.problems.read().await;
        let mut matched_tag_map = BTreeMap::<(TagCategory, String), TagRecord>::new();
        let mut results = Vec::new();

        for problem in problems.values() {
            let mut matched_tags = Vec::new();

            for tag in problem.all_tags() {
                if tag_matches(tag, &normalized_query) {
                    matched_tag_map
                        .entry((tag.category, tag.slug.clone()))
                        .or_insert_with(|| tag.clone());
                    matched_tags.push(tag.clone());
                }
            }

            if !matched_tags.is_empty() {
                matched_tags.sort_by(tag_sort_key);
                matched_tags.dedup_by(|left, right| {
                    left.category == right.category && left.slug == right.slug
                });

                results.push(SearchResult {
                    id: problem.id.clone(),
                    title: problem.title.clone(),
                    slug: problem.slug.clone(),
                    difficulty: problem.difficulty.clone(),
                    acceptance: problem.acceptance,
                    paid_only: problem.paid_only,
                    url: format!("https://leetcode.com/problems/{}/", problem.slug),
                    tags: matched_tags.iter().map(|tag| tag.name.clone()).collect(),
                    matched_tags,
                });
            }
        }

        results.sort_by(|left, right| {
            right
                .matched_tags
                .len()
                .cmp(&left.matched_tags.len())
                .then_with(|| numeric_id(&left.id).cmp(&numeric_id(&right.id)))
                .then_with(|| left.title.cmp(&right.title))
        });

        let hidden_tags = self.available_hidden_tags().await?;

        Ok(SearchResponse {
            query: query.to_string(),
            matched_tags: matched_tag_map.into_values().collect(),
            available_hidden_tags: hidden_tags,
            results,
            cache_ready: true,
        })
    }

    async fn available_hidden_tags(&self) -> Result<Vec<TagRecord>, AppError> {
        if let Some(tags) = self.hidden_tags.read().await.clone() {
            return Ok(tags);
        }

        let _guard = self.refresh_lock.lock().await;
        if let Some(tags) = self.hidden_tags.read().await.clone() {
            return Ok(tags);
        }

        let tags = self.fetch_hidden_tag_catalog().await?;
        *self.hidden_tags.write().await = Some(tags.clone());
        Ok(tags)
    }

    async fn suggest_tags(&self, raw_query: &str) -> Result<Vec<TagRecord>, AppError> {
        let query = raw_query.trim();
        if query.is_empty() {
            return Ok(self.available_hidden_tags().await?);
        }

        self.ensure_problem_cache().await?;

        let normalized_query = normalize(query);
        let mut seen = HashSet::<(TagCategory, String)>::new();
        let mut suggestions = Vec::<TagRecord>::new();

        for tag in self.available_hidden_tags().await? {
            if tag_matches(&tag, &normalized_query) && seen.insert((tag.category, tag.slug.clone()))
            {
                suggestions.push(tag);
            }
        }

        let problems = self.problems.read().await;
        for problem in problems.values() {
            for tag in problem.all_tags() {
                if tag_matches(tag, &normalized_query)
                    && seen.insert((tag.category, tag.slug.clone()))
                {
                    suggestions.push(tag.clone());
                }
            }
        }

        suggestions.sort_by(|left, right| {
            tag_match_priority(left, &normalized_query)
                .cmp(&tag_match_priority(right, &normalized_query))
                .then_with(|| tag_sort_key(left, right))
        });
        suggestions.truncate(10);

        Ok(suggestions)
    }

    async fn ensure_problem_cache(&self) -> Result<(), AppError> {
        if self.cache_is_fresh().await {
            return Ok(());
        }

        let _guard = self.refresh_lock.lock().await;

        if self.cache_is_fresh().await {
            return Ok(());
        }

        let catalog = self.fetch_problem_catalog().await?;
        let missing = {
            let problems = self.problems.read().await;
            catalog
                .iter()
                .filter(|entry| !problems.contains_key(&entry.slug))
                .map(|entry| entry.slug.clone())
                .collect::<Vec<_>>()
        };

        if !missing.is_empty() {
            info!("warming {} uncached problems from LeetCode", missing.len());
            let fetched = self.fetch_problem_details(&missing).await?;
            let catalog_lookup = catalog
                .iter()
                .map(|entry| (entry.slug.as_str(), entry))
                .collect::<HashMap<_, _>>();

            let mut problems = self.problems.write().await;
            for mut detail in fetched {
                if let Some(catalog_entry) = catalog_lookup.get(detail.slug.as_str()) {
                    detail.acceptance = catalog_entry.acceptance;
                    detail.paid_only = catalog_entry.paid_only;
                    if detail.title.is_empty() {
                        detail.title = catalog_entry.title.clone();
                    }
                    if detail.id.is_empty() {
                        detail.id = catalog_entry.id.clone();
                    }
                    if detail.difficulty.is_empty() {
                        detail.difficulty = catalog_entry.difficulty.clone();
                    }
                }

                problems.insert(detail.slug.clone(), detail);
            }
        }

        {
            let mut problems = self.problems.write().await;
            let live_slugs = catalog
                .iter()
                .map(|entry| entry.slug.clone())
                .collect::<HashSet<_>>();
            problems.retain(|slug, _| live_slugs.contains(slug));

            for entry in &catalog {
                if let Some(problem) = problems.get_mut(&entry.slug) {
                    problem.id = entry.id.clone();
                    problem.title = entry.title.clone();
                    problem.difficulty = entry.difficulty.clone();
                    problem.acceptance = entry.acceptance;
                    problem.paid_only = entry.paid_only;
                }
            }
        }

        let now = unix_now();
        {
            let mut meta = self.cache_meta.write().await;
            meta.saved_at_unix = now;
            meta.catalog_total = catalog.len();
        }

        self.persist_cache().await?;
        Ok(())
    }

    async fn cache_is_fresh(&self) -> bool {
        let meta = self.cache_meta.read().await;
        let problems = self.problems.read().await;

        if problems.is_empty() || meta.catalog_total == 0 {
            return false;
        }

        if problems.len() < meta.catalog_total {
            return false;
        }

        let age = unix_now().saturating_sub(meta.saved_at_unix);
        age < CACHE_MAX_AGE.as_secs()
    }

    async fn persist_cache(&self) -> Result<(), AppError> {
        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent).await.map_err(AppError::Io)?;
        }

        let meta = self.cache_meta.read().await.clone();
        let problems = self.problems.read().await;

        let mut values = problems.values().cloned().collect::<Vec<_>>();
        values.sort_by(|left, right| numeric_id(&left.id).cmp(&numeric_id(&right.id)));

        let cache_file = CacheFile {
            saved_at_unix: meta.saved_at_unix,
            catalog_total: meta.catalog_total,
            problems: values,
        };

        let serialized = serde_json::to_vec_pretty(&cache_file).map_err(AppError::Serialize)?;
        fs::write(&self.cache_path, serialized)
            .await
            .map_err(AppError::Io)?;

        Ok(())
    }

    async fn fetch_hidden_tag_catalog(&self) -> Result<Vec<TagRecord>, AppError> {
        let response = self
            .client
            .post(LEETCODE_GRAPHQL_URL)
            .json(&json!({
                "query": "query HiddenTags { problemsetPositionLevelTags { name slug } }"
            }))
            .send()
            .await
            .map_err(AppError::Request)?;

        let payload = response
            .error_for_status()
            .map_err(AppError::Request)?
            .json::<HiddenTagsResponse>()
            .await
            .map_err(AppError::Request)?;

        let data = payload.data.ok_or_else(|| {
            AppError::Upstream("LeetCode returned no hidden tag data.".to_string())
        })?;

        let mut tags = Vec::new();
        tags.extend(
            data.problemset_position_level_tags
                .into_iter()
                .map(|tag| TagRecord {
                    name: tag.name,
                    slug: tag.slug,
                    category: TagCategory::PositionLevel,
                }),
        );

        tags.sort_by(tag_sort_key);
        Ok(tags)
    }

    async fn fetch_problem_catalog(&self) -> Result<Vec<CatalogProblem>, AppError> {
        let response = self
            .client
            .get(LEETCODE_CATALOG_URL)
            .send()
            .await
            .map_err(AppError::Request)?;

        let payload = response
            .error_for_status()
            .map_err(AppError::Request)?
            .json::<CatalogResponse>()
            .await
            .map_err(AppError::Request)?;

        let mut problems = payload
            .stat_status_pairs
            .into_iter()
            .filter(|pair| !pair.stat.hidden)
            .map(CatalogProblem::from)
            .collect::<Vec<_>>();

        problems.sort_by(|left, right| numeric_id(&left.id).cmp(&numeric_id(&right.id)));
        Ok(problems)
    }

    async fn fetch_problem_details(
        &self,
        slugs: &[String],
    ) -> Result<Vec<ProblemRecord>, AppError> {
        let chunks = slugs
            .chunks(GRAPHQL_BATCH_SIZE)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();

        let results = stream::iter(
            chunks
                .into_iter()
                .map(|chunk| async move { self.fetch_problem_batch(chunk).await }),
        )
        .buffer_unordered(GRAPHQL_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;

        Ok(results.into_iter().flatten().collect())
    }

    async fn fetch_problem_batch(
        &self,
        slugs: Vec<String>,
    ) -> Result<Vec<ProblemRecord>, AppError> {
        let query = build_question_batch_query(&slugs);
        let response = self
            .client
            .post(LEETCODE_GRAPHQL_URL)
            .json(&json!({ "query": query }))
            .send()
            .await
            .map_err(AppError::Request)?;

        let payload = response
            .error_for_status()
            .map_err(AppError::Request)?
            .json::<BatchQuestionsResponse>()
            .await
            .map_err(AppError::Request)?;

        if let Some(errors) = payload.errors {
            return Err(AppError::Upstream(format!(
                "LeetCode GraphQL returned errors: {}",
                errors
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }

        let data = payload.data.ok_or_else(|| {
            AppError::Upstream("LeetCode returned an empty batch response.".to_string())
        })?;

        let mut output = Vec::new();
        for (_, value) in data {
            let Some(question) = value else {
                continue;
            };

            output.push(ProblemRecord {
                id: question.id,
                title: question.title,
                slug: question.slug,
                difficulty: question.difficulty,
                acceptance: question.acceptance,
                paid_only: question.paid_only,
                position_level_tags: question
                    .position_level_tags
                    .into_iter()
                    .map(|tag| TagRecord {
                        name: tag.name,
                        slug: tag.slug,
                        category: TagCategory::PositionLevel,
                    })
                    .collect(),
                topic_tags: question
                    .topic_tags
                    .into_iter()
                    .map(|tag| TagRecord {
                        name: tag.name,
                        slug: tag.slug,
                        category: TagCategory::Topic,
                    })
                    .collect(),
            });
        }

        Ok(output)
    }
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

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: String,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    query: String,
    matched_tags: Vec<TagRecord>,
    available_hidden_tags: Vec<TagRecord>,
    results: Vec<SearchResult>,
    cache_ready: bool,
}

#[derive(Debug, Serialize)]
struct AvailableTagsResponse {
    tags: Vec<TagRecord>,
}

#[derive(Debug, Serialize)]
struct SuggestionResponse {
    suggestions: Vec<TagRecord>,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    id: String,
    title: String,
    slug: String,
    difficulty: String,
    acceptance: f64,
    paid_only: bool,
    url: String,
    tags: Vec<String>,
    matched_tags: Vec<TagRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProblemRecord {
    id: String,
    title: String,
    slug: String,
    difficulty: String,
    acceptance: f64,
    paid_only: bool,
    position_level_tags: Vec<TagRecord>,
    topic_tags: Vec<TagRecord>,
}

impl ProblemRecord {
    fn all_tags(&self) -> impl Iterator<Item = &TagRecord> {
        self.position_level_tags
            .iter()
            .chain(self.topic_tags.iter())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Copy, Hash)]
#[serde(rename_all = "snake_case")]
enum TagCategory {
    PositionLevel,
    Topic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct TagRecord {
    name: String,
    slug: String,
    category: TagCategory,
}

#[derive(Debug, Clone, Default)]
struct CacheMeta {
    saved_at_unix: u64,
    catalog_total: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    saved_at_unix: u64,
    catalog_total: usize,
    problems: Vec<ProblemRecord>,
}

#[derive(Debug, Deserialize)]
struct HiddenTagsResponse {
    data: Option<HiddenTagsData>,
}

#[derive(Debug, Deserialize)]
struct HiddenTagsData {
    #[serde(rename = "problemsetPositionLevelTags")]
    problemset_position_level_tags: Vec<SimpleTag>,
}

#[derive(Debug, Deserialize)]
struct SimpleTag {
    name: String,
    slug: String,
}

#[derive(Debug, Deserialize)]
struct CatalogResponse {
    stat_status_pairs: Vec<CatalogPair>,
}

#[derive(Debug, Deserialize)]
struct CatalogPair {
    stat: CatalogStat,
    difficulty: CatalogDifficulty,
    paid_only: bool,
}

#[derive(Debug, Deserialize)]
struct CatalogStat {
    #[serde(rename = "question__title")]
    title: String,
    #[serde(rename = "question__title_slug")]
    slug: String,
    #[serde(
        rename = "frontend_question_id",
        deserialize_with = "string_from_number_or_string"
    )]
    id: String,
    #[serde(rename = "total_acs")]
    total_accepted: f64,
    #[serde(rename = "total_submitted")]
    total_submitted: f64,
    #[serde(rename = "question__hide")]
    hidden: bool,
}

#[derive(Debug, Deserialize)]
struct CatalogDifficulty {
    level: u8,
}

#[derive(Debug)]
struct CatalogProblem {
    id: String,
    title: String,
    slug: String,
    difficulty: String,
    acceptance: f64,
    paid_only: bool,
}

impl From<CatalogPair> for CatalogProblem {
    fn from(value: CatalogPair) -> Self {
        let acceptance = if value.stat.total_submitted > 0.0 {
            (value.stat.total_accepted / value.stat.total_submitted) * 100.0
        } else {
            0.0
        };

        Self {
            id: value.stat.id,
            title: value.stat.title,
            slug: value.stat.slug,
            difficulty: match value.difficulty.level {
                1 => "Easy",
                2 => "Medium",
                3 => "Hard",
                _ => "Unknown",
            }
            .to_string(),
            acceptance,
            paid_only: value.paid_only,
        }
    }
}

#[derive(Debug, Deserialize)]
struct BatchQuestionsResponse {
    data: Option<HashMap<String, Option<QuestionBatchNode>>>,
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct QuestionBatchNode {
    title: String,
    #[serde(rename = "titleSlug")]
    slug: String,
    #[serde(
        rename = "questionFrontendId",
        deserialize_with = "string_from_number_or_string"
    )]
    id: String,
    difficulty: String,
    #[serde(rename = "acRate")]
    acceptance: f64,
    #[serde(rename = "isPaidOnly")]
    paid_only: bool,
    #[serde(rename = "positionLevelTags", default)]
    position_level_tags: Vec<SimpleTag>,
    #[serde(rename = "topicTags", default)]
    topic_tags: Vec<SimpleTag>,
}

#[derive(Debug, Error)]
enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("Failed to build HTTP client: {0}")]
    HttpClient(reqwest::Error),
    #[error("Request to LeetCode failed: {0}")]
    Request(reqwest::Error),
    #[error("Failed to access the local cache: {0}")]
    Io(std::io::Error),
    #[error("Failed to serialize the local cache: {0}")]
    Serialize(serde_json::Error),
    #[error("{0}")]
    Upstream(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::HttpClient(_)
            | AppError::Request(_)
            | AppError::Io(_)
            | AppError::Serialize(_)
            | AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
        };

        let body = Json(json!({
            "error": self.to_string(),
        }));

        (status, body).into_response()
    }
}

fn build_question_batch_query(slugs: &[String]) -> String {
    let mut query = String::from("query BatchQuestions {");

    for (index, slug) in slugs.iter().enumerate() {
        let _ = write!(
            query,
            r#"
            q{index}: question(titleSlug: {slug:?}) {{
              title
              titleSlug
              questionFrontendId
              difficulty
              acRate
              isPaidOnly
              positionLevelTags {{
                name
                slug
              }}
              topicTags {{
                name
                slug
              }}
            }}"#,
        );
    }

    query.push('}');
    query
}

fn numeric_id(id: &str) -> u32 {
    id.parse::<u32>().unwrap_or(u32::MAX)
}

fn normalize(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn tag_matches(tag: &TagRecord, query: &str) -> bool {
    normalize(&tag.name).contains(query) || normalize(&tag.slug).contains(query)
}

fn tag_sort_key(left: &TagRecord, right: &TagRecord) -> std::cmp::Ordering {
    category_order(left.category)
        .cmp(&category_order(right.category))
        .then_with(|| level_order(left).cmp(&level_order(right)))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.slug.cmp(&right.slug))
}

fn category_order(category: TagCategory) -> u8 {
    match category {
        TagCategory::PositionLevel => 0,
        TagCategory::Topic => 1,
    }
}

fn level_order(tag: &TagRecord) -> u8 {
    if tag.category != TagCategory::PositionLevel {
        return u8::MAX;
    }

    match tag.slug.as_str() {
        "junior" => 0,
        "mid-level" => 1,
        "senior" => 2,
        "staff" => 3,
        "senior-staff" => 4,
        "principal" => 5,
        _ => 100,
    }
}

fn tag_match_priority(tag: &TagRecord, query: &str) -> (u8, u8, String, String) {
    let slug = normalize(&tag.slug);
    let name = normalize(&tag.name);
    let starts_with = if slug.starts_with(query) || name.starts_with(query) {
        0
    } else {
        1
    };

    (starts_with, category_order(tag.category), name, slug)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn string_from_number_or_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(value) => Ok(value),
        Value::Number(value) => Ok(value.to_string()),
        other => Err(serde::de::Error::custom(format!(
            "expected string or number, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_builder_escapes_slugs() {
        let query = build_question_batch_query(&[
            "two-sum".to_string(),
            "sum-of-total-strength-of-wizards".to_string(),
        ]);

        assert!(query.contains(r#"q0: question(titleSlug: "two-sum")"#));
        assert!(query.contains(r#"q1: question(titleSlug: "sum-of-total-strength-of-wizards")"#));
        assert!(query.contains("positionLevelTags"));
    }

    #[test]
    fn tag_matching_checks_name_and_slug_case_insensitively() {
        let tag = TagRecord {
            name: "Senior Staff".to_string(),
            slug: "senior-staff".to_string(),
            category: TagCategory::PositionLevel,
        };

        assert!(tag_matches(&tag, "staff"));
        assert!(tag_matches(&tag, "senior"));
        assert!(tag_matches(&tag, "senior-staff"));
        assert!(!tag_matches(&tag, "frontend"));
    }

    #[test]
    fn position_levels_sort_in_real_order() {
        let mut tags = vec![
            TagRecord {
                name: "Principal".to_string(),
                slug: "principal".to_string(),
                category: TagCategory::PositionLevel,
            },
            TagRecord {
                name: "Junior".to_string(),
                slug: "junior".to_string(),
                category: TagCategory::PositionLevel,
            },
            TagRecord {
                name: "Senior Staff".to_string(),
                slug: "senior-staff".to_string(),
                category: TagCategory::PositionLevel,
            },
            TagRecord {
                name: "Staff".to_string(),
                slug: "staff".to_string(),
                category: TagCategory::PositionLevel,
            },
            TagRecord {
                name: "Mid Level".to_string(),
                slug: "mid-level".to_string(),
                category: TagCategory::PositionLevel,
            },
            TagRecord {
                name: "Senior".to_string(),
                slug: "senior".to_string(),
                category: TagCategory::PositionLevel,
            },
        ];

        tags.sort_by(tag_sort_key);

        assert_eq!(
            tags.into_iter().map(|tag| tag.slug).collect::<Vec<_>>(),
            vec![
                "junior",
                "mid-level",
                "senior",
                "staff",
                "senior-staff",
                "principal",
            ]
        );
    }
}
