//! Seviye komutları: `/seviye`, `/sıralama`, `/seviye-ödül`.

use anyhow::anyhow;
use poise::serenity_prelude::{self as serenity, CreateEmbed, CreateEmbedFooter, GuildId, Mentionable};
use poise::CreateReply;

use crate::data::Context;
use crate::error::Error;
use crate::leveling::settings::{roles_on_level_loss, roles_on_level_up, LevelSettings};
use crate::leveling::{formula, ExclusionKind};

const LEADERBOARD_SIZE: usize = 10;

/// Bir üyenin seviyesini ve sıralamasını gösterir.
#[poise::command(slash_command, guild_only, category = "Seviye")]
pub async fn seviye(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Bakılacak kullanıcı (boşsa sen)"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let data = ctx.data();

    let rank = data.leveling.rank(&data.db, &data.redis, guild_id, user.id).await?;
    let progress = formula::progress(rank.total_xp);

    // Önce resimli rütbe kartı denenir; çizilemezse eski yazılı görünüme dönülür.
    ctx.defer().await?;
    // Üyenin kendi kart ayarı (renk, arka plan, yazı tipi); okunamazsa varsayılan.
    let personal = crate::rankcard::get(&data.db, guild_id, user.id).await.unwrap_or_default();
    let card = crate::cards::RankCard {
        name: user.global_name.clone().unwrap_or_else(|| user.name.clone()),
        level: progress.level,
        rank: rank.position,
        current: progress.current,
        needed: progress.needed,
        total: rank.total_xp,
        accent: personal.accent.as_deref().and_then(crate::cards::parse_hex).unwrap_or([88, 101, 242]),
        font: personal.font.clone().unwrap_or_else(|| "noto".to_owned()),
    };
    let backdrop = async {
        match personal.background.as_deref() {
            Some(url) => crate::cards::fetch_image(url).await,
            None => None,
        }
    };
    let (avatar, backdrop) = tokio::join!(crate::members::avatar_image(&user), backdrop);
    let drawn = tokio::task::spawn_blocking(move || crate::cards::render_rank(&card, avatar.as_ref(), backdrop.as_ref())).await;
    if let Ok(Ok(bytes)) = drawn {
        let file = serenity::CreateAttachment::bytes(bytes, "seviye.jpg");
        ctx.send(CreateReply::default().attachment(file)).await?;
        return Ok(());
    }

    let position = rank
        .position
        .map_or_else(|| "—".to_owned(), |p| format!("#{p}"));
    let embed = CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
        .colour(serenity::Colour::BLURPLE)
        .field("Seviye", format!("**{}**", progress.level), true)
        .field("Sıra", position, true)
        .field("Toplam XP", rank.total_xp.to_string(), true)
        .field(
            format!("İlerleme ({} / {} XP)", progress.current, progress.needed),
            formula::bar(progress, 16),
            false,
        );

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}


/// Sunucudaki en yüksek seviyeli 10 üyeyi gösterir.
#[poise::command(slash_command, guild_only, rename = "sıralama", category = "Seviye")]
pub async fn siralama(
    ctx: Context<'_>,
    #[rename = "dönem"]
    #[description = "Hangi dönem? (boşsa tüm zamanlar)"]
    period: Option<crate::leveling::Period>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    if let Some(period) = period {
        // Dönemlik sıralama: yalnızca o hafta / ay mesajla kazanılan XP; yeni gelenler de yarışabilir.
        let top = data.leveling.top_period(&data.redis, guild_id, period, LEADERBOARD_SIZE).await?;
        let label = if period == crate::leveling::Period::Week { "Bu hafta" } else { "Bu ay" };
        let description = if top.is_empty() {
            "Bu dönemde henüz kimse XP kazanmadı. 💬".to_owned()
        } else {
            top.iter().enumerate().map(|(i, (user, xp))| format!("`#{}` {} — **{xp}** XP", i + 1, user.mention())).collect::<Vec<_>>().join("\n")
        };
        let embed = CreateEmbed::new()
            .title(format!("🏆 {label} — Sıralama"))
            .colour(serenity::Colour::GOLD)
            .description(description)
            .footer(CreateEmbedFooter::new("Haftalık sıralama her Pazartesi, aylık sıralama her ayın 1'inde sıfırlanır."));
        ctx.send(CreateReply::default().embed(embed).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
        return Ok(());
    }
    let top = data
        .leveling
        .top(&data.db, &data.redis, guild_id, LEADERBOARD_SIZE)
        .await?;

    let description = if top.is_empty() {
        "Henüz kimse XP kazanmadı. Sohbet etmeye başlayın! 💬".to_owned()
    } else {
        top.iter()
            .enumerate()
            .map(|(i, (user, xp))| {
                let medal = match i {
                    0 => "🥇".to_owned(),
                    1 => "🥈".to_owned(),
                    2 => "🥉".to_owned(),
                    n => format!("`#{}`", n + 1),
                };
                format!(
                    "{medal} {} — Seviye **{}** ({xp} XP)",
                    user.mention(),
                    formula::level(*xp)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let guild_name = ctx
        .guild()
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "Sunucu".to_owned());
    let mut embed = CreateEmbed::new()
        .title(format!("🏆 {guild_name} — Sıralama"))
        .colour(serenity::Colour::GOLD)
        .description(description)
        .footer(CreateEmbedFooter::new("Her dakika ilk mesajın 15-25 XP kazandırır."));
    // Sıralama sayfası herkese açıksa ve panelin dış adresi varsa tam liste bağlantısı eklenir.
    let public = data.settings.get::<LevelSettings>(&data.db, guild_id).await.is_ok_and(|s| s.public_leaderboard);
    if let (true, Some(base)) = (public, data.public_url.public()) {
        embed = embed.url(format!("{base}/siralama/{guild_id}"));
    }

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Seviye ödül rolleri.
#[poise::command(
    slash_command,
    guild_only,
    rename = "seviye-ödül",
    default_member_permissions = "MANAGE_ROLES",
    subcommands("odul_ekle", "odul_sil", "odul_liste"),
    subcommand_required,
    category = "Seviye"
)]
pub async fn seviye_odul(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bir seviyeye ulaşınca verilecek rolü ayarlar.
#[poise::command(slash_command, guild_only, rename = "ekle", required_bot_permissions = "MANAGE_ROLES")]
async fn odul_ekle(
    ctx: Context<'_>,
    #[description = "Gereken seviye"]
    #[min = 1]
    #[max = 500]
    seviye: u32,
    #[description = "Verilecek rol"] rol: serenity::Role,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    if rol.managed {
        return reply(ctx, "❌ Bu rol bir entegrasyona ait, elle verilemez.".to_owned()).await;
    }
    if rol.id == guild_id.everyone_role() {
        return reply(ctx, "❌ @everyone ödül olarak verilemez.".to_owned()).await;
    }

    let data = ctx.data();
    data.leveling.set_reward(&data.db, guild_id, seviye, rol.id).await?;
    reply(
        ctx,
        format!(
            "✅ **{seviye}. seviyeye** ulaşanlara {} rolü verilecek.\n\
             _Rolümün bu rolün üstünde olduğundan emin ol._",
            rol.id.mention()
        ),
    )
    .await
}

/// Bir seviyenin ödül rolünü kaldırır.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn odul_sil(
    ctx: Context<'_>,
    #[description = "Ödülü kaldırılacak seviye"]
    #[min = 1]
    seviye: u32,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let text = if data.leveling.remove_reward(&data.db, guild_id, seviye).await? {
        format!("✅ {seviye}. seviye ödülü kaldırıldı.")
    } else {
        format!("ℹ️ {seviye}. seviye için bir ödül yok.")
    };
    reply(ctx, text).await
}

/// Tüm seviye ödüllerini listeler.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn odul_liste(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let rewards = data.leveling.rewards(&data.db, guild_id).await?;

    let text = if rewards.is_empty() {
        "Henüz seviye ödülü yok. `/seviye-ödül ekle` ile ekleyebilirsin.".to_owned()
    } else {
        rewards
            .iter()
            .map(|r| format!("Seviye **{}** → {}", r.level, r.role_id.mention()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    reply(ctx, text).await
}

/// XP kazanılmayacak kanal/kategori veya rolü ekler; tekrar verirsen kaldırır. Boş bırakırsan listeler.
#[poise::command(
    slash_command,
    guild_only,
    rename = "xp-muaf",
    default_member_permissions = "MANAGE_GUILD",
    category = "Seviye"
)]
pub async fn xp_muaf(
    ctx: Context<'_>,
    #[description = "XP kazanılmayacak kanal veya kategori"]
    #[channel_types("Text", "Category", "Voice", "Forum", "News")]
    kanal: Option<serenity::GuildChannel>,
    #[description = "XP kazanamayacak rol"] rol: Option<serenity::Role>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let mut lines = Vec::new();
    if let Some(channel) = &kanal {
        let added = data
            .leveling
            .toggle_exclusion(&data.db, guild_id, channel.id.get(), ExclusionKind::Channel)
            .await?;
        lines.push(if added {
            format!("🚫 {} artık XP kazandırmıyor.", channel.id.mention())
        } else {
            format!("✅ {} yeniden XP kazandırıyor.", channel.id.mention())
        });
    }
    if let Some(role) = &rol {
        let added = data
            .leveling
            .toggle_exclusion(&data.db, guild_id, role.id.get(), ExclusionKind::Role)
            .await?;
        lines.push(if added {
            format!("🚫 {} rolündekiler artık XP kazanamıyor.", role.id.mention())
        } else {
            format!("✅ {} rolündekiler yeniden XP kazanıyor.", role.id.mention())
        });
    }

    let exclusions = data.leveling.exclusions(&data.db, guild_id).await?;
    let mut list: Vec<String> = exclusions
        .iter()
        .map(|(id, kind)| match kind {
            ExclusionKind::Channel => format!("• Kanal: <#{id}>"),
            ExclusionKind::Role => format!("• Rol: <@&{id}>"),
        })
        .collect();
    list.sort();
    if list.is_empty() {
        lines.push("_XP muafiyeti yok; her kanalda herkes XP kazanır._".to_owned());
    } else {
        lines.push(format!("**XP kazandırmayanlar:**\n{}", list.join("\n")));
    }
    lines.push("_Aynı kanal/rolü tekrar verirsen muafiyet kalkar._".to_owned());
    reply(ctx, lines.join("\n")).await
}

fn guild_id(ctx: Context<'_>) -> Result<GuildId, Error> {
    ctx.guild_id()
        .ok_or_else(|| anyhow!("guild_only komut sunucu dışında çalıştı"))
}

async fn reply(ctx: Context<'_>, content: String) -> Result<(), Error> {
    ctx.send(CreateReply::default().content(content).ephemeral(true))
        .await?;
    Ok(())
}

/// Üyelere XP ver veya XP'lerini azalt.
#[poise::command(
    slash_command,
    guild_only,
    rename = "xp",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("xp_ver", "xp_al"),
    subcommand_required,
    category = "Seviye"
)]
pub async fn xp(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bir üyeye XP verir.
#[poise::command(slash_command, guild_only, rename = "ver")]
async fn xp_ver(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "XP verilecek üye"]
    user: serenity::User,
    #[description = "Verilecek XP"]
    #[min = 1]
    #[max = 1_000_000]
    miktar: u32,
) -> Result<(), Error> {
    change_xp(ctx, user, i64::from(miktar)).await
}

/// Bir üyenin XP'sini azaltır.
#[poise::command(slash_command, guild_only, rename = "al")]
async fn xp_al(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "XP'si azaltılacak üye"]
    user: serenity::User,
    #[description = "Alınacak XP"]
    #[min = 1]
    #[max = 1_000_000]
    miktar: u32,
) -> Result<(), Error> {
    change_xp(ctx, user, -i64::from(miktar)).await
}

/// XP'yi değiştirir ve seviye değiştiyse ödül rollerini ayarlara göre verir / geri alır.
async fn change_xp(ctx: Context<'_>, user: serenity::User, delta: i64) -> Result<(), Error> {
    if user.bot {
        return reply(ctx, "Botların XP'si olmaz.".to_owned()).await;
    }
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    ctx.defer_ephemeral().await?;

    let (old_xp, new_xp) = data.leveling.add_xp(&data.db, &data.redis, guild_id, user.id, delta).await?;
    let (old, new) = (formula::level(old_xp), formula::level(new_xp));
    let settings = data.settings.get::<LevelSettings>(&data.db, guild_id).await?;
    let rewards = data.leveling.rewards(&data.db, guild_id).await?;

    let (grant, revoke) = if new > old {
        roles_on_level_up(&rewards, old, new, settings.stack_rewards)
    } else if new < old && settings.remove_on_loss {
        (Vec::new(), roles_on_level_loss(&rewards, new))
    } else {
        (Vec::new(), Vec::new())
    };
    let mut notes = Vec::new();
    for role in grant {
        match ctx.http().add_member_role(guild_id, user.id, role, Some("Seviye ödülü")).await {
            Ok(()) => notes.push(format!("➕ {}", role.mention())),
            Err(e) => notes.push(format!("⚠️ {} verilemedi ({e})", role.mention())),
        }
    }
    for role in revoke {
        // Üyede olmayan rolü almak hata değildir; Discord sessizce kabul eder.
        match ctx.http().remove_member_role(guild_id, user.id, role, Some("Seviye düştü")).await {
            Ok(()) => notes.push(format!("➖ {}", role.mention())),
            Err(e) => notes.push(format!("⚠️ {} alınamadı ({e})", role.mention())),
        }
    }

    let mut text = format!(
        "{} {} için XP: **{old_xp} → {new_xp}** (seviye {old} → {new}).",
        if delta >= 0 { "✅" } else { "📉" },
        user.id.mention()
    );
    if !notes.is_empty() {
        text.push_str(&format!("\nÖdül rolleri: {}", notes.join(", ")));
    }
    reply(ctx, text).await
}

/// Bir üyenin kazandığı başarımları ve ilerlemesini gösterir.
#[poise::command(slash_command, guild_only, rename = "başarımlar", category = "Seviye")]
pub async fn basarimlar(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Bakılacak kullanıcı (boşsa sen)"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    use crate::achievements::{self, AchievementSettings, Kind};

    let guild_id = guild_id(ctx)?;
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let data = ctx.data();
    let settings = data.settings.get::<AchievementSettings>(&data.db, guild_id).await?;
    if !settings.enabled {
        return reply(ctx, "Bu sunucuda başarımlar kapalı. Yöneticiler panelden açabilir.".to_owned()).await;
    }
    let (counters, unlocked) = achievements::progress(data, guild_id, user.id).await?;
    let definitions = settings.definitions();

    let mut embed = CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(format!("{} — başarımlar", user.tag())).icon_url(user.face()))
        .colour(serenity::Colour::GOLD)
        .footer(CreateEmbedFooter::new(format!(
            "{} / {} başarım kazanıldı",
            definitions.iter().filter(|d| unlocked.contains(&d.key)).count(),
            definitions.len()
        )));
    for (kind, _, label) in Kind::ALL {
        let count = counters.get(&kind).copied().unwrap_or(0);
        let lines: Vec<String> = definitions
            .iter()
            .filter(|d| d.kind == kind)
            .map(|d| {
                if unlocked.contains(&d.key) {
                    format!("✅ **{}** — {}", d.name, d.threshold)
                } else {
                    format!("▫️ {} — {} / {}", d.name, count.min(d.threshold), d.threshold)
                }
            })
            .collect();
        if !lines.is_empty() {
            let mut title = label.to_owned();
            if let Some(first) = title.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            embed = embed.field(format!("{title}: {count}"), lines.join("\n").chars().take(1000).collect::<String>(), false);
        }
    }
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}
