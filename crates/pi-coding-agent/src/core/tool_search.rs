//! Tool discovery: BM25 ranking over tool metadata.
//!
//! Port of `packages/coding-agent/src/extensions/tool-search/tool.ts`. The
//! ranker is shared by `tool_search`; the tool registration lives in
//! `agent_session` because it needs the live active set.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// A tool as the ranker sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchDocument {
    /// Tool name.
    pub name: String,
    /// Description, schema descriptions, and property names.
    pub text: String,
    /// Name, description, and instructions of the tool's namespace.
    pub namespace_text: Option<String>,
}

/// One ranked tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSearchMatch {
    /// Tool name.
    pub name: String,
    /// BM25 score.
    pub score: f64,
}

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of",
    "on", "or", "that", "the", "this", "to", "with",
];

/// Naive singular form, so `issues` matches `issue`.
fn stem(term: &str) -> String {
    let chars: Vec<char> = term.chars().collect();
    let len = chars.len();
    if len > 4 && term.ends_with("ies") {
        return format!("{}y", &term[..term.len() - 3]);
    }
    if len > 4
        && (term.ends_with("ches")
            || term.ends_with("shes")
            || term.ends_with("sses")
            || term.ends_with("xes")
            || term.ends_with("zes"))
    {
        return term[..term.len() - 2].to_string();
    }
    if len > 3 && term.ends_with('s') && !term.ends_with("ss") {
        return term[..term.len() - 1].to_string();
    }
    term.to_string()
}

fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word)
}

fn split_camel_case(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 4);
    for (index, current) in chars.iter().enumerate() {
        if index > 0 {
            let previous = chars[index - 1];
            let next = chars.get(index + 1).copied();
            let boundary = (previous.is_ascii_lowercase() || previous.is_ascii_digit())
                && current.is_ascii_uppercase()
                || (previous.is_ascii_uppercase()
                    && current.is_ascii_uppercase()
                    && next.is_some_and(|next| next.is_ascii_lowercase()));
            if boundary {
                out.push(' ');
            }
        }
        out.push(*current);
    }
    out
}

/// Unicode word segmentation with identifier boundaries and English stop
/// words/singular forms.
pub fn tokenize(text: &str) -> Vec<String> {
    let normalized: String = text.nfc().collect();
    let normalized = split_camel_case(&normalized).to_lowercase();
    let cleaned: String = normalized
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect();
    cleaned
        .split_word_bounds()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .filter(|word| !is_stop_word(word))
        .map(|word| {
            if word.chars().all(|character| character.is_ascii_alphanumeric()) {
                stem(word)
            } else {
                word.to_string()
            }
        })
        .collect()
}

fn schema_text(schema: &Value, parts: &mut Vec<String>) {
    let Some(object) = schema.as_object() else {
        return;
    };
    if let Some(description) = object.get("description").and_then(Value::as_str) {
        parts.push(description.to_string());
    }
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            parts.push(name.clone());
            schema_text(property, parts);
        }
    }
    if let Some(items) = object.get("items") {
        schema_text(items, parts);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = object.get(key).and_then(Value::as_array) {
            for variant in variants {
                schema_text(variant, parts);
            }
        }
    }
}

/// Build a rankable document from tool metadata.
pub fn create_tool_search_document(
    name: &str,
    description: &str,
    parameters: Option<&Value>,
    namespace: Option<(&str, Option<&str>, Option<&str>)>,
) -> ToolSearchDocument {
    let mut parts = vec![description.to_string()];
    if let Some(parameters) = parameters {
        schema_text(parameters, &mut parts);
    }
    let namespace_text = namespace.map(|(name, description, instructions)| {
        [
            name.to_string(),
            description.unwrap_or_default().to_string(),
            instructions.unwrap_or_default().to_string(),
        ]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
    });
    ToolSearchDocument {
        name: name.to_string(),
        text: parts
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        namespace_text,
    }
}

fn normalize_name(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '$' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn is_exact_name_query(query: &str, name: &str) -> bool {
    let normalized = normalize_name(name);
    normalized == query || normalized.ends_with(&format!("__{query}"))
}

struct IndexedDocument {
    name: String,
    name_terms: HashSet<String>,
    local_name_terms: Vec<String>,
    text_counts: HashMap<String, usize>,
    text_length: usize,
    namespace_terms: HashSet<String>,
}

fn index_document(document: &ToolSearchDocument) -> IndexedDocument {
    let mut text_counts: HashMap<String, usize> = HashMap::new();
    for term in tokenize(&document.text) {
        *text_counts.entry(term).or_insert(0) += 1;
    }
    let segments: Vec<&str> = document.name.split("__").collect();
    let local = segments
        .last()
        .map(|segment| tokenize(segment))
        .unwrap_or_default();
    let local_name_terms = local
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    IndexedDocument {
        name: document.name.clone(),
        name_terms: tokenize(&document.name).into_iter().collect(),
        local_name_terms,
        text_length: text_counts.values().sum(),
        text_counts,
        namespace_terms: tokenize(document.namespace_text.as_deref().unwrap_or_default())
            .into_iter()
            .collect(),
    }
}

const NAME_WEIGHT: f64 = 3.0;
const NAMESPACE_WEIGHT: f64 = 0.5;

/// Okapi BM25 over the description, with name and namespace as extra fields.
pub struct Bm25Ranker {
    k1: f64,
    b: f64,
}

impl Default for Bm25Ranker {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

impl Bm25Ranker {
    /// Create a ranker with optional `k1`/`b`.
    pub fn new(k1: Option<f64>, b: Option<f64>) -> Self {
        Self {
            k1: k1.unwrap_or(1.2),
            b: b.unwrap_or(0.75),
        }
    }

    /// Rank `documents` for `query`, returning at most `limit` matches.
    pub fn rank(
        &self,
        query: &str,
        documents: &[ToolSearchDocument],
        limit: usize,
    ) -> Vec<ToolSearchMatch> {
        let query_terms: HashSet<String> = tokenize(query).into_iter().collect();
        if query_terms.is_empty() || documents.is_empty() || limit == 0 {
            return Vec::new();
        }
        let indexed: Vec<IndexedDocument> = documents.iter().map(index_document).collect();
        let average_length = if indexed.is_empty() {
            1.0
        } else {
            let total: usize = indexed.iter().map(|document| document.text_length).sum();
            (total as f64 / indexed.len() as f64).max(1.0)
        };
        let idf: HashMap<String, f64> = query_terms
            .iter()
            .map(|term| {
                let frequency = indexed
                    .iter()
                    .filter(|document| {
                        document.name_terms.contains(term)
                            || document.text_counts.contains_key(term)
                            || document.namespace_terms.contains(term)
                    })
                    .count() as f64;
                let value = (1.0
                    + (indexed.len() as f64 - frequency + 0.5) / (frequency + 0.5))
                    .ln();
                (term.clone(), value)
            })
            .collect();
        let full_name_bonus: f64 = idf.values().sum();
        let exact_query = normalize_name(query);
        let mut matches: Vec<(ToolSearchMatch, usize, bool)> = Vec::new();
        for document in &indexed {
            let norm = self.k1
                * (1.0 - self.b + (self.b * document.text_length as f64) / average_length);
            let mut score = 0.0;
            let mut covered = 0usize;
            for term in &query_terms {
                let count = *document.text_counts.get(term).unwrap_or(&0) as f64;
                let weight = if document.name_terms.contains(term) {
                    NAME_WEIGHT
                } else {
                    0.0
                } + if document.namespace_terms.contains(term) {
                    NAMESPACE_WEIGHT
                } else {
                    0.0
                } + (count * (self.k1 + 1.0)) / (count + norm);
                if weight > 0.0 {
                    covered += 1;
                }
                score += idf.get(term).copied().unwrap_or(0.0) * weight;
            }
            if score == 0.0 {
                continue;
            }
            if !document.local_name_terms.is_empty()
                && document
                    .local_name_terms
                    .iter()
                    .all(|term| query_terms.contains(term))
            {
                score += full_name_bonus;
            }
            matches.push((
                ToolSearchMatch {
                    name: document.name.clone(),
                    score,
                },
                covered,
                is_exact_name_query(&exact_query, &document.name),
            ));
        }
        matches.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| {
                    b.0.score
                        .partial_cmp(&a.0.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        matches
            .into_iter()
            .take(limit)
            .map(|(matched, _, _)| matched)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn tokenizes_camel_case_and_stop_words() {
        assert_eq!(
            tokenize("readFile and the ISSUES"),
            vec!["read", "file", "issue"]
        );
    }

    #[test]
    fn schema_text_includes_property_names_and_descriptions() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path"},
                "limit": {"type": "number"}
            }
        });
        let mut parts = Vec::new();
        schema_text(&schema, &mut parts);
        assert!(parts.contains(&"File path".to_string()));
        assert!(parts.contains(&"path".to_string()));
        assert!(parts.contains(&"limit".to_string()));
    }

    #[test]
    fn ranks_exact_name_first() {
        let docs = vec![
            ToolSearchDocument {
                name: "mcp__app__thread_read".to_string(),
                text: "Read a thread".to_string(),
                namespace_text: None,
            },
            ToolSearchDocument {
                name: "mcp__app__thread_list".to_string(),
                text: "List threads".to_string(),
                namespace_text: None,
            },
        ];
        let matches = Bm25Ranker::default().rank("thread_read", &docs, 8);
        assert_eq!(matches[0].name, "mcp__app__thread_read");
    }

    #[test]
    fn only_returns_positive_matches() {
        let docs = vec![ToolSearchDocument {
            name: "mcp__docs__search".to_string(),
            text: "Search documentation".to_string(),
            namespace_text: None,
        }];
        assert!(Bm25Ranker::default().rank("weather", &docs, 8).is_empty());
    }
}
