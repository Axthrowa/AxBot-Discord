//! `/panel/gorunum`: tema ve vurgu rengi seçici + panelin bütün ortak bileşenleri tek sayfada.
//! Hiçbir veri göstermez; girişsiz de açılır (seçim tarayıcıda saklanır, bkz. `theme.js`).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Html;

use super::pages::{layout, layout_public, Nav, Page};
use crate::web::WebState;

const THEMES: &[(&str, &str, &str, &str)] = &[
    ("", "Sistem", "#10151F", "#F5F6F9"),
    ("koyu", "Koyu", "#10151F", "#171D2A"),
    ("acik", "Açık", "#F5F6F9", "#FFFFFF"),
    ("siyah", "Siyah", "#000000", "#0B0B0E"),
    ("grafit", "Grafit", "#1B1C1F", "#232428"),
];

const ACCENTS: &[(&str, &str, &str)] = &[
    ("", "Deniz yosunu", "#2FB8A6"),
    ("mavi", "Mavi", "#5B8DEF"),
    ("mor", "Mor", "#A084F5"),
    ("pembe", "Pembe", "#EC79B0"),
    ("turuncu", "Turuncu", "#F29A50"),
    ("kirmizi", "Kırmızı", "#EC6A60"),
    ("yesil", "Yeşil", "#55C77F"),
];

/// Tema ve renk seçici (düğmeleri `theme.js` dinler).
fn picker() -> String {
    let themes: String = THEMES
        .iter()
        .map(|(value, name, bg, surface)| {
            format!(
                "<button type=\"button\" class=\"tpick\" data-value=\"{value}\" aria-pressed=\"false\">\
                 <i style=\"background:linear-gradient(135deg,{bg} 50%,{surface} 50%)\"></i>{name}</button>"
            )
        })
        .collect();
    let accents: String = ACCENTS
        .iter()
        .map(|(value, name, colour)| {
            format!(
                "<button type=\"button\" class=\"apick\" data-value=\"{value}\" title=\"{name}\" aria-label=\"{name}\" \
                 aria-pressed=\"false\" style=\"background:{colour}\"></button>"
            )
        })
        .collect();
    format!(
        "<div class=\"sect\"><h3>Tema</h3><div class=\"pick\" data-pick=\"theme\">{themes}</div>\
         <h3 style=\"margin-top:14px\">Vurgu rengi</h3><div class=\"pick\" data-pick=\"accent\">{accents}</div>\
         <p class=\"note\">Seçim bu tarayıcıda saklanır ve bütün panel sayfalarında geçerli olur. \
         \"Sistem\" bilgisayarın açık/koyu tercihini izler.</p></div>"
    )
}
use super::plugins::{checkbox, checklist, field, radios, section, Options};

const SWATCHES: &[(&str, &str)] = &[
    ("--bg", "Zemin"),
    ("--surface", "Yüzey"),
    ("--line", "Çizgi"),
    ("--text", "Metin"),
    ("--muted", "Soluk metin"),
    ("--accent", "Vurgu"),
    ("--ok", "Başarılı"),
    ("--warn", "Uyarı"),
    ("--err", "Hata"),
];

pub async fn page(State(state): State<WebState>, headers: HeaderMap) -> Html<String> {
    let swatches: String = SWATCHES
        .iter()
        .map(|(var, name)| {
            format!(
                "<div class=\"card\"><div style=\"height:28px;border-radius:var(--r-sm);border:1px solid var(--line);background:var({var})\"></div>\
                 <div class=\"k\" style=\"margin-top:6px\">{name}</div><code>{var}</code></div>"
            )
        })
        .collect();
    let options = Options {
        guild_name: "Örnek".to_owned(),
        channels: vec![(1, "genel".to_owned()), (2, "duyurular".to_owned())],
        categories: Vec::new(),
        roles: vec![(3, "Üye".to_owned())],
        all_roles: vec![(3, "Üye".to_owned()), (4, "Moderatör".to_owned()), (5, "Yönetici".to_owned())],
    };
    let form = section(
        "Form alanları",
        field("Metin", "<input type=\"text\" value=\"Merhaba {kullanıcı}\">", "Açıklama satırı burada durur.")
            + &field("Sayı", "<input type=\"number\" value=\"7\" min=\"0\" max=\"30\">", "")
            + &field("Kanal", &Options::select("kanal", &options.channels, "# ", Some(1), "— Kapalı —"), "")
            + &field("Uzun metin", "<textarea rows=\"3\">Çok satırlı metin</textarea>", "")
            + &checkbox("anahtar", true, "Açık anahtar")
            + &checkbox("anahtar2", false, "Kapalı anahtar")
            + &field("Seçenek", &radios("secim", &[("a", "Yetkileri al"), ("b", "At"), ("c", "Yasakla")], "a"), "")
            + &field("Roller", &checklist("rol", &options.all_roles, "@ ", &[4]), ""),
    );
    let body = format!(
        "<h1>Görünüm</h1><p class=\"sub\">Panelin teması ve vurgu rengi; altta bütün ortak parçalar seçtiğin renklerle görünür.</p>\
         {picker}<h2>Renkler</h2><div class=\"cards\">{swatches}</div>\
         <h2>Yazı</h2><div class=\"box\"><h1>Başlık 1</h1><h2 style=\"margin-top:4px\">Başlık 2</h2>\
         <p class=\"lead\">Gövde metni: Türkçe karakterler — ğüşıöç ĞÜŞİÖÇ.</p><p class=\"note\">Soluk not metni.</p>\
         <code>1556440251059408966</code></div>\
         <h2>Düğmeler</h2><div class=\"box\" style=\"display:flex;gap:8px;flex-wrap:wrap\">\
         <button>Kaydet</button><button class=\"ghost\">Vazgeç</button><button class=\"danger\">Sil</button><a class=\"btn\" href=\"#\">Bağlantı düğmesi</a></div>\
         <h2>Durum</h2><div class=\"box\" style=\"display:flex;gap:8px;flex-wrap:wrap\">\
         <span class=\"badge b-ok\">AÇIK</span><span class=\"badge b-warn\">EKSİK İZİN</span><span class=\"badge b-err\">HATA</span>\
         <span class=\"badge b-off\">KAPALI</span></div>\
         <div class=\"flash ok\" style=\"margin-top:10px\">✓ Ayarlar kaydedildi.</div><div class=\"flash err\">✕ Kanal bu sunucuya ait değil.</div>\
         <div class=\"box warn\" style=\"margin-bottom:8px\">Uyarı kutusu</div><div class=\"box err\">Hata kutusu</div>\
         <h2>Form</h2><form onsubmit=\"return false\">{form}<div class=\"savebar\"><button>Kaydet</button>\
         <a class=\"btn ghost\" href=\"#\">Vazgeç</a></div></form>\
         <h2>Tablo</h2><table><tr><th>#</th><th>Üye</th><th>Puan</th></tr>\
         <tr><td>1</td><td>Ayşe</td><td>1.240</td></tr><tr><td>2</td><td>Mehmet</td><td>980</td></tr></table>",
        picker = picker(),
    );
    let html = match super::auth::session(&state, &headers).await {
        Some(session) => layout(&state, &session, Nav::Look, None, Page { title: "Görünüm", body }).await,
        None => layout_public("Görünüm", &body),
    };
    Html(html)
}
