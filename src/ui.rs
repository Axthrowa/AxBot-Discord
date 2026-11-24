//! Components V2 mesajları (kapsayıcı, metin, bölüm, galeri, ayırıcı).
//!
//! serenity 0.12'de V2 kurucusu yok; mesaj ham JSON olarak `Http` ile gönderilir. V2 bayraklı mesajda
//! `content` ve `embeds` çalışmaz; metin de kapsayıcının içinde gider. Gelen V2 mesajlarında serenity kapsayıcı
//! gibi üst düzey bileşenleri atlar, yalnızca düğme/menü satırlarını okur.

use poise::serenity_prelude as serenity;
use serde_json::{json, Value};

use crate::data::Context;
use crate::error::Error;

/// `IS_COMPONENTS_V2` mesaj bayrağı.
const COMPONENTS_V2: u64 = 1 << 15;
const EPHEMERAL: u64 = 1 << 6;

#[derive(Clone, Debug)]
enum Block {
    Text(String),
    /// Sağında küçük resim olan metin.
    Section { text: String, thumbnail: String },
    Gallery(Vec<String>),
    Separator,
    Row(serenity::CreateActionRow),
}

/// Tek kapsayıcılı V2 mesajı; bloklar eklendiği sırayla görünür.
#[derive(Clone, Debug, Default)]
pub struct Message {
    accent: Option<serenity::Colour>,
    blocks: Vec<Block>,
    ephemeral: bool,
    files: Vec<serenity::CreateAttachment>,
}

impl Message {
    pub fn new() -> Self {
        Self::default()
    }

    /// Kapsayıcının sol kenar rengi.
    pub fn accent(mut self, colour: serenity::Colour) -> Self {
        self.accent = Some(colour);
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(Block::Text(text.into()));
        self
    }

    pub fn section(mut self, text: impl Into<String>, thumbnail: impl Into<String>) -> Self {
        self.blocks.push(Block::Section { text: text.into(), thumbnail: thumbnail.into() });
        self
    }

    /// En fazla 10 görsel (fazlası atılır).
    pub fn gallery(mut self, urls: impl IntoIterator<Item = String>) -> Self {
        self.blocks.push(Block::Gallery(urls.into_iter().take(10).collect()));
        self
    }

    pub fn separator(mut self) -> Self {
        self.blocks.push(Block::Separator);
        self
    }

    /// Düğme ya da menü satırı (kapsayıcının içinde).
    pub fn row(mut self, row: serenity::CreateActionRow) -> Self {
        self.blocks.push(Block::Row(row));
        self
    }

    pub fn rows(self, rows: impl IntoIterator<Item = serenity::CreateActionRow>) -> Self {
        rows.into_iter().fold(self, Self::row)
    }

    pub fn ephemeral(mut self, ephemeral: bool) -> Self {
        self.ephemeral = ephemeral;
        self
    }

    /// Discord'a gidecek mesaj gövdesi (`flags` + `components`).
    pub fn to_json(&self) -> Value {
        let components: Vec<Value> = self
            .blocks
            .iter()
            .map(|block| match block {
                Block::Text(text) => json!({ "type": 10, "content": text }),
                Block::Section { text, thumbnail } => json!({
                    "type": 9,
                    "components": [{ "type": 10, "content": text }],
                    "accessory": { "type": 11, "media": { "url": thumbnail } },
                }),
                Block::Gallery(urls) => json!({
                    "type": 12,
                    "items": urls.iter().map(|url| json!({ "media": { "url": url } })).collect::<Vec<_>>(),
                }),
                Block::Separator => json!({ "type": 14, "divider": true, "spacing": 1 }),
                Block::Row(row) => serde_json::to_value(row).unwrap_or(Value::Null),
            })
            .filter(|v| !v.is_null())
            .collect();
        let mut container = json!({ "type": 17, "components": components });
        if let Some(colour) = self.accent {
            container["accent_color"] = json!(colour.0);
        }
        let flags = COMPONENTS_V2 | if self.ephemeral { EPHEMERAL } else { 0 };
        // Yeni dosyalar `attachments` içinde sırayla numaralanır (düzenlemede eskiler yerine geçer).
        let attachments: Vec<Value> =
            self.files.iter().enumerate().map(|(id, f)| json!({ "id": id, "filename": f.filename })).collect();
        json!({ "flags": flags, "components": [container], "attachments": attachments, "allowed_mentions": { "parse": [] } })
    }

    /// Mesajla gönderilecek dosya; içerikte `attachment://ad` olarak kullanılır.
    pub fn file(mut self, file: serenity::CreateAttachment) -> Self {
        self.files.push(file);
        self
    }

    /// Düzenleme gövdesi: gizlilik bayrağı sonradan değiştirilemez, yalnızca V2 bayrağı gider.
    fn edit_json(&self) -> Value {
        let mut body = self.to_json();
        body["flags"] = json!(COMPONENTS_V2);
        body
    }
}

/// Komuta V2 mesajıyla yanıt verir. Yanıt ertelendiyse (`defer`) bekleme mesajı V2 mesajına dönüşür.
/// Önek komutlarında kanala yazar.
pub async fn reply(ctx: Context<'_>, message: &Message) -> Result<(), Error> {
    let http = &ctx.serenity_context().http;
    let files = message.files.clone();
    match ctx {
        poise::Context::Application(app) => {
            if app.has_sent_initial_response.load(std::sync::atomic::Ordering::SeqCst) {
                http.edit_original_interaction_response(&app.interaction.token, &message.edit_json(), files).await?;
            } else {
                let response = json!({ "type": 4, "data": message.to_json() });
                http.create_interaction_response(app.interaction.id, &app.interaction.token, &response, files).await?;
                app.has_sent_initial_response.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        poise::Context::Prefix(_) => {
            http.send_message(ctx.channel_id(), files, &message.clone().ephemeral(false).to_json()).await?;
        }
    }
    Ok(())
}

/// Düğmeye/menüye basılınca mesajı yerinde günceller.
pub async fn update(http: &serenity::Http, press: &serenity::ComponentInteraction, message: &Message) -> Result<(), Error> {
    let response = json!({ "type": 7, "data": message.edit_json() });
    http.create_interaction_response(press.id, &press.token, &response, message.files.clone()).await?;
    Ok(())
}

/// Komutun ilk yanıtını değiştirir (ör. süre dolunca menüyü kaldırmak için).
pub async fn edit_original(http: &serenity::Http, token: &str, message: &Message) -> Result<(), Error> {
    http.edit_original_interaction_response(token, &message.edit_json(), message.files.clone()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_container_with_flags() {
        let button = serenity::CreateButton::new("x").label("Tamam");
        let message = Message::new()
            .accent(serenity::Colour(0x5865F2))
            .section("Merhaba", "https://example.com/a.png")
            .separator()
            .gallery(vec!["https://example.com/b.png".to_owned()])
            .row(serenity::CreateActionRow::Buttons(vec![button]))
            .ephemeral(true);
        let body = message.to_json();
        assert_eq!(body["flags"], json!(COMPONENTS_V2 | EPHEMERAL));
        let container = &body["components"][0];
        assert_eq!(container["type"], 17);
        assert_eq!(container["accent_color"], 0x5865F2);
        let kinds: Vec<u64> = container["components"].as_array().unwrap().iter().map(|c| c["type"].as_u64().unwrap()).collect();
        assert_eq!(kinds, vec![9, 14, 12, 1]);
    }

    /// serenity V2 bileşenli mesajı hata vermeden çözmeli (düğme tıklamaları buna bağlı).
    #[test]
    fn patched_serenity_reads_v2_messages() {
        let raw = json!({
            "id": "1", "channel_id": "2", "type": 0, "content": "", "timestamp": "2026-11-23T00:00:00+00:00",
            "tts": false, "mention_everyone": false, "mentions": [], "mention_roles": [], "attachments": [],
            "embeds": [], "pinned": false, "flags": COMPONENTS_V2,
            "author": { "id": "3", "username": "bot", "discriminator": "0", "avatar": null },
            "components": [
                { "type": 17, "components": [{ "type": 10, "content": "Merhaba" }] },
                { "type": 1, "components": [{ "type": 2, "style": 1, "custom_id": "x", "label": "Tamam" }] },
            ],
        });
        let message: serenity::Message = serde_json::from_value(raw).expect("V2 mesajı çözülemedi");
        assert_eq!(message.components.len(), 1);
        assert_eq!(message.components[0].components.len(), 1);
    }
}
