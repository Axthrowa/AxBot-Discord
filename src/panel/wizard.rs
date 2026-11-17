//! Kurulum sihirbazı: yeni bir sunucuda önerilen sırayla yapılacak adımlar. Her adımın durumu mevcut
//! ayarlardan okunur ve ilgili eklenti sayfasına bağlanır (ayarın kendisi o sayfada yapılır).

use std::fmt::Write as _;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use poise::serenity_prelude::GuildId;

use super::pages::{forbidden, layout, GuildNav, Nav, Page};
use super::plugins::{is_enabled, Extra};
use super::{auth, can_manage};
use crate::web::{escape, WebState};

/// Adımlar: (eklenti sayfası, başlık, neden önemli). Sıra = önerilen kurulum sırası.
const STEPS: [(&str, &str, &str); 8] = [
    ("kayitlar", "Kayıt kanalları", "Moderasyon vakaları ve sunucu olayları (silinen mesajlar, giriş/çıkış) buraya yazılır; diğer korumalar uyarılarını da buraya bırakır."),
    ("moderasyon", "Otomatik moderasyon", "Küfür, davet, spam ve büyük harf gibi kuralların eylemleri; muaf roller; uyarı sayısına göre otomatik cezalar."),
    ("karantina", "Karantina rolü", "`/mod karantina` ve sağ tık \"Hızlı ceza\" bu rolü verir. Rolün kanalları görmemesini rol izinlerinden ayarla."),
    ("koruma", "Giriş koruması", "Çok yeni hesapları ve aynı anda çok sayıda girişi (baskın) durdurur."),
    ("dogrulama", "Doğrulama", "Yeni üyeler bir butona (isteğe bağlı resimli kod) basmadan kanalları göremez."),
    ("karsilama", "Karşılama ve otomatik rol", "Hoş geldin / veda mesajları, karşılama kartı, girişte verilen roller."),
    ("biletler", "Destek biletleri", "Üyelerin yetkililere özel kanal açabildiği bilet paneli."),
    ("yikim-korumasi", "Yıkım koruması", "Yetkili hesabı ele geçirilirse toplu kanal/rol silme ve yasaklamayı durdurur. Yalnızca sunucu yöneticileri açabilir."),
];

pub async fn page(State(state): State<WebState>, headers: HeaderMap, Path(guild): Path<u64>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }
    let Some(name) = state.cache.guild(guild_id).map(|g| g.name.clone()) else {
        return forbidden();
    };
    let Ok(cfg) = state.configs.get(&state.db, guild_id).await else {
        return forbidden();
    };
    let extra = Extra::load(&state, guild_id).await;
    let quarantine = state
        .settings
        .get::<crate::automod::rules::ModSettings>(&state.db, guild_id)
        .await
        .is_ok_and(|s| s.quarantine_role.is_some_and(|r| r != 0));
    // İlk adım: botun izinleri ve rol sırası (ayrıntısı sağlık sayfasında).
    let ready = crate::setup_report::build(&state.http, &state.cache, &state.db, &state.configs, guild_id).await.is_ok_and(|r| r.healthy());

    let mut done = usize::from(ready);
    let mut rows = String::new();
    let _ = write!(
        rows,
        "<tr><td>1</td><td>{}</td><td><b>Botun izinleri ve rol sırası</b><br><span class=\"note\">Botun rolü en üstte ve gerekli izinler açık olmalı; yoksa diğer adımlar sessizce çalışmaz.</span></td>\
         <td><a href=\"/panel/sunucu/{guild_id}/saglik\">Kontrol et →</a></td></tr>",
        mark(ready)
    );
    for (i, (slug, title, about)) in STEPS.iter().enumerate() {
        let (on, link) = match *slug {
            "karantina" => (quarantine, "moderasyon"),
            _ => (is_enabled(slug, &cfg, extra), *slug),
        };
        done += usize::from(on);
        let _ = write!(
            rows,
            "<tr><td>{}</td><td>{}</td><td><b>{}</b><br><span class=\"note\">{}</span></td><td><a href=\"/panel/sunucu/{guild_id}/eklenti/{link}\">{} →</a></td></tr>",
            i + 2,
            mark(on),
            escape(title),
            escape(about),
            if on { "Düzenle" } else { "Kur" }
        );
    }
    let total = STEPS.len() + 1;
    let body = format!(
        "<p><a href=\"/panel/sunucu/{guild_id}\">← {}</a></p><h1>🧭 Kurulum sihirbazı</h1>\
         <p class=\"sub\">Yeni bir sunucu için önerilen sıra. {done} / {total} adım tamam. Her adım ilgili eklenti sayfasını açar; kaydedip buraya dönebilirsin.</p>\
         <table><tr><th>#</th><th></th><th>Adım</th><th></th></tr>{rows}</table>\
         <p class=\"note\">Diğer eklentiler (seviye, ekonomi, müzik…) isteğe bağlıdır; Eklentiler sayfasından açılır.</p>",
        escape(&name)
    );
    let nav = GuildNav { id: guild_id, name: &name, active: "kurulum" };
    Html(layout(&state, &session, Nav::Home, Some(nav), Page { title: "Kurulum sihirbazı", body }).await).into_response()
}

fn mark(on: bool) -> &'static str {
    if on {
        "✅"
    } else {
        "⬜"
    }
}
