use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use futures::{StreamExt, TryStreamExt, stream};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::json;
use tokio::{
    fs,
    sync::{Mutex, RwLock},
};
use tracing::{error, info};

use crate::{
    error::AppError,
    models::{
        BatchQuestionsResponse, CacheFile, CacheMeta, CatalogProblem, CatalogResponse,
        HiddenTagsResponse, ProblemRecord, SearchResponse, SearchResult, TagCategory, TagRecord,
    },
    util::{
        build_question_batch_query, normalize, numeric_id, tag_match_priority, tag_matches,
        tag_sort_key, unix_now,
    },
};

const LEETCODE_GRAPHQL_URL: &str = "https://leetcode.com/graphql/";
const LEETCODE_CATALOG_URL: &str = "https://leetcode.com/api/problems/algorithms/";
const CACHE_MAX_AGE: Duration = Duration::from_secs(60 * 60 * 24);
const GRAPHQL_BATCH_SIZE: usize = 20;
const GRAPHQL_CONCURRENCY: usize = 8;

#[derive(Clone)]
pub(crate) struct SearchService {
    client: reqwest::Client,
    cache_path: PathBuf,
    problems: Arc<RwLock<HashMap<String, ProblemRecord>>>,
    hidden_tags: Arc<RwLock<Option<Vec<TagRecord>>>>,
    refresh_lock: Arc<Mutex<()>>,
    cache_meta: Arc<RwLock<CacheMeta>>,
}

impl SearchService {
    pub(crate) async fn new(cache_path: PathBuf) -> Result<Self, AppError> {
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

    pub(crate) async fn search(&self, raw_query: &str) -> Result<SearchResponse, AppError> {
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

    pub(crate) async fn available_hidden_tags(&self) -> Result<Vec<TagRecord>, AppError> {
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

    pub(crate) async fn suggest_tags(&self, raw_query: &str) -> Result<Vec<TagRecord>, AppError> {
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
