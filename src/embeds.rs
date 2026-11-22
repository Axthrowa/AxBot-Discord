//! Gömülü mesajlar (Embed Builder): Panelden gönderilen ve düzenlenen zengin içerikli mesajlar.
//!
//! Ayarlar `PluginSettings` deposunda `"embeds"` anahtarında tutulur.

use poise::serenity_prelude::{self as serenity, CreateEmbed, CreateEmbedFooter};
use serde::{Deserialize, Serialize};

use crate::settings::PluginConfig;

pub const MAX_EMBEDS: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

/// Panelden gönderilmiş bir gömülü mesaj (düzenlemek ve silmek için saklanır).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedEmbed {
    pub message_id: u64,
    pub channel_id: u64,
    pub title: String,
    pub description: String,
    /// `#RRGGBB` formatında renk kodu.
    pub colour: String,
    pub image: Option<String>,
    pub thumbnail: Option<String>,
    pub footer: Option<String>,
    pub author: Option<String>,
    pub url: Option<String>,
    pub timestamp: bool,
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedSettings {
    pub embeds: Vec<SavedEmbed>,
}

impl PluginConfig for EmbedSettings {
    const KEY: &'static str = "embeds";
}

impl SavedEmbed {
    pub fn build(&self) -> CreateEmbed {
        let [r, g, b] = crate::cards::parse_hex(&self.colour).unwrap_or([88, 101, 242]);
        let mut embed = CreateEmbed::new().colour(serenity::Colour::from_rgb(r, g, b));
        if !self.title.is_empty() {
            embed = embed.title(&self.title);
        }
        if !self.description.is_empty() {
            embed = embed.description(&self.description);
        }
        if let Some(author) = &self.author {
            if !author.is_empty() {
                embed = embed.author(serenity::CreateEmbedAuthor::new(author));
            }
        }
        if let Some(url) = &self.url {
            if !url.is_empty() {
                embed = embed.url(url);
            }
        }
        if self.timestamp {
            embed = embed.timestamp(serenity::Timestamp::now());
        }
        if let Some(image) = &self.image {
            embed = embed.image(image);
        }
        if let Some(thumbnail) = &self.thumbnail {
            embed = embed.thumbnail(thumbnail);
        }
        if let Some(footer) = &self.footer {
            embed = embed.footer(CreateEmbedFooter::new(footer));
        }
        for field in &self.fields {
            if !field.name.is_empty() && !field.value.is_empty() {
                embed = embed.field(&field.name, &field.value, field.inline);
            }
        }
        embed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_settings_json_roundtrip() -> anyhow::Result<()> {
        let embed = SavedEmbed {
            message_id: 123456,
            channel_id: 654321,
            title: "Duyuru".to_owned(),
            description: "Test açıklaması".to_owned(),
            colour: "#FF0000".to_owned(),
            image: Some("https://example.com/img.png".to_owned()),
            thumbnail: None,
            footer: Some("Alt bilgi".to_owned()),
            ..SavedEmbed::default()
        };
        let settings = EmbedSettings {
            embeds: vec![embed.clone()],
        };

        let json = serde_json::to_string(&settings)?;
        let back: EmbedSettings = serde_json::from_str(&json)?;
        assert_eq!(settings, back);
        assert_eq!(back.embeds.len(), 1);
        assert_eq!(back.embeds[0].title, "Duyuru");
        Ok(())
    }

    #[test]
    fn saved_embed_build_creates_expected_embed() {
        let saved = SavedEmbed {
            message_id: 1,
            channel_id: 1,
            title: "Başlık".to_owned(),
            description: "Açıklama".to_owned(),
            colour: "#5865F2".to_owned(),
            image: None,
            thumbnail: None,
            footer: Some("Son".to_owned()),
            ..SavedEmbed::default()
        };
        let embed = saved.build();
        // Embed build'i başarıyla panik olmadan oluşturulmalı
        assert!(!format!("{embed:?}").is_empty());
    }

    #[test]
    fn saved_embed_new_fields_and_backward_compatibility() -> anyhow::Result<()> {
        let old_json = r##"{
            "message_id": 999,
            "channel_id": 888,
            "title": "Eski Başlık",
            "description": "Eski Açıklama",
            "colour": "#336699"
        }"##;

        let parsed: SavedEmbed = serde_json::from_str(old_json)?;
        assert_eq!(parsed.title, "Eski Başlık");
        assert_eq!(parsed.author, None);
        assert_eq!(parsed.url, None);
        assert!(!parsed.timestamp);
        assert!(parsed.fields.is_empty());

        let with_new_fields = SavedEmbed {
            message_id: 111,
            channel_id: 222,
            title: "Yeni Başlık".to_owned(),
            description: "Yeni Açıklama".to_owned(),
            colour: "#FF5500".to_owned(),
            image: None,
            thumbnail: None,
            footer: Some("Alt".to_owned()),
            author: Some("AxBot Yazar".to_owned()),
            url: Some("https://example.com/post".to_owned()),
            timestamp: true,
            fields: vec![EmbedField {
                name: "Alan 1".to_owned(),
                value: "Değer 1".to_owned(),
                inline: true,
            }],
        };

        let json = serde_json::to_string(&with_new_fields)?;
        let back: SavedEmbed = serde_json::from_str(&json)?;
        assert_eq!(with_new_fields, back);
        assert_eq!(back.fields.len(), 1);
        assert_eq!(back.author.as_deref(), Some("AxBot Yazar"));

        let built = with_new_fields.build();
        assert!(!format!("{built:?}").is_empty());
        Ok(())
    }
}
