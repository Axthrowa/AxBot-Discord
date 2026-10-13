//! Sunucu yönetimi: `/davetler`, `/davet-sıralama`, `/sayaç`, `/doğum-günü`, `/geçici-oda`, `/oda`, `/youtube`.

use poise::serenity_prelude::{self as serenity, CreateEmbed, EditChannel, Mentionable, PermissionOverwrite, PermissionOverwriteType, Permissions};
use poise::{ChoiceParameter as _, CreateReply};

use super::community::cannot_send;
use super::moderation::{guild_id, reply_private};
use crate::data::Context;
use crate::error::Error;
use crate::guild_config::Setting;
use crate::statchannels::{self, StatKind};
use crate::{birthdays, invites, youtube};

// ===================== Davet takibi =====================

/// Bir üyenin kaç kişiyi davet ettiğini gösterir.
#[poise::command(slash_command, guild_only, category = "Sunucu")]
pub async fn davetler(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Bakılacak üye (boşsa sen)"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let db = &ctx.data().db;
    let stats = invites::stats(db, guild_id, user.id).await?;
    let mut embed = CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
        .title(format!("📨 {} davet", stats.active))
        .description(format!(
            "✅ Sunucuda olan: **{}**\n🚪 Ayrılan: **{}**\n⚠️ Sahte (7 günden yeni hesap): **{}**",
            stats.active, stats.left, stats.fake
        ))
        .colour(serenity::Colour::BLURPLE);
    if let Some((inviter, code)) = invites::invited_by(db, guild_id, user.id).await? {
        let text = match (inviter, code) {
            (Some(inviter), Some(code)) => format!("{} (`{code}`)", inviter.mention()),
            (Some(inviter), None) => inviter.mention().to_string(),
            (None, _) => "Bilinmiyor (özel bağlantı veya izin yok)".to_owned(),
        };
        embed = embed.field("Davet eden", text, false);
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// En çok davet eden 10 üye.
#[poise::command(slash_command, guild_only, rename = "davet-sıralama", category = "Sunucu")]
pub async fn davet_siralama(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let top = invites::leaderboard(&ctx.data().db, guild_id, 10).await?;
    let description = if top.is_empty() {
        "Henüz takip edilen davet yok. _Davetlerin sayılması için botun 'Sunucuyu Yönet' izni olmalı._".to_owned()
    } else {
        top.iter()
            .enumerate()
            .map(|(i, (user, n))| {
                let medal = ["🥇", "🥈", "🥉"].get(i).map_or_else(|| format!("`#{}`", i + 1), |m| (*m).to_owned());
                format!("{medal} {} — **{n}** davet", user.mention())
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let embed = CreateEmbed::new()
        .title("📨 Davet sıralaması")
        .description(description)
        .colour(serenity::Colour::GOLD)
        .footer(serenity::CreateEmbedFooter::new("Sadece sunucuda olan ve sahte olmayan davetler sayılır."));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ===================== Sayaç kanalları =====================

/// Adı otomatik güncellenen sayaç kanalları (üye sayısı, hedef…).
#[poise::command(
    slash_command,
    guild_only,
    rename = "sayaç",
    default_member_permissions = "MANAGE_CHANNELS",
    subcommands("sayac_ekle", "sayac_sil", "sayac_liste"),
    subcommand_required,
    category = "Sunucu"
)]
pub async fn sayac(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Sayaç kanalı oluşturur (kilitli ses kanalı; adı 10 dakikada bir güncellenir).
#[poise::command(slash_command, guild_only, rename = "ekle", required_bot_permissions = "MANAGE_CHANNELS")]
async fn sayac_ekle(
    ctx: Context<'_>,
    #[rename = "tür"]
    #[description = "Ne sayılsın"]
    kind: StatKind,
    #[rename = "hedef"]
    #[description = "Üye hedefi (sadece 'Üye hedefi' türü için)"]
    #[min = 1]
    goal: Option<u32>,
    #[description = "Sayılacak rol (sadece 'Roldeki üyeler' türü için)"] rol: Option<serenity::Role>,
    #[description = "Para (sadece 'Kripto para fiyatı' için; ör. bitcoin, ethereum, dogecoin)"]
    #[max_length = 50]
    para: Option<String>,
    #[description = "Kanalın oluşturulacağı kategori"]
    #[channel_types("Category")]
    kategori: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    if kind == StatKind::Goal && goal.is_none() {
        return reply_private(ctx, "🎯 'Üye hedefi' için `hedef` sayısını da yazmalısın.").await;
    }
    if kind == StatKind::RoleMembers && rol.is_none() {
        return reply_private(ctx, "👥 'Roldeki üyeler' için `rol` seçmelisin.").await;
    }
    if matches!(kind, StatKind::Online | StatKind::RecordOnline) && !statchannels::presence_intent() {
        return reply_private(
            ctx,
            "🟢 Çevrimiçi sayaçları için botun **Presence Intent** izni açık olmalı:\n\
             Discord Developer Portal → uygulaman → **Bot** → *Privileged Gateway Intents* → **Presence Intent**'i aç, kaydet ve botu yeniden başlat.",
        )
        .await;
    }
    let coin = para.map(|p| p.trim().to_lowercase().replace(' ', "-"));
    if kind == StatKind::Crypto && !coin.as_deref().is_some_and(statchannels::valid_coin) {
        return reply_private(ctx, "💰 'Kripto para fiyatı' için `para` yaz (CoinGecko adıyla: `bitcoin`, `ethereum`, `dogecoin`…).").await;
    }
    if statchannels::count(&data.db, guild_id).await? >= statchannels::MAX_PER_GUILD {
        return reply_private(ctx, format!("En fazla {} sayaç kanalı olabilir.", statchannels::MAX_PER_GUILD)).await;
    }
    ctx.defer_ephemeral().await?;
    let goal = if kind == StatKind::Goal { goal } else { None };
    let role = if kind == StatKind::RoleMembers { rol.map(|r| r.id) } else { None };
    let price = match (kind, coin.as_deref()) {
        (StatKind::Crypto, Some(coin)) => match statchannels::coin_price(&data.http_client, coin).await {
            Ok(price) => Some((coin, price)),
            Err(_) => return reply_private(ctx, "❌ Bu para bulunamadı ya da fiyat servisine ulaşılamadı. CoinGecko'daki adını yaz (ör. `bitcoin`).").await,
        },
        _ => None,
    };
    let extra = price.as_ref().map(|(coin, price)| (*coin, price.as_str()));
    let channel = statchannels::create(&ctx.serenity_context().cache, ctx.http(), &data.db, guild_id, kind, goal, role, kategori.map(|c| c.id), extra).await?;
    reply_private(
        ctx,
        format!(
            "✅ Sayaç kanalı oluşturuldu: {}\n_Adı 10 dakikada bir güncellenir (Discord sınırı). Kanalı silersen sayaç da kalkar._",
            channel.mention()
        ),
    )
    .await
}

/// Bir sayaç kanalını siler.
#[poise::command(slash_command, guild_only, rename = "sil", required_bot_permissions = "MANAGE_CHANNELS")]
async fn sayac_sil(
    ctx: Context<'_>,
    #[description = "Silinecek sayaç kanalı"]
    #[channel_types("Voice")]
    kanal: serenity::GuildChannel,
) -> Result<(), Error> {
    let data = ctx.data();
    if !statchannels::on_channel_deleted(&data.db, kanal.id).await? {
        return reply_private(ctx, "Bu bir sayaç kanalı değil.").await;
    }
    let _ = kanal.id.delete(ctx).await;
    reply_private(ctx, "🗑️ Sayaç kanalı silindi.").await
}

/// Sayaç kanallarını listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn sayac_liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let list = statchannels::list(&ctx.data().db, guild_id).await?;
    let text = if list.is_empty() {
        "Henüz sayaç kanalı yok. `/sayaç ekle` ile oluşturabilirsin.".to_owned()
    } else {
        list.iter()
            .map(|(channel, kind, goal)| {
                let goal = goal.map_or_else(String::new, |g| format!(" (hedef {g})"));
                format!("• {} — {}{goal}", channel.mention(), kind.name())
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    reply_private(ctx, text).await
}

// ===================== Doğum günleri =====================

/// Doğum günleri: kaydet, listele, kutlama kanalı.
#[poise::command(
    slash_command,
    guild_only,
    rename = "doğum-günü",
    subcommands("dg_ayarla", "dg_sil", "dg_liste", "dg_kanal"),
    subcommand_required,
    category = "Sunucu"
)]
pub async fn dogum_gunu(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Doğum gününü kaydeder (yıl sorulmaz).
#[poise::command(slash_command, guild_only, rename = "ayarla")]
async fn dg_ayarla(
    ctx: Context<'_>,
    #[rename = "gün"]
    #[description = "Gün (1-31)"]
    #[min = 1]
    #[max = 31]
    day: u32,
    #[rename = "ay"]
    #[description = "Ay (1-12)"]
    #[min = 1]
    #[max = 12]
    month: u32,
    #[rename = "yıl"]
    #[description = "Doğum yılı (isteğe bağlı; sunucu izin veriyorsa yaşınla kutlanırsın)"]
    #[min = 1900]
    #[max = 2100]
    year: Option<i32>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    if !birthdays::is_valid(month, day) {
        return reply_private(ctx, format!("❌ {day}/{month} geçerli bir tarih değil.")).await;
    }
    let data = ctx.data();
    let allow_year = data.settings.get::<birthdays::BirthdaySettings>(&data.db, guild_id).await?.allow_year;
    // Sunucu yıl kullanımına izin vermiyorsa yıl hiç saklanmaz.
    let stored_year = year.filter(|_| allow_year);
    birthdays::set(&data.db, guild_id, ctx.author().id, month, day, stored_year).await?;
    let cfg = data.guild_configs.get(&data.db, guild_id).await?;
    let mut text = format!("🎂 Doğum günün **{}** olarak kaydedildi.", birthdays::format(month, day));
    if cfg.birthday_channel.is_none() && cfg.birthday_role.is_none() {
        text.push_str("\n_Bu sunucuda kutlama henüz açık değil; bir yöneticinin `/doğum-günü kanal` ayarlaması gerekiyor._");
    }
    reply_private(ctx, text).await
}

/// Kayıtlı doğum gününü siler.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn dg_sil(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let text = if birthdays::remove(&ctx.data().db, guild_id, ctx.author().id).await? {
        "🗑️ Doğum günün silindi."
    } else {
        "Kayıtlı bir doğum günün yok."
    };
    reply_private(ctx, text).await
}

/// Yaklaşan doğum günlerini listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn dg_liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let today = birthdays::today();
    let mut all = birthdays::all(&ctx.data().db, guild_id).await?;
    if let Some(guild) = ctx.guild() {
        all.retain(|(user, _, _)| guild.members.contains_key(user));
    }
    all.sort_by_key(|(_, month, day)| birthdays::days_until(today, *month, *day));
    let description = if all.is_empty() {
        "Henüz kimse doğum gününü kaydetmedi. `/doğum-günü ayarla` ile ekleyebilirsin! 🎂".to_owned()
    } else {
        all.iter()
            .take(15)
            .map(|(user, month, day)| {
                let left = birthdays::days_until(today, *month, *day);
                let when = match left {
                    0 => "**bugün!** 🎉".to_owned(),
                    1 => "yarın".to_owned(),
                    n => format!("{n} gün sonra"),
                };
                format!("• {} — {} ({when})", user.mention(), birthdays::format(*month, *day))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let embed = CreateEmbed::new()
        .title("🎂 Yaklaşan doğum günleri")
        .description(description)
        .colour(serenity::Colour::from_rgb(255, 115, 250));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Kutlama kanalını ve doğum günü rolünü ayarlar (ikisi de boşsa kutlama kapanır).
#[poise::command(slash_command, guild_only, rename = "kanal", required_permissions = "MANAGE_GUILD")]
async fn dg_kanal(
    ctx: Context<'_>,
    #[description = "Kutlama mesajlarının gönderileceği kanal"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
    #[description = "Doğum günü boyunca verilecek rol (ertesi gün alınır)"] rol: Option<serenity::Role>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    if let Some(channel) = &kanal {
        if let Some(problem) = cannot_send(ctx, channel) {
            return reply_private(ctx, problem).await;
        }
    }
    if let Some(role) = &rol {
        if let Err(reason) = crate::rolepanel::check_role(ctx.serenity_context(), guild_id, role) {
            return reply_private(ctx, format!("❌ {reason}")).await;
        }
    }
    data.guild_configs
        .set(&data.db, guild_id, Setting::Birthday(kanal.as_ref().map(|c| c.id), rol.as_ref().map(|r| r.id)))
        .await?;
    let text = match (&kanal, &rol) {
        (None, None) => "🎂 Doğum günü kutlamaları kapatıldı.".to_owned(),
        _ => {
            let mut parts = vec!["🎂 Doğum günü kutlamaları açık (Türkiye saatiyle gün başında).".to_owned()];
            if let Some(c) = &kanal {
                parts.push(format!("• Kanal: {}", c.id.mention()));
            }
            if let Some(r) = &rol {
                parts.push(format!("• Rol: {}", r.id.mention()));
            }
            parts.join("\n")
        }
    };
    reply_private(ctx, text).await
}

// ===================== Geçici ses odaları =====================

/// "Oda Oluştur" kanalını kurar: oraya giren üyeye kendi ses odası açılır.
#[poise::command(
    slash_command,
    guild_only,
    rename = "geçici-oda",
    default_member_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS | MOVE_MEMBERS",
    category = "Sunucu"
)]
pub async fn gecici_oda(
    ctx: Context<'_>,
    #[description = "Oda oluşturma kanalı olacak ses kanalı (boşsa yeni oluşturulur)"]
    #[channel_types("Voice")]
    kanal: Option<serenity::GuildChannel>,
    #[description = "Yeni kanal oluşturulacaksa kategorisi"]
    #[channel_types("Category")]
    kategori: Option<serenity::GuildChannel>,
    #[description = "Sistemi kapat"] kapat: Option<bool>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    if kapat == Some(true) {
        data.guild_configs.set(&data.db, guild_id, Setting::TempVoiceHub(None)).await?;
        return reply_private(ctx, "🔇 Geçici oda sistemi kapatıldı (açık odalar boşalınca silinir).").await;
    }
    let hub = match kanal {
        Some(channel) => channel.id,
        None => {
            let mut builder = serenity::CreateChannel::new("➕ Oda Oluştur").kind(serenity::ChannelType::Voice);
            if let Some(category) = &kategori {
                builder = builder.category(category.id);
            }
            guild_id.create_channel(ctx, builder).await?.id
        }
    };
    data.guild_configs.set(&data.db, guild_id, Setting::TempVoiceHub(Some(hub))).await?;
    reply_private(
        ctx,
        format!(
            "✅ {} kanalına giren üyelere kendi ses odası açılacak; oda boşalınca silinir.\n\
             Oda sahibi `/oda isim`, `/oda limit`, `/oda kilitle`, `/oda aç` komutlarını kullanabilir.",
            hub.mention()
        ),
    )
    .await
}

/// Geçici ses odanı yönet.
#[poise::command(
    slash_command,
    guild_only,
    subcommands(
        "oda_isim",
        "oda_limit",
        "oda_kilitle",
        "oda_ac",
        "oda_sahiplen",
        "oda_yasakla",
        "oda_yasak_kaldir",
        "oda_at",
        "oda_gizle",
        "oda_goster",
        "oda_devret",
        "oda_sahip",
        "oda_temizle"
    ),
    subcommand_required,
    category = "Sunucu"
)]
pub async fn oda(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Komutu kullananın bulunduğu geçici oda; sahibi değilse (veya odada değilse) açıklama döner.
async fn my_room(ctx: Context<'_>, require_owner: bool) -> Result<Option<serenity::ChannelId>, Error> {
    let user = ctx.author().id;
    let channel = ctx
        .guild()
        .and_then(|g| g.voice_states.get(&user).and_then(|vs| vs.channel_id));
    let Some(channel) = channel else {
        reply_private(ctx, "Önce geçici ses odana gir.").await?;
        return Ok(None);
    };
    let Some(owner) = ctx.data().tempvoice.owner_of(channel) else {
        reply_private(ctx, "Bulunduğun kanal bir geçici oda değil.").await?;
        return Ok(None);
    };
    if require_owner && owner != user {
        reply_private(ctx, format!("Bu odanın sahibi {}; sadece o değiştirebilir.", owner.mention())).await?;
        return Ok(None);
    }
    Ok(Some(channel))
}

/// Odanın adını değiştirir.
#[poise::command(slash_command, guild_only, rename = "isim")]
async fn oda_isim(
    ctx: Context<'_>,
    #[rename = "isim"]
    #[description = "Yeni oda adı"]
    #[max_length = 90]
    name: String,
) -> Result<(), Error> {
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    channel.edit(ctx, EditChannel::new().name(name.trim())).await?;
    reply_private(ctx, "✅ Oda adı değiştirildi. _(Discord bir kanalın adını 10 dakikada en fazla 2 kez değiştirmeye izin verir.)_").await
}

/// Odaya girebilecek kişi sayısını ayarlar (0 = sınırsız).
#[poise::command(slash_command, guild_only, rename = "limit")]
async fn oda_limit(
    ctx: Context<'_>,
    #[rename = "kişi"]
    #[description = "En fazla kişi (0 = sınırsız)"]
    #[min = 0]
    #[max = 99]
    limit: u32,
) -> Result<(), Error> {
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    channel.edit(ctx, EditChannel::new().user_limit(limit)).await?;
    let text = if limit == 0 { "✅ Kişi sınırı kaldırıldı.".to_owned() } else { format!("✅ Oda en fazla **{limit}** kişilik.") };
    reply_private(ctx, text).await
}

/// Odayı kilitler: yeni kimse giremez (içeridekiler kalır).
#[poise::command(slash_command, guild_only, rename = "kilitle")]
async fn oda_kilitle(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    let overwrite = PermissionOverwrite {
        allow: Permissions::empty(),
        deny: Permissions::CONNECT,
        kind: PermissionOverwriteType::Role(guild_id.everyone_role()),
    };
    channel.create_permission(ctx, overwrite).await?;
    reply_private(ctx, "🔒 Oda kilitlendi.").await
}

/// Odanın kilidini açar.
#[poise::command(slash_command, guild_only, rename = "aç")]
async fn oda_ac(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    channel
        .delete_permission(ctx, PermissionOverwriteType::Role(guild_id.everyone_role()))
        .await?;
    reply_private(ctx, "🔓 Oda herkese açıldı.").await
}

/// Sahibi odadan çıktıysa odayı sahiplenirsin.
#[poise::command(slash_command, guild_only, rename = "sahiplen")]
async fn oda_sahiplen(ctx: Context<'_>) -> Result<(), Error> {
    let Some(channel) = my_room(ctx, false).await? else {
        return Ok(());
    };
    let data = ctx.data();
    let owner = data.tempvoice.owner_of(channel);
    let owner_present = owner.is_some_and(|o| {
        ctx.guild()
            .is_some_and(|g| g.voice_states.get(&o).is_some_and(|vs| vs.channel_id == Some(channel)))
    });
    if owner == Some(ctx.author().id) {
        return reply_private(ctx, "Bu oda zaten senin.").await;
    }
    if owner_present {
        return reply_private(ctx, "Oda sahibi hâlâ odada; sahiplenemezsin.").await;
    }
    let user = ctx.author().id;
    let manage = Permissions::VIEW_CHANNEL | Permissions::CONNECT | Permissions::SPEAK | Permissions::MANAGE_CHANNELS | Permissions::MOVE_MEMBERS;
    channel
        .create_permission(ctx, PermissionOverwrite { allow: manage, deny: Permissions::empty(), kind: PermissionOverwriteType::Member(user) })
        .await?;
    if let Some(old) = owner {
        let _ = channel.delete_permission(ctx, PermissionOverwriteType::Member(old)).await;
    }
    data.tempvoice.set_owner(&data.db, channel, user).await?;
    ctx.say(format!("👑 {} odanın yeni sahibi.", user.mention())).await?;
    Ok(())
}

// ===================== YouTube =====================

/// YouTube yeni video duyuruları.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    subcommands("yt_ekle", "yt_sil", "yt_liste"),
    subcommand_required,
    category = "Sunucu"
)]
pub async fn youtube(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bir YouTube kanalının yeni videolarını duyurur.
#[poise::command(slash_command, guild_only, rename = "ekle")]
async fn yt_ekle(
    ctx: Context<'_>,
    #[rename = "youtube_kanalı"]
    #[description = "Kanal bağlantısı veya @adı (örn. @barisozcan)"]
    #[max_length = 200]
    source: String,
    #[rename = "duyuru_kanalı"]
    #[description = "Duyurunun yapılacağı kanal"]
    #[channel_types("Text", "News")]
    channel: serenity::GuildChannel,
    #[description = "Duyuruda etiketlenecek rol"] rol: Option<serenity::Role>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    if let Some(problem) = cannot_send(ctx, &channel) {
        return reply_private(ctx, problem).await;
    }
    if youtube::count(&data.db, guild_id).await? >= youtube::MAX_PER_GUILD {
        return reply_private(ctx, format!("En fazla {} YouTube kanalı takip edilebilir.", youtube::MAX_PER_GUILD)).await;
    }
    ctx.defer_ephemeral().await?;
    let yt_id = match youtube::resolve_channel(&data.http_client, &source).await {
        Ok(id) => id,
        Err(e) => return reply_private(ctx, format!("❌ YouTube kanalı bulunamadı: {e}")).await,
    };
    let feed = match youtube::fetch_feed(&data.http_client, &yt_id).await {
        Ok(feed) => feed,
        Err(e) => return reply_private(ctx, format!("❌ Kanalın video listesi okunamadı: {e}")).await,
    };
    let sub = youtube::Subscription {
        yt_channel_id: yt_id,
        name: feed.channel_name.clone(),
        channel_id: channel.id,
        role_id: rol.as_ref().map(|r| r.id),
    };
    youtube::add(&data.db, guild_id, &sub, feed.videos.first().map(|v| v.id.as_str())).await?;
    let latest = feed
        .videos
        .first()
        .map_or_else(String::new, |v| format!("\nSon video: **{}**", v.title));
    reply_private(
        ctx,
        format!(
            "📺 **{}** takip ediliyor; yeni videolar {} kanalına duyurulacak (5 dakikada bir kontrol edilir).{latest}",
            feed.channel_name,
            channel.id.mention()
        ),
    )
    .await
}

/// YouTube takibini kaldırır.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn yt_sil(
    ctx: Context<'_>,
    #[rename = "ad"]
    #[description = "Kanal adı (listedeki gibi) veya UC… kimliği"]
    name: String,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let text = if youtube::remove(&ctx.data().db, guild_id, &name).await? {
        format!("🗑️ **{name}** artık takip edilmiyor.")
    } else {
        format!("**{name}** adında takip edilen kanal yok. `/youtube liste` ile bakabilirsin.")
    };
    reply_private(ctx, text).await
}

/// Takip edilen YouTube kanalları.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn yt_liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let list = youtube::list(&ctx.data().db, guild_id).await?;
    let text = if list.is_empty() {
        "Takip edilen YouTube kanalı yok. `/youtube ekle` ile ekleyebilirsin.".to_owned()
    } else {
        list.iter()
            .map(|s| {
                let role = s.role_id.map_or_else(String::new, |r| format!(" · {}", r.mention()));
                format!("• **{}** → {}{role}", s.name, s.channel_id.mention())
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    reply_private(ctx, text).await
}

// ===================== Geçici oda: ek komutlar =====================

const ROOM_MANAGE: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::CONNECT)
    .union(Permissions::SPEAK)
    .union(Permissions::MANAGE_CHANNELS)
    .union(Permissions::MOVE_MEMBERS);

/// Hedef, oda sahibinin işlem yapabileceği biri mi? Değilse açıklama döner.
fn room_target_problem(ctx: Context<'_>, target: &serenity::User) -> Option<&'static str> {
    if target.id == ctx.author().id {
        Some("Bunu kendine yapamazsın.")
    } else if target.id == ctx.serenity_context().cache.current_user().id {
        Some("Beni odamdan edemezsin. 🙂")
    } else {
        None
    }
}

/// Bir üyeyi odandan yasaklar: odadaysa çıkarılır ve tekrar giremez.
#[poise::command(slash_command, guild_only, rename = "yasakla")]
async fn oda_yasakla(ctx: Context<'_>, #[rename = "üye"] #[description = "Odaya giremeyecek üye"] user: serenity::User) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    if let Some(problem) = room_target_problem(ctx, &user) {
        return reply_private(ctx, problem).await;
    }
    let overwrite = PermissionOverwrite { allow: Permissions::empty(), deny: Permissions::CONNECT, kind: PermissionOverwriteType::Member(user.id) };
    channel.create_permission(ctx, overwrite).await?;
    let inside = ctx.guild().is_some_and(|g| g.voice_states.get(&user.id).is_some_and(|vs| vs.channel_id == Some(channel)));
    if inside {
        // Botun 'Üyeleri Taşı' izni yoksa üye odada kalır ama çıkınca geri giremez.
        let _ = guild_id.disconnect_member(ctx, user.id).await;
    }
    reply_private(ctx, format!("🚫 {} artık odana giremez.", user.id.mention())).await
}

/// Odandan yasakladığın üyenin yasağını kaldırır.
#[poise::command(slash_command, guild_only, rename = "yasağı-kaldır")]
async fn oda_yasak_kaldir(ctx: Context<'_>, #[rename = "üye"] #[description = "Yasağı kalkacak üye"] user: serenity::User) -> Result<(), Error> {
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    channel.delete_permission(ctx, PermissionOverwriteType::Member(user.id)).await?;
    reply_private(ctx, format!("✅ {} yeniden odana girebilir.", user.id.mention())).await
}

/// Bir üyeyi odandan çıkarır (tekrar girebilir).
#[poise::command(slash_command, guild_only, rename = "at")]
async fn oda_at(ctx: Context<'_>, #[rename = "üye"] #[description = "Odadan çıkarılacak üye"] user: serenity::User) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    if let Some(problem) = room_target_problem(ctx, &user) {
        return reply_private(ctx, problem).await;
    }
    let inside = ctx.guild().is_some_and(|g| g.voice_states.get(&user.id).is_some_and(|vs| vs.channel_id == Some(channel)));
    if !inside {
        return reply_private(ctx, "Bu üye odanda değil.").await;
    }
    match guild_id.disconnect_member(ctx, user.id).await {
        Ok(_) => reply_private(ctx, format!("👢 {} odadan çıkarıldı.", user.id.mention())).await,
        Err(_) => reply_private(ctx, "❌ Çıkaramadım; botun 'Üyeleri Taşı' izni olmalı.").await,
    }
}

/// Odayı gizler: kanal listesinde sadece içeridekiler ve senin izin verdiklerin görür.
#[poise::command(slash_command, guild_only, rename = "gizle")]
async fn oda_gizle(ctx: Context<'_>) -> Result<(), Error> {
    set_room_visibility(ctx, false).await
}

/// Gizlediğin odayı yeniden herkese gösterir.
#[poise::command(slash_command, guild_only, rename = "göster")]
async fn oda_goster(ctx: Context<'_>) -> Result<(), Error> {
    set_room_visibility(ctx, true).await
}

/// @everyone için "Kanalı Gör" iznini değiştirir; kilit (Bağlan yasağı) olduğu gibi korunur.
async fn set_room_visibility(ctx: Context<'_>, visible: bool) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    let everyone = PermissionOverwriteType::Role(guild_id.everyone_role());
    let locked = ctx.guild().is_some_and(|g| {
        g.channels.get(&channel).is_some_and(|c| {
            c.permission_overwrites.iter().any(|o| o.kind == everyone && o.deny.contains(Permissions::CONNECT))
        })
    });
    let mut deny = if locked { Permissions::CONNECT } else { Permissions::empty() };
    if !visible {
        deny |= Permissions::VIEW_CHANNEL;
    }
    if deny.is_empty() {
        channel.delete_permission(ctx, everyone).await?;
    } else {
        channel.create_permission(ctx, PermissionOverwrite { allow: Permissions::empty(), deny, kind: everyone }).await?;
    }
    reply_private(ctx, if visible { "👁️ Oda yeniden herkese görünür." } else { "🙈 Oda gizlendi." }).await
}

/// Odanın sahipliğini odadaki başka bir üyeye devreder.
#[poise::command(slash_command, guild_only, rename = "devret")]
async fn oda_devret(ctx: Context<'_>, #[rename = "üye"] #[description = "Yeni oda sahibi"] user: serenity::User) -> Result<(), Error> {
    let Some(channel) = my_room(ctx, true).await? else {
        return Ok(());
    };
    if user.bot || user.id == ctx.author().id {
        return reply_private(ctx, "Odayı başka bir üyeye devredebilirsin (botlara olmaz).").await;
    }
    let inside = ctx.guild().is_some_and(|g| g.voice_states.get(&user.id).is_some_and(|vs| vs.channel_id == Some(channel)));
    if !inside {
        return reply_private(ctx, "Yeni sahip odada olmalı.").await;
    }
    channel
        .create_permission(ctx, PermissionOverwrite { allow: ROOM_MANAGE, deny: Permissions::empty(), kind: PermissionOverwriteType::Member(user.id) })
        .await?;
    let _ = channel.delete_permission(ctx, PermissionOverwriteType::Member(ctx.author().id)).await;
    let data = ctx.data();
    data.tempvoice.set_owner(&data.db, channel, user.id).await?;
    ctx.say(format!("👑 {} odanın yeni sahibi.", user.id.mention())).await?;
    Ok(())
}

/// Bulunduğun geçici odanın sahibini gösterir.
#[poise::command(slash_command, guild_only, rename = "sahip")]
async fn oda_sahip(ctx: Context<'_>) -> Result<(), Error> {
    let Some(channel) = my_room(ctx, false).await? else {
        return Ok(());
    };
    match ctx.data().tempvoice.owner_of(channel) {
        Some(owner) => reply_private(ctx, format!("👑 Bu odanın sahibi {}.", owner.mention())).await,
        None => reply_private(ctx, "Bu odanın sahibi bilinmiyor.").await,
    }
}

/// Boş kalmış tüm geçici odaları siler (yöneticiler için).
#[poise::command(slash_command, guild_only, rename = "temizle", required_permissions = "MANAGE_CHANNELS")]
async fn oda_temizle(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let removed = data.tempvoice.clean(ctx.serenity_context(), &data.db, guild_id).await;
    reply_private(ctx, format!("🧹 **{removed}** boş geçici oda silindi.")).await
}
