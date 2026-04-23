use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::util::string_from_number_or_string;

#[derive(Debug, Deserialize)]
pub(crate) struct SearchParams {
    pub(crate) q: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SearchResponse {
    pub(crate) query: String,
    pub(crate) matched_tags: Vec<TagRecord>,
    pub(crate) available_hidden_tags: Vec<TagRecord>,
    pub(crate) results: Vec<SearchResult>,
    pub(crate) cache_ready: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AvailableTagsResponse {
    pub(crate) tags: Vec<TagRecord>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SuggestionResponse {
    pub(crate) suggestions: Vec<TagRecord>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SearchResult {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) slug: String,
    pub(crate) difficulty: String,
    pub(crate) acceptance: f64,
    pub(crate) paid_only: bool,
    pub(crate) url: String,
    pub(crate) tags: Vec<String>,
    pub(crate) matched_tags: Vec<TagRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProblemRecord {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) slug: String,
    pub(crate) difficulty: String,
    pub(crate) acceptance: f64,
    pub(crate) paid_only: bool,
    pub(crate) position_level_tags: Vec<TagRecord>,
    pub(crate) topic_tags: Vec<TagRecord>,
}

impl ProblemRecord {
    pub(crate) fn all_tags(&self) -> impl Iterator<Item = &TagRecord> {
        self.position_level_tags
            .iter()
            .chain(self.topic_tags.iter())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Copy, Hash)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TagCategory {
    PositionLevel,
    Topic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TagRecord {
    pub(crate) name: String,
    pub(crate) slug: String,
    pub(crate) category: TagCategory,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CacheMeta {
    pub(crate) saved_at_unix: u64,
    pub(crate) catalog_total: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CacheFile {
    pub(crate) saved_at_unix: u64,
    pub(crate) catalog_total: usize,
    pub(crate) problems: Vec<ProblemRecord>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HiddenTagsResponse {
    pub(crate) data: Option<HiddenTagsData>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HiddenTagsData {
    #[serde(rename = "problemsetPositionLevelTags")]
    pub(crate) problemset_position_level_tags: Vec<SimpleTag>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SimpleTag {
    pub(crate) name: String,
    pub(crate) slug: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogResponse {
    pub(crate) stat_status_pairs: Vec<CatalogPair>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogPair {
    pub(crate) stat: CatalogStat,
    pub(crate) difficulty: CatalogDifficulty,
    pub(crate) paid_only: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogStat {
    #[serde(rename = "question__title")]
    pub(crate) title: String,
    #[serde(rename = "question__title_slug")]
    pub(crate) slug: String,
    #[serde(
        rename = "frontend_question_id",
        deserialize_with = "string_from_number_or_string"
    )]
    pub(crate) id: String,
    #[serde(rename = "total_acs")]
    pub(crate) total_accepted: f64,
    #[serde(rename = "total_submitted")]
    pub(crate) total_submitted: f64,
    #[serde(rename = "question__hide")]
    pub(crate) hidden: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CatalogDifficulty {
    pub(crate) level: u8,
}

#[derive(Debug)]
pub(crate) struct CatalogProblem {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) slug: String,
    pub(crate) difficulty: String,
    pub(crate) acceptance: f64,
    pub(crate) paid_only: bool,
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
pub(crate) struct BatchQuestionsResponse {
    pub(crate) data: Option<HashMap<String, Option<QuestionBatchNode>>>,
    pub(crate) errors: Option<Vec<GraphqlError>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GraphqlError {
    pub(crate) message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QuestionBatchNode {
    pub(crate) title: String,
    #[serde(rename = "titleSlug")]
    pub(crate) slug: String,
    #[serde(
        rename = "questionFrontendId",
        deserialize_with = "string_from_number_or_string"
    )]
    pub(crate) id: String,
    pub(crate) difficulty: String,
    #[serde(rename = "acRate")]
    pub(crate) acceptance: f64,
    #[serde(rename = "isPaidOnly")]
    pub(crate) paid_only: bool,
    #[serde(rename = "positionLevelTags", default)]
    pub(crate) position_level_tags: Vec<SimpleTag>,
    #[serde(rename = "topicTags", default)]
    pub(crate) topic_tags: Vec<SimpleTag>,
}
