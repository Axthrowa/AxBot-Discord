//! `/rol` üst komutu: otomatik rol, rol paneli, rol bilgisi ve süreli rol.
//!
//! Süreli rol zamanlanmış görevle (`Task::TempRoleEnd`) geri alınır; bot kapalıyken süresi dolanlar açılışta alınır.

use std::time::Duration;

use poise::serenity_prelude::{self as serenity, Mentionable, Role};

use super::moderation::{guild_id, reply_private};
use crate::data::Context;
use crate::error::Error;
use crate::moderation::duration;
use crate::rolepanel;
use crate::scheduler::Task;

/// En uzun süreli rol: 1 yıl.
const MAX_TEMP_ROLE: Duration = Duration::from_secs(365 * 24 * 3600);
/// Bir sunucudaki bekleyen süreli rol sınırı.
const MAX_PENDING: i64 = 500;

/// Rol araçları.
#[poise::command(
    slash_command,
    guild_only,
    rename = "rol",
    subcommands(
        "super::community::otorol",
        "super::community::rol_paneli",
        "super::utility::rol_bilgi",
        "sureli_ver",
        "sureli_al",
        "sureliler",
        "toplu"
    ),
    subcommand_required,
    category = "Topluluk"
)]
pub async fn rol(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Komutu kullanan bu rolü verebilir mi? (Discord'daki gibi: kendi en yüksek rolünün altındakiler)
fn author_outranks(ctx: Context<'_>, member: &serenity::Member, role: &Role) -> bool {
    let Some(guild) = ctx.guild() else {
        return false;
    };
    if guild.owner_id == member.user.id {
        return true;
    }
    let top = member.roles.iter().filter_map(|r| guild.roles.get(r)).map(|r| r.position).max().unwrap_or(0);
    role.position < top
}

/// Bir üyeye belirli bir süreliğine rol verir; süre dolunca rol kendiliğinden alınır.
#[poise::command(
    slash_command,
    guild_only,
    rename = "süreli-ver",
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES"
)]
async fn sureli_ver(
    ctx: Context<'_>,
    #[description = "Rolü alacak üye"] üye: serenity::Member,
    #[description = "Verilecek rol"] rol: Role,
    #[rename = "süre"]
    #[description = "Ne kadar süre (örn. 2sa, 3g, 1h)"]
    length: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let length = match duration::parse(&length) {
        Ok(d) if d > MAX_TEMP_ROLE => return reply_private(ctx, "❌ Süreli rol en fazla 1 yıl olabilir.").await,
        Ok(d) if d < Duration::from_secs(60) => return reply_private(ctx, "❌ Süre en az 1 dakika olmalı.").await,
        Ok(d) => d,
        Err(e) => return reply_private(ctx, format!("❌ Geçersiz süre: {e}")).await,
    };
    if let Err(reason) = rolepanel::check_role(ctx.serenity_context(), guild, &rol) {
        return reply_private(ctx, format!("❌ {reason}")).await;
    }
    let Some(author) = ctx.author_member().await else {
        return reply_private(ctx, "❌ Üyelik bilgin alınamadı, tekrar dene.").await;
    };
    if !author_outranks(ctx, &author, &rol) {
        return reply_private(ctx, format!("❌ {} rolü senin en yüksek rolünün üstünde ya da ona eşit; veremezsin.", rol.id.mention())).await;
    }
    let data = ctx.data();
    let pending = data.scheduler.pending_temp_roles(guild).await?;
    if pending >= MAX_PENDING {
        return reply_private(ctx, format!("❌ Bu sunucuda zaten {MAX_PENDING} süreli rol bekliyor.")).await;
    }
    ctx.defer_ephemeral().await?;
    ctx.http().add_member_role(guild, üye.user.id, rol.id, Some(&format!("Süreli rol ({}) — {}", duration::format(length), ctx.author().name))).await?;
    // Aynı üye-rol için eski görev varsa yenisi onun yerini alır (süre uzatma / kısaltma).
    data.scheduler.cancel_temp_role(guild, üye.user.id, rol.id).await?;
    let ends = chrono::Utc::now().timestamp() + i64::try_from(length.as_secs())?;
    data.scheduler
        .schedule(ends, &Task::TempRoleEnd { guild_id: guild.get(), user_id: üye.user.id.get(), role_id: rol.id.get() })
        .await?;
    reply_private(ctx, format!("✅ {} üyesine {} rolü verildi; <t:{ends}:R> geri alınacak.", üye.user.id.mention(), rol.id.mention())).await
}

/// Süreli bir rolü süresini beklemeden hemen geri alır.
#[poise::command(
    slash_command,
    guild_only,
    rename = "süreli-al",
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES"
)]
async fn sureli_al(
    ctx: Context<'_>,
    #[description = "Üye"] üye: serenity::Member,
    #[description = "Geri alınacak rol"] rol: Role,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let Some(author) = ctx.author_member().await else {
        return reply_private(ctx, "❌ Üyelik bilgin alınamadı, tekrar dene.").await;
    };
    if !author_outranks(ctx, &author, &rol) {
        return reply_private(ctx, format!("❌ {} rolü senin en yüksek rolünün üstünde ya da ona eşit.", rol.id.mention())).await;
    }
    let removed = ctx.data().scheduler.cancel_temp_role(guild, üye.user.id, rol.id).await?;
    if removed == 0 {
        return reply_private(ctx, "❌ Bu üyede bu rol süreli olarak verilmemiş.").await;
    }
    ctx.http().remove_member_role(guild, üye.user.id, rol.id, Some("Süreli rol erken geri alındı")).await?;
    reply_private(ctx, format!("✅ {} üyesinden {} rolü alındı.", üye.user.id.mention(), rol.id.mention())).await
}

/// Bekleyen süreli rolleri listeler.
#[poise::command(slash_command, guild_only, rename = "süreliler", required_permissions = "MANAGE_ROLES")]
async fn sureliler(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let list = ctx.data().scheduler.temp_roles(guild, 25).await?;
    if list.is_empty() {
        return reply_private(ctx, "Bekleyen süreli rol yok. Vermek için: `/rol süreli-ver`.").await;
    }
    let lines: Vec<String> = list
        .iter()
        .map(|(user, role, ends)| format!("• <@{user}> — <@&{role}> · <t:{ends}:R> bitiyor"))
        .collect();
    reply_private(ctx, format!("**⏳ Süreli roller** (ilk {})\n{}", list.len(), lines.join("\n"))).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum BulkAction {
    #[name = "Ver"]
    Give,
    #[name = "Al"]
    Take,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum BulkTarget {
    #[name = "Herkes"]
    Everyone,
    #[name = "Yalnızca insanlar"]
    Humans,
    #[name = "Yalnızca botlar"]
    Bots,
    #[name = "Filtre rolüne sahip olanlar"]
    WithRole,
}

/// İşlemin bu üyeye uygulanıp uygulanmayacağı (rol zaten istenen durumdaysa atlanır).
pub fn bulk_applies(action: BulkAction, target: BulkTarget, is_bot: bool, has_role: bool, has_filter: bool) -> bool {
    let chosen = match target {
        BulkTarget::Everyone => true,
        BulkTarget::Humans => !is_bot,
        BulkTarget::Bots => is_bot,
        BulkTarget::WithRole => has_filter,
    };
    chosen && (action == BulkAction::Give) != has_role
}

/// Aynı sunucuda aynı anda tek toplu işlem.
static BULK_RUNNING: std::sync::LazyLock<dashmap::DashSet<serenity::GuildId>> = std::sync::LazyLock::new(dashmap::DashSet::new);

/// Bir rolü çok sayıda üyeye toplu verir ya da alır (ör. otorolü eski üyelere vermek).
#[poise::command(
    slash_command,
    guild_only,
    rename = "toplu",
    required_permissions = "MANAGE_ROLES",
    required_bot_permissions = "MANAGE_ROLES",
    user_cooldown = 30
)]
async fn toplu(
    ctx: Context<'_>,
    #[rename = "işlem"]
    #[description = "Rolü ver mi, al mı?"]
    action: BulkAction,
    #[description = "Rol"] rol: Role,
    #[description = "Kimlere?"] hedef: BulkTarget,
    #[rename = "filtre-rol"]
    #[description = "Hedef 'Filtre rolüne sahip olanlar' ise bu role sahip üyeler"]
    filtre: Option<Role>,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    if hedef == BulkTarget::WithRole && filtre.is_none() {
        return reply_private(ctx, "❌ Bu hedef için `filtre-rol` seçmelisin.").await;
    }
    if let Err(reason) = rolepanel::check_role(ctx.serenity_context(), guild, &rol) {
        return reply_private(ctx, format!("❌ {reason}")).await;
    }
    let Some(author) = ctx.author_member().await else {
        return reply_private(ctx, "❌ Üyelik bilgin alınamadı, tekrar dene.").await;
    };
    if !author_outranks(ctx, &author, &rol) {
        return reply_private(ctx, format!("❌ {} rolü senin en yüksek rolünün üstünde ya da ona eşit.", rol.id.mention())).await;
    }
    if !BULK_RUNNING.insert(guild) {
        return reply_private(ctx, "⏳ Bu sunucuda zaten bir toplu rol işlemi sürüyor; bitince tekrar dene.").await;
    }
    reply_private(ctx, "⏳ Toplu işlem başladı. Üye sayısına göre birkaç dakika sürebilir; bitince bu kanala yazacağım.").await?;

    let http = ctx.serenity_context().http.clone();
    let (channel, author_id, role_id, filter) = (ctx.channel_id(), ctx.author().id, rol.id, filtre.map(|r| r.id));
    let reason = format!("Toplu rol işlemi — {}", ctx.author().name);
    tokio::spawn(async move {
        use poise::futures_util::StreamExt;
        let (mut changed, mut failed) = (0u32, 0u32);
        let mut members = guild.members_iter(&http).boxed();
        while let Some(member) = members.next().await {
            let Ok(member) = member else {
                failed += 1;
                break;
            };
            let has_filter = filter.is_some_and(|f| member.roles.contains(&f));
            if !bulk_applies(action, hedef, member.user.bot, member.roles.contains(&role_id), has_filter) {
                continue;
            }
            // Discord'un hız sınırlarını serenity bekletir; istekler sırayla gider.
            let result = match action {
                BulkAction::Give => http.add_member_role(guild, member.user.id, role_id, Some(&reason)).await,
                BulkAction::Take => http.remove_member_role(guild, member.user.id, role_id, Some(&reason)).await,
            };
            match result {
                Ok(()) => changed += 1,
                Err(_) => failed += 1,
            }
        }
        BULK_RUNNING.remove(&guild);
        let verb = if action == BulkAction::Give { "verildi" } else { "alındı" };
        let mut text = format!("{} toplu rol işlemi bitti: {} rolü **{changed}** üyeye {verb}.", author_id.mention(), role_id.mention());
        if failed > 0 {
            text.push_str(&format!(" {failed} üyede hata oldu (botun rol sırası / izinleri)."));
        }
        let message = serenity::CreateMessage::new().content(text).allowed_mentions(serenity::CreateAllowedMentions::new().users(vec![author_id]));
        if let Err(e) = channel.send_message(&http, message).await {
            tracing::warn!(guild = %guild, error = %e, "Toplu rol sonucu gönderilemedi");
        }
    });
    Ok(())
}

#[cfg(test)]
mod bulk_tests {
    use super::*;

    #[test]
    fn bulk_skips_members_already_in_the_wanted_state() {
        use BulkAction::*;
        use BulkTarget::*;
        assert!(bulk_applies(Give, Everyone, false, false, false));
        assert!(!bulk_applies(Give, Everyone, false, true, false));
        assert!(bulk_applies(Take, Everyone, true, true, false));
        assert!(!bulk_applies(Give, Humans, true, false, false));
        assert!(bulk_applies(Give, Bots, true, false, false));
        assert!(!bulk_applies(Give, WithRole, false, false, false));
        assert!(bulk_applies(Give, WithRole, false, false, true));
    }
}
