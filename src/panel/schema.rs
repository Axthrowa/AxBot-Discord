//! Ayar şeması: bir modülün ayar alanları bir kez tanımlanır; panel formu, form doğrulaması ve
//! kayıt bundan üretilir. Ayarlar `PluginConfig` türünün JSON hâli üzerinden okunup yazılır, şemada
//! olmayan alanlar olduğu gibi korunur. Alan anahtarı hem JSON alan adı hem form adıdır.

use std::future::Future;
use std::pin::Pin;

use poise::serenity_prelude::GuildId;
use serde_json::{json, Map, Value};

use super::plugins::{checkbox, checklist, field, radios, section, Input, Options};
use crate::settings::PluginConfig;
use crate::web::{escape, WebState};

type Fut<'a, T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send + 'a>>;
type Extra = for<'a> fn(&'a WebState, GuildId, &'a Options) -> Fut<'a, String>;

pub enum Kind {
    Bool,
    Number { min: u32, max: u32 },
    /// Boşsa `null` yazılır.
    Text { max: usize, rows: u8, placeholder: &'static str },
    /// Yazı kanalı; boşsa `null` (kapalı).
    Channel,
    /// Sunucunun bütün rollerinden çoklu seçim.
    Roles,
    Choice(fn() -> Vec<(&'static str, &'static str)>),
    /// Değer taşımaz; `label` HTML açıklama olarak gösterilir.
    Note,
}

pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: Kind,
}

/// Ayarın saklandığı yer (bir `PluginConfig` türü).
pub struct Store {
    load: for<'a> fn(&'a WebState, GuildId) -> Fut<'a, Value>,
    save: for<'a> fn(&'a WebState, GuildId, Value) -> Fut<'a, ()>,
}

pub const fn store<T: PluginConfig>() -> Store {
    Store { load: load_as::<T>, save: save_as::<T> }
}

fn load_as<T: PluginConfig>(state: &WebState, guild_id: GuildId) -> Fut<'_, Value> {
    Box::pin(async move { Ok(serde_json::to_value(&*state.settings.get::<T>(&state.db, guild_id).await?)?) })
}

fn save_as<T: PluginConfig>(state: &WebState, guild_id: GuildId, value: Value) -> Fut<'_, ()> {
    Box::pin(async move {
        let settings: T = serde_json::from_value(value)?;
        state.settings.set(&state.db, guild_id, settings).await?;
        Ok(())
    })
}

pub struct Schema {
    pub title: &'static str,
    pub fields: &'static [Field],
    pub store: Store,
    /// Açık/kapalı rozeti bu alana bakar (doğru, dolu ya da sıfırdan büyükse açık); `None` = hep açık.
    pub enabled_by: Option<&'static str>,
    /// Formun altına eklenen, ayar olmayan içerik (ör. sıralama tablosu).
    pub extra: Option<Extra>,
}

fn id_of(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn is_on(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_u64() != Some(0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

impl Schema {
    /// Kayıtlı değerlerle dolu form alanları ve açık/kapalı durumu.
    fn render(&self, current: &Value, options: &Options) -> (String, bool) {
        let mut html = String::new();
        for f in self.fields {
            let value = &current[f.key];
            html += &match &f.kind {
                Kind::Bool => checkbox(f.key, value.as_bool().unwrap_or(false), f.label),
                Kind::Number { min, max } => {
                    let n = value.as_u64().unwrap_or(u64::from(*min));
                    field(f.label, &format!("<input type=\"number\" name=\"{}\" value=\"{n}\" min=\"{min}\" max=\"{max}\">", f.key), f.hint)
                }
                Kind::Text { max, rows, placeholder } => field(
                    f.label,
                    &format!(
                        "<textarea name=\"{}\" rows=\"{rows}\" maxlength=\"{max}\" placeholder=\"{}\">{}</textarea>",
                        f.key,
                        escape(placeholder),
                        escape(value.as_str().unwrap_or_default())
                    ),
                    f.hint,
                ),
                Kind::Channel => field(f.label, &Options::select(f.key, &options.channels, "# ", id_of(value), "— Kapalı —"), f.hint),
                Kind::Roles => {
                    let selected: Vec<u64> = value.as_array().map(|a| a.iter().filter_map(id_of).collect()).unwrap_or_default();
                    field(f.label, &checklist(f.key, &options.all_roles, "@ ", &selected), f.hint)
                }
                Kind::Choice(choices) => field(f.label, &radios(f.key, &choices(), value.as_str().unwrap_or_default()), f.hint),
                Kind::Note => format!("<p class=\"note\">{}</p>", f.label),
            };
        }
        let enabled = self.enabled_by.is_none_or(|key| is_on(&current[key]));
        (section(self.title, html), enabled)
    }

    /// Formu doğrular ve kayıtlı ayarın üstüne yazar; hata kodu `flash` anahtarıdır.
    fn apply(&self, input: &Input<'_>, mut current: Value) -> Result<Value, &'static str> {
        let mut changes = Map::new();
        for f in self.fields {
            let value = match &f.kind {
                Kind::Bool => json!(input.checked(f.key)),
                Kind::Number { min, max } => json!(input.number(f.key, *min, *max)?),
                Kind::Text { max, .. } => {
                    let raw = input.raw(f.key);
                    if raw.chars().count() > *max {
                        return Err("metin");
                    }
                    if raw.is_empty() { Value::Null } else { json!(raw) }
                }
                Kind::Channel => input.channel(f.key)?.map_or(Value::Null, |c| json!(c.get())),
                Kind::Roles => json!(input.ids(f.key, &input.options.all_roles, "rol")?),
                Kind::Choice(choices) => {
                    let raw = input.raw(f.key);
                    if !choices().iter().any(|(key, _)| *key == raw) {
                        return Err("hata");
                    }
                    json!(raw)
                }
                Kind::Note => continue,
            };
            changes.insert(f.key.to_owned(), value);
        }
        let Some(object) = current.as_object_mut() else {
            return Err("hata");
        };
        object.extend(changes);
        Ok(current)
    }
}

/// Eklenti sayfasının form içeriği.
pub async fn fields(schema: &Schema, state: &WebState, guild_id: GuildId, options: &Options) -> anyhow::Result<(String, bool)> {
    let current = (schema.store.load)(state, guild_id).await?;
    let (mut html, enabled) = schema.render(&current, options);
    if let Some(extra) = schema.extra {
        html += &extra(state, guild_id, options).await?;
    }
    Ok((html, enabled))
}

/// Formu kaydeder (önce bütün alanlar doğrulanır, yarım kayıt olmaz).
pub async fn save(schema: &Schema, state: &WebState, guild_id: GuildId, input: &Input<'_>) -> Result<(), &'static str> {
    let current = (schema.store.load)(state, guild_id).await.map_err(|e| {
        tracing::warn!(guild = %guild_id, error = ?e, "Panel: ayar okunamadı");
        "hata"
    })?;
    let updated = schema.apply(input, current)?;
    (schema.store.save)(state, guild_id, updated).await.map_err(|e| {
        tracing::warn!(guild = %guild_id, error = ?e, "Panel: ayar kaydedilemedi");
        "hata"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices() -> Vec<(&'static str, &'static str)> {
        vec![("a", "A"), ("b", "B")]
    }

    const FIELDS: &[Field] = &[
        Field { key: "on", label: "Açık", hint: "", kind: Kind::Bool },
        Field { key: "n", label: "Sayı", hint: "", kind: Kind::Number { min: 1, max: 9 } },
        Field { key: "t", label: "Metin", hint: "", kind: Kind::Text { max: 5, rows: 2, placeholder: "" } },
        Field { key: "ch", label: "Kanal", hint: "", kind: Kind::Channel },
        Field { key: "roles", label: "Roller", hint: "", kind: Kind::Roles },
        Field { key: "pick", label: "Seçim", hint: "", kind: Kind::Choice(choices) },
        Field { key: "note", label: "Açıklama", hint: "", kind: Kind::Note },
    ];

    fn schema() -> Schema {
        Schema { title: "T", fields: FIELDS, store: store::<crate::modules::ModuleSettings>(), enabled_by: Some("on"), extra: None }
    }

    fn options() -> Options {
        Options {
            guild_name: "Test".to_owned(),
            channels: vec![(10, "genel".to_owned())],
            categories: Vec::new(),
            roles: vec![(20, "Üye".to_owned())],
            all_roles: vec![(20, "Üye".to_owned()), (21, "Yönetici".to_owned())],
        }
    }

    fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn applies_valid_form_and_keeps_other_fields() {
        let options = options();
        let data = form(&[("on", "1"), ("n", "3"), ("t", "selam"), ("ch", "10"), ("roles", "21"), ("pick", "b")]);
        let input = Input { form: &data, options: &options };
        let out = schema().apply(&input, json!({ "eski": 7, "t": "x" })).unwrap();
        assert_eq!(out, json!({ "eski": 7, "on": true, "n": 3, "t": "selam", "ch": 10, "roles": [21], "pick": "b" }));
        let (html, enabled) = schema().render(&out, &options);
        assert!(enabled && html.contains("selam") && html.contains("Açıklama"));
    }

    #[test]
    fn rejects_invalid_values() {
        let options = options();
        let cases: &[(&[(&str, &str)], &str)] = &[
            (&[("n", "10"), ("pick", "a")], "sayi"),
            (&[("n", "2"), ("t", "çok uzun"), ("pick", "a")], "metin"),
            (&[("n", "2"), ("ch", "999"), ("pick", "a")], "kanal"),
            (&[("n", "2"), ("roles", "999"), ("pick", "a")], "rol"),
            (&[("n", "2"), ("pick", "z")], "hata"),
        ];
        for (pairs, code) in cases {
            let data = form(pairs);
            let input = Input { form: &data, options: &options };
            assert_eq!(schema().apply(&input, json!({})).err(), Some(*code), "{pairs:?}");
        }
        // Boş metin ve kanal kapalı demektir.
        let data = form(&[("n", "2"), ("pick", "a")]);
        let out = schema().apply(&Input { form: &data, options: &options }, json!({})).unwrap();
        assert_eq!((out["t"].clone(), out["ch"].clone(), out["on"].clone()), (Value::Null, Value::Null, json!(false)));
    }
}
