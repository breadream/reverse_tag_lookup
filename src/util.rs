use std::{
    fmt::Write as _,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, de::Deserializer};
use serde_json::Value;

use crate::models::{TagCategory, TagRecord};

pub(crate) fn build_question_batch_query(slugs: &[String]) -> String {
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

pub(crate) fn numeric_id(id: &str) -> u32 {
    id.parse::<u32>().unwrap_or(u32::MAX)
}

pub(crate) fn normalize(value: &str) -> String {
    value.to_ascii_lowercase()
}

pub(crate) fn tag_matches(tag: &TagRecord, query: &str) -> bool {
    normalize(&tag.name).contains(query) || normalize(&tag.slug).contains(query)
}

pub(crate) fn tag_sort_key(left: &TagRecord, right: &TagRecord) -> std::cmp::Ordering {
    category_order(left.category)
        .cmp(&category_order(right.category))
        .then_with(|| level_order(left).cmp(&level_order(right)))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.slug.cmp(&right.slug))
}

pub(crate) fn category_order(category: TagCategory) -> u8 {
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

pub(crate) fn tag_match_priority(tag: &TagRecord, query: &str) -> (u8, u8, String, String) {
    let slug = normalize(&tag.slug);
    let name = normalize(&tag.name);
    let starts_with = if slug.starts_with(query) || name.starts_with(query) {
        0
    } else {
        1
    };

    (starts_with, category_order(tag.category), name, slug)
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn string_from_number_or_string<'de, D>(deserializer: D) -> Result<String, D::Error>
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
