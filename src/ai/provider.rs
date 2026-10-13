//! Yapay zekâ sağlayıcılarının HTTP istemcileri: NVIDIA NIM (OpenAI uyumlu) ve Google Gemini.

use std::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::search::{self, SearchArgs};
use crate::config::{AiConfig, AiProvider, SearchBackend};

const NIM_URL: &str = "https://integrate.api.nvidia.com/v1/chat/completions";
const GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Akıl yürüten modellerde düşünme de bu sınıra dahildir; düşük tutulursa cevap boş kalır.
const MAX_OUTPUT_TOKENS: u32 = 1500;
/// Düşük sıcaklık: daha tutarlı dil bilgisi, daha az uydurma kelime.
const TEMPERATURE: f32 = 0.4;
/// Akıl yürüten modellerde (örn. GLM) düşünmeyi kısa tutar; desteklemeyen modeller yok sayar.
const REASONING_EFFORT: &str = "low";
/// Soru başına en fazla bu kadar arama turu; sonra model eldeki bilgiyle cevaplamak zorunda.
const MAX_SEARCH_ROUNDS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// Cevap yazılırken kullanıcıya gösterilecek ara metin (şimdiye kadar yazılan kısım veya
/// "aranıyor" durumu). Model cevabı parça parça gönderir; bekleyen kişi ilk kelimeleri
/// cevabın tamamı bitmeden görür.
pub type Progress = tokio::sync::watch::Sender<String>;

/// Sohbet geçmişi + yeni mesajı gönderip modelin yanıtını döner.
pub async fn complete(
    client: &reqwest::Client,
    config: &AiConfig,
    system: &str,
    messages: &[ChatMessage],
    progress: Option<&Progress>,
) -> anyhow::Result<String> {
    let messages = merge_consecutive(messages);
    match ask(client, config, system, &messages, progress).await {
        Ok(text) => Ok(text),
        Err(error) => {
            // Asıl sağlayıcıya ulaşılamadıysa (ağ / servis hatası) yedek sağlayıcı denenir.
            let Some(fallback) = config.fallback.as_deref() else {
                return Err(error);
            };
            tracing::warn!(provider = config.provider.name(), fallback = fallback.provider.name(), error = ?error, "AI sağlayıcısı yanıt vermedi; yedeğe geçiliyor");
            ask(client, fallback, system, &messages, progress).await
        }
    }
}

async fn ask(client: &reqwest::Client, config: &AiConfig, system: &str, messages: &[ChatMessage], progress: Option<&Progress>) -> anyhow::Result<String> {
    let raw = match config.provider {
        AiProvider::Nim => nim(client, config, system, messages, progress).await?,
        AiProvider::Gemini => gemini(client, config, system, messages).await?,
    };
    let text = strip_reasoning(&raw).trim().to_owned();
    if text.is_empty() {
        bail!("model boş yanıt döndü");
    }
    Ok(text)
}

/// OpenAI uyumlu araç döngüsü: model `web_search` çağırırsa arama yapılır ve sonuç modele
/// geri verilir. Arama hakkı bitince son cevap **araçsız** bir istekle alınır: toplanan
/// sonuçlar düz metin olarak verilir. (NIM modelleri `tool_choice: "none"` ayarını her zaman
/// dinlemiyor; araçsız istek ise her modelde metin cevap üretir.)
async fn nim(
    client: &reqwest::Client,
    config: &AiConfig,
    system: &str,
    messages: &[ChatMessage],
    progress: Option<&Progress>,
) -> anyhow::Result<String> {
    // Bazı açık modeller tek bir sistem mesajı kabul eder; o yüzden en başta tek mesaj.
    let mut base = vec![json!({ "role": "system", "content": system })];
    base.extend(messages.iter().map(|m| {
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        json!({ "role": role, "content": m.content })
    }));

    let mut findings: Vec<String> = Vec::new();
    if let Some(backend) = config.search.as_ref() {
        let mut conversation = base.clone();
        for round in 0..MAX_SEARCH_ROUNDS {
            let reply = nim_request(client, config, &conversation, true, progress).await?;
            if reply.tool_calls.is_empty() {
                // Model aramaya gerek görmedi (veya yeterli bilgiyi topladı).
                if !reply.content.trim().is_empty() {
                    return Ok(reply.content);
                }
                tracing::debug!(round, "Model boş cevap verdi, son tura geçiliyor");
                break;
            }
            let calls = reply.tool_calls;
            tracing::debug!(round, calls = calls.len(), "Model web araması istedi");

            conversation.push(json!({
                "role": "assistant",
                "content": reply.content,
                "tool_calls": calls,
            }));
            for call in &calls {
                let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
                let result = run_tool_call(client, backend, call, progress).await;
                tracing::debug!(
                    chars = result.chars().count(),
                    preview = %result.chars().take(300).collect::<String>(),
                    "Arama sonucu modele verildi"
                );
                findings.push(result.clone());
                conversation.push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
            }
        }
    }

    // Son tur: araç yok, arama sonuçları düz metin.
    let mut final_messages = base;
    if !findings.is_empty() {
        final_messages.push(json!({
            "role": "user",
            "content": format!(
                "[Senin yaptığın web aramalarının sonuçları]\n{}\n\n\
                 Yeni arama yapamazsın. Yukarıdaki sonuçlara dayanarak son kullanıcı mesajını cevapla; \
                 sonuçlarda cevap yoksa bulamadığını kısaca söyle. Kullandığın kaynağın bağlantısını ekle.",
                findings.join("\n---\n")
            ),
        }));
    }
    let reply = nim_request(client, config, &final_messages, false, progress).await?;
    if reply.content.trim().is_empty() {
        bail!("NVIDIA NIM boş içerik döndü");
    }
    Ok(reply.content)
}

/// Tek bir sohbet isteği. Yanıt akış (SSE) olarak okunur; içerik geldikçe `progress`'e yazılır.
async fn nim_request(
    client: &reqwest::Client,
    config: &AiConfig,
    messages: &[Value],
    with_tools: bool,
    progress: Option<&Progress>,
) -> anyhow::Result<NimReply> {
    let mut request = json!({
        "model": config.model,
        "messages": messages,
        "max_tokens": MAX_OUTPUT_TOKENS,
        "temperature": TEMPERATURE,
        "reasoning_effort": REASONING_EFFORT,
        "stream": true,
    });
    if with_tools {
        request["tools"] = json!([search::tool_definition()]);
        request["tool_choice"] = json!("auto");
    }

    let mut response = client
        .post(NIM_URL)
        .bearer_auth(&config.api_key)
        .timeout(REQUEST_TIMEOUT)
        .json(&request)
        .send()
        .await
        .context("NVIDIA NIM'e ulaşılamadı")?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        bail!("NVIDIA NIM HTTP {status}: {}", truncate(&text, 300));
    }

    let mut stream = NimStream::default();
    while let Some(chunk) = response.chunk().await.context("NVIDIA NIM yanıtı okunamadı")? {
        if stream.feed(&chunk) {
            // Araç çağrısı varken gelen içerik cevap değildir; gösterilmez.
            if let (Some(progress), true) = (progress, stream.calls.is_empty()) {
                let visible = visible_text(&stream.content);
                if !visible.is_empty() {
                    progress.send_replace(visible.to_owned());
                }
            }
        }
    }
    Ok(stream.finish())
}

/// Bir isteğin birleştirilmiş sonucu.
struct NimReply {
    content: String,
    /// OpenAI biçiminde araç çağrıları (`id`, `function.name`, `function.arguments`).
    tool_calls: Vec<Value>,
}

/// Akışta parça parça gelen araç çağrısı.
#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// SSE akışını satır satır çözüp içerik ve araç çağrılarını biriktirir.
#[derive(Default)]
struct NimStream {
    /// Henüz satır sonu gelmemiş baytlar (bir olay iki ağ parçasına bölünebilir).
    pending: Vec<u8>,
    content: String,
    calls: Vec<PartialCall>,
}

impl NimStream {
    /// Gelen baytları işler; içerik veya araç çağrısı değiştiyse `true`.
    fn feed(&mut self, bytes: &[u8]) -> bool {
        self.pending.extend_from_slice(bytes);
        let mut changed = false;
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let Ok(event) = serde_json::from_str::<Value>(data.trim()) else {
                // "[DONE]" ve boş satırlar.
                continue;
            };
            changed |= self.apply(&event);
        }
        changed
    }

    fn apply(&mut self, event: &Value) -> bool {
        // Akış parçası `delta`, akışı desteklemeyen sunucular tek seferde `message` gönderir.
        let Some(delta) = event
            .pointer("/choices/0/delta")
            .or_else(|| event.pointer("/choices/0/message"))
        else {
            return false;
        };
        let mut changed = false;
        if let Some(text) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            self.content.push_str(text);
            changed = true;
        }
        for (position, call) in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten().enumerate() {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|i| usize::try_from(i).ok())
                .unwrap_or(position);
            // Model saçmalarsa bellek şişmesin.
            if index >= 8 {
                continue;
            }
            while self.calls.len() <= index {
                self.calls.push(PartialCall::default());
            }
            let Some(partial) = self.calls.get_mut(index) else {
                continue;
            };
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                partial.id = id.to_owned();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                partial.name.push_str(name);
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                partial.arguments.push_str(arguments);
            }
            changed = true;
        }
        changed
    }

    fn finish(self) -> NimReply {
        let tool_calls = self
            .calls
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .enumerate()
            .map(|(i, c)| {
                let id = if c.id.is_empty() { format!("call_{i}") } else { c.id };
                json!({ "id": id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } })
            })
            .collect();
        NimReply {
            content: self.content,
            tool_calls,
        }
    }
}

/// Yazılmakta olan cevabın kullanıcıya gösterilebilir kısmı: `<think>` bölümü gizlenir.
fn visible_text(content: &str) -> &str {
    let text = match content.rfind("</think>") {
        Some(end) => &content[end + "</think>".len()..],
        None if content.contains("<think>") => "",
        None => content,
    };
    text.trim_start()
}

/// Aracı çalıştırır; hata olursa modelin görebileceği bir hata metni döner (cevap yine üretilir).
async fn run_tool_call(
    client: &reqwest::Client,
    backend: &SearchBackend,
    call: &Value,
    progress: Option<&Progress>,
) -> String {
    let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default();
    if name != "web_search" {
        return format!("Bilinmeyen araç: {name}");
    }
    let raw_args = call
        .pointer("/function/arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let args: SearchArgs = match serde_json::from_str(raw_args) {
        Ok(args) => args,
        Err(e) => return format!("Geçersiz arama argümanı: {e}"),
    };

    tracing::info!(query = %args.query, topic = ?args.topic, "AI web araması yapıyor");
    if let Some(progress) = progress {
        progress.send_replace(format!("🔎 Web'de arıyorum: *{}*", truncate(&args.query, 100)));
    }
    match search::search(client, backend, &args).await {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(error = ?e, "Web araması başarısız");
            format!("Arama şu an yapılamadı ({e}). Kullanıcıya güncel bilgiye ulaşamadığını söyle.")
        }
    }
}

async fn gemini(
    client: &reqwest::Client,
    config: &AiConfig,
    system: &str,
    messages: &[ChatMessage],
) -> anyhow::Result<String> {
    let contents: Vec<Value> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "model",
            };
            json!({ "role": role, "parts": [{ "text": m.content }] })
        })
        .collect();

    let response = client
        .post(format!("{GEMINI_URL}/{}:generateContent", config.model))
        // Anahtar URL yerine başlıkta: loglara/proxy kayıtlarına düşmesin.
        .header("x-goog-api-key", &config.api_key)
        .timeout(REQUEST_TIMEOUT)
        .json(&json!({
            "systemInstruction": { "parts": [{ "text": system }] },
            "contents": contents,
            // Gemini'nin yerleşik Google araması: ek anahtar gerekmez, model gerekirse kendisi arar.
            "tools": [{ "google_search": {} }],
            "generationConfig": {
                "maxOutputTokens": MAX_OUTPUT_TOKENS,
                "temperature": TEMPERATURE,
            },
        }))
        .send()
        .await
        .context("Gemini API'ye ulaşılamadı")?;
    let body = read_json(response, "Gemini").await?;

    let parts = body
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            let reason = body
                .pointer("/promptFeedback/blockReason")
                .or_else(|| body.pointer("/candidates/0/finishReason"))
                .and_then(Value::as_str)
                .unwrap_or("bilinmiyor");
            anyhow!("Gemini yanıt üretmedi (sebep: {reason})")
        })?;

    Ok(parts
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(""))
}

async fn read_json(response: reqwest::Response, service: &str) -> anyhow::Result<Value> {
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| format!("{service} yanıtı okunamadı"))?;
    if !status.is_success() {
        bail!("{service} HTTP {status}: {}", truncate(&text, 300));
    }
    serde_json::from_str(&text).with_context(|| format!("{service} yanıtı JSON değil"))
}

/// Sağlayıcılar aynı rolden art arda mesajları sevmez; birleştirilir.
fn merge_consecutive(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut merged: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for msg in messages {
        match merged.last_mut() {
            Some(last) if last.role == msg.role => {
                last.content.push('\n');
                last.content.push_str(&msg.content);
            }
            _ => merged.push(msg.clone()),
        }
    }
    merged
}

/// Akıl yürütme modellerinin `<think>…</think>` bölümünü atar.
fn strip_reasoning(text: &str) -> &str {
    match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.to_owned(),
        }
    }

    #[test]
    fn merges_same_role_messages() {
        let merged = merge_consecutive(&[
            msg(Role::User, "a"),
            msg(Role::User, "b"),
            msg(Role::Assistant, "c"),
            msg(Role::User, "d"),
        ]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].content, "a\nb");
    }

    #[test]
    fn stream_joins_content_split_across_chunks() {
        let mut stream = NimStream::default();
        // Olay iki ağ parçasına, üstelik çok baytlı bir harfin ortasından bölünmüş.
        let event = "data: {\"choices\":[{\"delta\":{\"content\":\"Selam ş\"}}]}\n\n".as_bytes();
        let cut = event.len() - 8;
        assert!(!stream.feed(&event[..cut]));
        assert!(stream.feed(&event[cut..]));
        assert!(stream.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"ef\"}}]}\ndata: [DONE]\n"));
        let reply = stream.finish();
        assert_eq!(reply.content, "Selam şef");
        assert!(reply.tool_calls.is_empty());
    }

    #[test]
    fn stream_assembles_tool_call_arguments() {
        let mut stream = NimStream::default();
        stream.feed(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"web_search\",\"arguments\":\"{\\\"query\\\":\"}}]}}]}\n",
        );
        stream.feed(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"dolar\\\"}\"}}]}}]}\n");
        let reply = stream.finish();
        assert_eq!(reply.tool_calls.len(), 1);
        let call = &reply.tool_calls[0];
        assert_eq!(call.pointer("/function/name").and_then(Value::as_str), Some("web_search"));
        assert_eq!(call.pointer("/function/arguments").and_then(Value::as_str), Some("{\"query\":\"dolar\"}"));
        assert_eq!(call.get("id").and_then(Value::as_str), Some("c1"));
    }

    #[test]
    fn visible_text_hides_unfinished_thinking() {
        assert_eq!(visible_text("<think>hmm"), "");
        assert_eq!(visible_text("<think>hmm</think>\nCevap"), "Cevap");
        assert_eq!(visible_text("Düz"), "Düz");
    }

    #[test]
    fn strips_think_block() {
        assert_eq!(strip_reasoning("<think>uzun düşünce</think>\nCevap"), "\nCevap");
        assert_eq!(strip_reasoning("Düz cevap"), "Düz cevap");
    }
}
