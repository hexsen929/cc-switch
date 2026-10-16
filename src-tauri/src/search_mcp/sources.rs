use super::{network, SearchMcpConfig, SearchSource};
use crate::error::AppError;
use futures::{stream, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Pool {
    turns: HashMap<String, usize>,
    resting: HashMap<String, Instant>,
}
static POOL: LazyLock<Mutex<Pool>> = LazyLock::new(|| Mutex::new(Pool::default()));
type CachedQuota = (Instant, Value);
static QUOTAS: LazyLock<Mutex<HashMap<String, CachedQuota>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn key_id(engine: &str, key: &str) -> String {
    format!("{engine}:{:x}", Sha256::digest(key.as_bytes()))
}

fn key_order(source: &SearchSource, backoff: bool) -> Vec<usize> {
    let count = source.keys.len().max(1);
    let Ok(mut pool) = POOL.lock() else {
        return (0..count).collect();
    };
    let turn = pool.turns.entry(source.engine.clone()).or_default();
    let start = if source.ordered_keys {
        0
    } else {
        *turn % count
    };
    *turn = turn.wrapping_add(1);
    let now = Instant::now();
    pool.resting.retain(|_, until| *until > now);
    let mut keys: Vec<_> = (0..count).map(|offset| (start + offset) % count).collect();
    if backoff {
        // Healthy keys first, then the resting key whose cooldown ends first.
        keys.sort_by_key(|index| {
            pool.resting
                .get(&key_id(
                    &source.engine,
                    source
                        .keys
                        .get(*index)
                        .map(String::as_str)
                        .unwrap_or_default(),
                ))
                .copied()
        });
    }
    keys
}

fn rest_key(engine: &str, key: &str, status: u16, retry_after: Option<u64>) {
    let seconds = match status {
        401 | 403 => 600,
        402 | 432 | 433 => 3600,
        _ => retry_after.unwrap_or(60).clamp(30, 600),
    };
    if let Ok(mut pool) = POOL.lock() {
        let until = Instant::now() + Duration::from_secs(seconds);
        pool.resting
            .entry(key_id(engine, key))
            .and_modify(|old| *old = (*old).max(until))
            .or_insert(until);
    }
}

fn base<'a>(source: &'a SearchSource, key: &str) -> &'a str {
    if !source.base_url.is_empty() {
        return &source.base_url;
    }
    match source.engine.as_str() {
        "tavily" => "https://api.tavily.com",
        "brave" => "https://api.search.brave.com",
        "exa" => "https://api.exa.ai",
        "firecrawl" => "https://api.firecrawl.dev",
        "you" if key.is_empty() => "https://api.you.com",
        "you" => "https://ydc-index.io",
        _ => "",
    }
}

fn request(
    source: &SearchSource,
    key: &str,
    query: &str,
    count: usize,
) -> Result<reqwest::RequestBuilder, AppError> {
    let at = base(source, key).trim_end_matches('/');
    let client = network::client_for(at)?;
    let mut request = match source.engine.as_str() {
        "tavily" => client.post(format!("{at}/search")).json(&json!({"query":query,"max_results":count,"search_depth":"basic"})).bearer_auth(key),
        "brave" => client.get(format!("{at}/res/v1/web/search")).query(&[("q", query), ("count", &count.to_string())]).header("x-subscription-token", key),
        "exa" => client.post(format!("{at}/search")).json(&json!({"query":query,"numResults":count,"contents":{"text":{"maxCharacters":3000}}})).header("x-api-key", key),
        "firecrawl" => client.post(format!("{at}/v2/search")).json(&json!({"query":query,"limit":count})).bearer_auth(key),
        "you" if key.is_empty() => client.get(format!("{at}/v1/agents/search")).query(&[("query", query), ("count", &count.to_string())]),
        "you" => client.post(format!("{at}/v1/search")).json(&json!({"query":query,"count":count})).header("x-api-key", key),
        "searxng" => {
            let request = client.get(format!("{at}/search")).query(&[("q", query), ("format", "json")]);
            if key.is_empty() { request } else { request.bearer_auth(key) }
        }
        _ => return Err(AppError::InvalidInput("Unknown search engine".into())),
    };
    request = request.header("accept", "application/json");
    Ok(request)
}

fn results(engine: &str, response: &Value, count: usize) -> Result<String, AppError> {
    let you_rows = if engine == "you" {
        let web = response["results"]["web"].as_array();
        let news = response["results"]["news"].as_array();
        if web.is_some() || news.is_some() {
            Some(
                web.into_iter()
                    .chain(news)
                    .flat_map(|items| items.iter().cloned())
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        }
    } else {
        None
    };
    let list = match engine {
        "brave" => response["web"]["results"].as_array(),
        "firecrawl" => response["data"]["web"]
            .as_array()
            .or_else(|| response["data"].as_array()),
        "you" => you_rows.as_ref().or_else(|| response["results"].as_array()),
        _ => response["results"].as_array(),
    }
    .ok_or_else(|| AppError::Message(format!("{engine}: unrecognized search response")))?;
    let mut found = Vec::new();
    for item in list.iter().take(count) {
        let Some(url) = item["url"]
            .as_str()
            .filter(|url| network::parse_url(url).is_ok())
        else {
            continue;
        };
        let title = item["title"].as_str().unwrap_or(url);
        let snippet = ["content", "text", "description", "markdown"]
            .into_iter()
            .find_map(|field| item[field].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                item["snippets"]
                    .as_array()
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default()
            });
        found.push(json!({"title":title,"url":url,"snippet":snippet}));
    }
    serde_json::to_string_pretty(&json!({"source":engine,"results":found}))
        .map_err(|_| AppError::Message("Cannot format search results".into()))
}

pub(super) async fn search(
    config: &SearchMcpConfig,
    query: &str,
    count: usize,
) -> Result<String, AppError> {
    let mut failures = Vec::new();
    let source_count = config
        .sources
        .iter()
        .filter(|source| source.enabled)
        .count()
        .max(1);
    // Every enabled source gets a turn even if the first source has many dead
    // keys. Keep a small margin for parsing/formatting under the global timeout.
    let source_budget =
        Duration::from_millis(config.timeout_seconds.saturating_mul(950) / source_count as u64);
    for source in config.sources.iter().filter(|source| source.enabled) {
        let until = Instant::now() + source_budget;
        for index in key_order(source, config.key_backoff) {
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let key = source
                .keys
                .get(index)
                .map(String::as_str)
                .unwrap_or_default();
            let timeout = remaining
                .min((source_budget / source.keys.len().max(1) as u32).max(Duration::from_secs(1)))
                .min(Duration::from_secs(10));
            let response = match request(source, key, query, count)?
                .timeout(timeout)
                .send()
                .await
            {
                Ok(response) => response,
                Err(_) => {
                    failures.push(format!("{}: request failed", source.engine));
                    continue;
                }
            };
            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                if config.key_backoff && [401, 402, 403, 429, 432, 433].contains(&status) {
                    let retry_after = response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse().ok());
                    rest_key(&source.engine, key, status, retry_after);
                }
                failures.push(format!("{}: HTTP {status}", source.engine));
                continue;
            }
            let body = match network::limited_bytes(response, 2 * 1024 * 1024).await {
                Ok(body) => body,
                Err(_) => {
                    failures.push(format!(
                        "{}: response too large or unreadable",
                        source.engine
                    ));
                    continue;
                }
            };
            let parsed = serde_json::from_slice::<Value>(&body)
                .map_err(|_| AppError::Message("Invalid search JSON".into()));
            match parsed.and_then(|value| results(&source.engine, &value, count)) {
                Ok(text) => {
                    if let Ok(mut pool) = POOL.lock() {
                        pool.resting.remove(&key_id(&source.engine, key));
                    }
                    return Ok(text);
                }
                Err(_) => failures.push(format!("{}: invalid response", source.engine)),
            }
        }
    }
    failures.sort();
    failures.dedup();
    Err(AppError::Message(if failures.is_empty() {
        "No enabled search source; configure Web Search in Settings".into()
    } else {
        format!("Search sources failed: {}", failures.join("; "))
    }))
}

fn tavily_quota(value: &Value) -> Option<(u64, Option<u64>)> {
    let key = value.get("key")?;
    let limit = key.get("limit")?;
    if let Some(limit) = limit.as_u64() {
        return Some((key["usage"].as_u64()?, Some(limit)));
    }
    if !limit.is_null() {
        return None;
    }
    let account = value.get("account")?;
    match account.get("plan_limit")? {
        Value::Null => Some((key["usage"].as_u64()?, None)),
        limit => Some((account["plan_usage"].as_u64()?, Some(limit.as_u64()?))),
    }
}

async fn quota(source: SearchSource, index: usize, key: String, fresh: bool) -> Value {
    let cache_id = key_id(&format!("{}:{}", source.engine, source.base_url), &key);
    if !fresh {
        if let Some(mut row) = QUOTAS.lock().ok().and_then(|cache| {
            cache
                .get(&cache_id)
                .filter(|(at, _)| at.elapsed() < Duration::from_secs(120))
                .map(|(_, row)| row.clone())
        }) {
            row["keyNumber"] = json!(index + 1);
            return row;
        }
    }
    let mut row = json!({"engine":source.engine,"keyNumber":index+1,"known":false});
    if source.engine != "tavily" {
        row["error"] = json!("This source does not expose supported quota data");
        return row;
    }
    let response = match network::client_for(base(&source, &key)) {
        Ok(client) => {
            client
                .get(format!("{}/usage", base(&source, &key)))
                .bearer_auth(&key)
                .timeout(Duration::from_secs(3))
                .send()
                .await
        }
        Err(_) => {
            row["error"] = json!("Cannot initialize quota query");
            return row;
        }
    };
    let parsed = match response {
        Ok(response) if response.status().is_success() => {
            network::limited_bytes(response, 64 * 1024)
                .await
                .ok()
                .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
        }
        _ => None,
    };
    if let Some(value) = parsed {
        if let Some((used, limit)) = tavily_quota(&value) {
            row["known"] = json!(true);
            row["used"] = json!(used);
            row["limit"] = json!(limit);
            let paygo_left = value["account"]["paygo_limit"]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(value["account"]["paygo_usage"].as_u64().unwrap_or(0));
            row["paygoLeft"] = json!(paygo_left);
            if limit.is_some_and(|limit| used >= limit) && paygo_left == 0 {
                rest_key("tavily", &key, 402, None);
            } else if let Ok(mut pool) = POOL.lock() {
                pool.resting.remove(&key_id("tavily", &key));
            }
        }
    }
    if row["known"] != true {
        row["error"] = json!("Quota could not be read; remaining credit is unknown");
    }
    if let Ok(mut cache) = QUOTAS.lock() {
        cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(120));
        cache.insert(cache_id, (Instant::now(), row.clone()));
    }
    row
}

pub(super) async fn quotas(config: &SearchMcpConfig, fresh: bool) -> Result<Vec<Value>, AppError> {
    let mut rows = Vec::new();
    for source in &config.sources {
        for (index, key) in source.keys.iter().enumerate() {
            rows.push((source.clone(), index, key.clone()));
        }
    }
    Ok(stream::iter(rows)
        .map(|(source, index, key)| quota(source, index, key, fresh))
        .buffered(4)
        .collect()
        .await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_six_search_responses_are_normalized_without_credentials() {
        for (engine, value) in [
            (
                "tavily",
                json!({"results":[{"title":"A","url":"https://example.com","content":"text"}]}),
            ),
            (
                "brave",
                json!({"web":{"results":[{"title":"A","url":"https://example.com","description":"text"}]}}),
            ),
            (
                "exa",
                json!({"results":[{"title":"A","url":"https://example.com","text":"text"}]}),
            ),
            (
                "firecrawl",
                json!({"data":{"web":[{"title":"A","url":"https://example.com","description":"text"}]}}),
            ),
            (
                "you",
                json!({"results":{"web":[{"title":"A","url":"https://example.com","snippets":["text"]}]}}),
            ),
            (
                "you",
                json!({"results":{"news":[{"title":"A","url":"https://example.com","snippets":["text"]}]}}),
            ),
            (
                "searxng",
                json!({"results":[{"title":"A","url":"https://example.com","content":"text"}]}),
            ),
        ] {
            let output = results(engine, &value, 6).unwrap();
            assert!(output.contains("text"));
            assert!(output.contains("https://example.com"));
            assert!(!output.contains("api_key"));
        }
    }

    #[test]
    fn keys_rotate_and_ordered_keys_start_with_first() {
        let source = SearchSource {
            engine: "test-order".into(),
            keys: vec!["one".into(), "two".into()],
            ..Default::default()
        };
        assert_ne!(key_order(&source, false), key_order(&source, false));
        let ordered = SearchSource {
            ordered_keys: true,
            ..source
        };
        assert_eq!(key_order(&ordered, false), vec![0, 1]);
        rest_key("test-order", "one", 429, Some(60));
        assert_eq!(key_order(&ordered, true), vec![1, 0]);
    }

    #[test]
    fn quota_unknown_is_never_reported_as_zero() {
        assert_eq!(tavily_quota(&json!({})), None);
        assert_eq!(
            tavily_quota(&json!({"key":{"usage":7,"limit":10}})),
            Some((7, Some(10)))
        );
        assert_eq!(
            tavily_quota(
                &json!({"key":{"usage":7,"limit":null},"account":{"plan_usage":9,"plan_limit":100}})
            ),
            Some((9, Some(100)))
        );
        assert_eq!(
            tavily_quota(&json!({"key":{"usage":7,"limit":null},"account":{"plan_limit":null}})),
            Some((7, None))
        );
    }
}
