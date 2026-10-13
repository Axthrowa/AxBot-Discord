//! Yapay zekânın güncel bilgiye erişmesi için web araması.
//!
//! - **SearXNG** (varsayılan): `docker-compose.dev.yml` içinde yerel çalışan açık kaynak metasearch
//!   motoru. Google, Bing, Wikipedia vb. sonuçlarını birleştirir; hesap/anahtar gerekmez.
//! - **Tavily**: `TAVILY_API_KEY` verilirse kullanılır.

use std::time::Duration;

use anyhow::{bail, Context as _};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::SearchBackend;

const TAVILY_URL: &str = "https://api.tavily.com/search";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESULTS: usize = 5;
const SNIPPET_CHARS: usize = 600;

#[derive(Debug, Deserialize)]
pub struct SearchArgs {
    pub query: String,
    /// "general" veya "news"
    #[serde(default)]
    pub topic: Option<String>,
}

impl SearchArgs {
    fn is_news(&self) -> bool {
        self.topic.as_deref() == Some("news")
    }
}

/// Servisten bağımsız tek bir arama sonucu.
struct Hit {
    title: String,
    url: String,
    content: String,
    date: Option<String>,
}

struct Results {
    answer: Option<String>,
    hits: Vec<Hit>,
}

/// Modelin `tools` listesine eklenen araç tanımı (OpenAI biçimi).
pub fn tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "web_search",
            "description": "İnternette güncel bilgi arar. Haberler, spor sonuçları, fiyatlar, hava durumu, \
                            yeni çıkan ürünler/sürümler, güncel olaylar ve emin olmadığın her gerçek bilgi için kullan.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Kullanıcının dilinde kısa, doğal arama sorgusu (örn. 'dolar kuru'). Tarih ekleme."
                    },
                    "topic": {
                        "type": "string",
                        "enum": ["general", "news"],
                        "description": "Sadece haber/son dakika olayları için 'news'. Fiyat, kur, skor, hava durumu ve diğer her şey için 'general'."
                    }
                },
                "required": ["query"]
            }
        }
    })
}

/// Arama yapar ve sonuçları modelin okuyacağı düz metin olarak döner.
pub async fn search(client: &reqwest::Client, backend: &SearchBackend, args: &SearchArgs) -> anyhow::Result<String> {
    let mut results = match backend {
        SearchBackend::Searxng { base_url } => searxng(client, base_url, args).await?,
        SearchBackend::Tavily { api_key } => tavily(client, api_key, args).await?,
    };
    // Haber kategorisi az motor içerir; boş dönerse aynı sorguyu genel aramada dene.
    if results.hits.is_empty() && args.is_news() {
        let general = SearchArgs {
            query: args.query.clone(),
            topic: None,
        };
        results = match backend {
            SearchBackend::Searxng { base_url } => searxng(client, base_url, &general).await?,
            SearchBackend::Tavily { api_key } => tavily(client, api_key, &general).await?,
        };
    }
    Ok(format_results(&args.query, &results))
}

// ---------------------------------------------------------------- SearXNG

#[derive(Deserialize)]
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
    /// Eski sürümlerde metin, yenilerinde `{ "answer": ... }` nesnesi.
    #[serde(default)]
    answers: Vec<Value>,
}

#[derive(Deserialize)]
struct SearxngResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

async fn searxng(client: &reqwest::Client, base_url: &str, args: &SearchArgs) -> anyhow::Result<Results> {
    let category = if args.is_news() { "news" } else { "general" };
    let response = client
        .get(format!("{base_url}/search"))
        .query(&[
            ("q", args.query.as_str()),
            ("format", "json"),
            ("categories", category),
            ("language", "auto"),
            ("safesearch", "1"),
        ])
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .context("SearXNG'ye ulaşılamadı (docker compose ile searxng servisi çalışıyor mu?)")?;

    let status = response.status();
    if !status.is_success() {
        let body: String = response.text().await.unwrap_or_default().chars().take(300).collect();
        bail!("SearXNG HTTP {status}: {body}");
    }
    let body: SearxngResponse = response.json().await.context("SearXNG yanıtı okunamadı")?;

    let answer = body.answers.iter().find_map(|a| match a {
        Value::String(s) => Some(s.clone()),
        other => other.get("answer").and_then(Value::as_str).map(str::to_owned),
    });
    let hits = body
        .results
        .into_iter()
        .filter(|r| !r.url.is_empty())
        .take(MAX_RESULTS)
        .map(|r| Hit {
            title: r.title,
            url: r.url,
            content: r.content,
            date: r.published_date.filter(|d| !d.is_empty()),
        })
        .collect();

    Ok(Results { answer, hits })
}

// ---------------------------------------------------------------- Tavily

#[derive(Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    published_date: Option<String>,
}

async fn tavily(client: &reqwest::Client, api_key: &str, args: &SearchArgs) -> anyhow::Result<Results> {
    let response = client
        .post(TAVILY_URL)
        .bearer_auth(api_key)
        .timeout(REQUEST_TIMEOUT)
        .json(&json!({
            "query": args.query,
            "topic": if args.is_news() { "news" } else { "general" },
            "search_depth": "basic",
            "max_results": MAX_RESULTS,
            "include_answer": "basic",
        }))
        .send()
        .await
        .context("Tavily'ye ulaşılamadı")?;

    let status = response.status();
    if !status.is_success() {
        let body: String = response.text().await.unwrap_or_default().chars().take(300).collect();
        bail!("Tavily HTTP {status}: {body}");
    }
    let body: TavilyResponse = response.json().await.context("Tavily yanıtı okunamadı")?;

    Ok(Results {
        answer: body.answer,
        hits: body
            .results
            .into_iter()
            .map(|r| Hit {
                title: r.title,
                url: r.url,
                content: r.content,
                date: r.published_date,
            })
            .collect(),
    })
}

// ---------------------------------------------------------------- Ortak

fn format_results(query: &str, results: &Results) -> String {
    let answer = results.answer.as_deref().filter(|a| !a.is_empty());
    if results.hits.is_empty() && answer.is_none() {
        return format!("\"{query}\" için sonuç bulunamadı.");
    }

    let mut out = format!("\"{query}\" arama sonuçları:\n");
    if let Some(answer) = answer {
        out.push_str(&format!("Özet: {answer}\n"));
    }
    for (i, hit) in results.hits.iter().enumerate() {
        let snippet: String = hit.content.chars().take(SNIPPET_CHARS).collect();
        let date = hit.date.as_deref().map(|d| format!(" ({d})")).unwrap_or_default();
        out.push_str(&format!("\n[{}] {}{date}\n{}\n{snippet}\n", i + 1, hit.title, hit.url));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_results_with_answer_and_sources() {
        let results = Results {
            answer: Some("Maç 2-1 bitti.".into()),
            hits: vec![Hit {
                title: "Maç raporu".into(),
                url: "https://example.com/mac".into(),
                content: "Galatasaray 2-1 kazandı.".into(),
                date: Some("2026-10-07".into()),
            }],
        };
        let text = format_results("gs maç", &results);
        assert!(text.contains("Özet: Maç 2-1 bitti."));
        assert!(text.contains("[1] Maç raporu (2026-10-07)"));
        assert!(text.contains("https://example.com/mac"));
    }

    #[test]
    fn empty_results_say_so() {
        let results = Results { answer: None, hits: vec![] };
        assert!(format_results("xyz", &results).contains("sonuç bulunamadı"));
    }

    #[test]
    fn parses_searxng_answers_in_both_formats() -> Result<(), serde_json::Error> {
        let body: SearxngResponse = serde_json::from_str(
            r#"{"results":[{"title":"t","url":"https://a.b","content":"c","publishedDate":null}],
                "answers":[{"answer":"42","url":null}]}"#,
        )?;
        assert_eq!(body.results.len(), 1);
        assert_eq!(body.answers[0].get("answer").and_then(Value::as_str), Some("42"));
        Ok(())
    }
}
