//! Sunucu çapında soygun: `/soy` komutu üye verilmeden çalıştırılınca başlar. Üyeler butonla ekibe katılır,
//! herkes giriş payını yatırır; ekip büyüdükçe başarı ihtimali artar, ganimet eşit paylaşılır.

use std::time::{Duration, Instant};

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteractionCollector, CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, UserId,
};
use poise::CreateReply;

use super::moderation::reply_private;
use crate::data::Context;
use crate::economy::{self, money};
use crate::error::Error;

/// Katılmak için yatırılan pay (başarısız olursa yanar).
pub const STAKE: i64 = 200;
const LOBBY: Duration = Duration::from_secs(45);
const MAX_CREW: usize = 10;
/// Aynı sunucuda iki soygun arasında beklenecek süre.
const COOLDOWN_SECS: u64 = 30 * 60;

/// Ekip büyüklüğüne göre başarı ihtimali (yüzde): tek başına %35, her ek kişi +%8, en fazla %75.
pub fn success_chance(crew: usize) -> u64 {
    (35 + 8 * crew.saturating_sub(1) as u64).min(75)
}

/// Başarılı soygunda kişi başına düşen pay: yatırılan pay geri gelir + ganimet eşit bölünür.
/// `bonus_roll`: 0..=99 arası şans (ganimete %0-99 ek).
pub fn share(crew: usize, bonus_roll: u64) -> i64 {
    let crew = i64::try_from(crew.max(1)).unwrap_or(1);
    let loot = 500 * crew + 500 * crew * i64::try_from(bonus_roll.min(99)).unwrap_or(0) / 100;
    STAKE + loot / crew
}

pub async fn run(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let data = ctx.data();
    // Sunucu başına bekleme (NX: aynı anda iki soygun başlayamaz).
    let key = format!("eko:soygun:{guild_id}");
    let mut conn = data.redis.clone();
    let claimed: Option<String> = redis::cmd("SET").arg(&key).arg(1).arg("EX").arg(COOLDOWN_SECS).arg("NX").query_async(&mut conn).await?;
    if claimed.is_none() {
        let left: i64 = redis::cmd("TTL").arg(&key).query_async(&mut conn).await.unwrap_or(0);
        return reply_private(ctx, format!("🚓 Banka alarmda; **{}** sonra yeni bir soygun planlanabilir.", economy::format_wait(Duration::from_secs(u64::try_from(left).unwrap_or(0))))).await;
    }
    // Başlatan ilk üyedir; payını yatıramıyorsa soygun başlamaz ve bekleme geri alınır.
    if economy::spend(&data.db, guild_id, ctx.author().id, STAKE).await?.is_none() {
        let _: redis::RedisResult<()> = redis::cmd("DEL").arg(&key).query_async(&mut conn).await;
        return reply_private(ctx, format!("Soygun için cüzdanında en az {} olmalı (giriş payı).", money(STAKE))).await;
    }

    let id = ctx.id();
    let button = |disabled: bool| vec![CreateActionRow::Buttons(vec![CreateButton::new(format!("soygun:{id}")).label("Ekibe katıl").emoji('🦹').style(ButtonStyle::Danger).disabled(disabled)])];
    let ends = serenity::Timestamp::now().unix_timestamp() + i64::try_from(LOBBY.as_secs()).unwrap_or(45);
    let lobby = |crew: &[UserId]| {
        CreateEmbed::new()
            .title("🏦 Banka soygunu planlanıyor!")
            .description(format!(
                "Giriş payı **{}** (başarısız olursa yanar). Ekip büyüdükçe başarı ihtimali artar.\n\n**Ekip ({}):** {}\nBaşarı ihtimali: **%{}**\n\n⏳ Başlangıç: <t:{ends}:R>",
                money(STAKE),
                crew.len(),
                crew.iter().map(|u| format!("<@{u}>")).collect::<Vec<_>>().join(" "),
                success_chance(crew.len())
            ))
            .colour(serenity::Colour::DARK_RED)
    };
    let mut crew = vec![ctx.author().id];
    let reply = ctx.send(CreateReply::default().embed(lobby(&crew)).components(button(false)).allowed_mentions(CreateAllowedMentions::new())).await?;
    let message = reply.message().await?.id;

    let deadline = Instant::now() + LOBBY;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || crew.len() >= MAX_CREW {
            break;
        }
        let Some(press) = ComponentInteractionCollector::new(ctx.serenity_context()).message_id(message).timeout(left).await else {
            break;
        };
        let note = |text: String| CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true));
        if crew.contains(&press.user.id) {
            // Bir butona cevap verilemezse soygun bitmez (yoksa ekibin payları yanardı).
            let _ = press.create_response(ctx, note("Zaten ekiptesin.".to_owned())).await;
            continue;
        }
        if press.user.bot || economy::spend(&data.db, guild_id, press.user.id, STAKE).await?.is_none() {
            let _ = press.create_response(ctx, note(format!("Katılmak için cüzdanında {} olmalı.", money(STAKE)))).await;
            continue;
        }
        crew.push(press.user.id);
        let _ = press.create_response(ctx, CreateInteractionResponse::UpdateMessage(CreateInteractionResponseMessage::new().embed(lobby(&crew)).components(button(false)))).await;
    }

    let succeeded = crate::rand_util::below(100) < success_chance(crew.len());
    let names = crew.iter().map(|u| format!("<@{u}>")).collect::<Vec<_>>().join(" ");
    let result = if succeeded {
        let each = share(crew.len(), crate::rand_util::below(100));
        for user in &crew {
            economy::add(&data.db, guild_id, *user, each).await?;
        }
        CreateEmbed::new()
            .title("💰 Soygun başarılı!")
            .description(format!("{names}\n\nKasa boşaltıldı! Herkes **{}** aldı (giriş payı dahil).", money(each)))
            .colour(serenity::Colour::DARK_GREEN)
    } else {
        CreateEmbed::new()
            .title("🚓 Soygun başarısız!")
            .description(format!("{names}\n\nAlarm çaldı, ekip dağıldı. Herkes **{}** giriş payını kaybetti.", money(STAKE)))
            .colour(serenity::Colour::DARK_GREY)
    };
    reply.edit(ctx, CreateReply::default().embed(result).components(button(true)).allowed_mentions(CreateAllowedMentions::new())).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigger_crews_have_better_odds_up_to_a_cap() {
        assert_eq!(success_chance(1), 35);
        assert_eq!(success_chance(2), 43);
        assert!(success_chance(4) > success_chance(3));
        assert_eq!(success_chance(10), 75);
        assert_eq!(success_chance(0), 35);
    }

    #[test]
    fn winners_always_profit_and_expected_value_is_fair() {
        // Kazanan her zaman payından fazlasını alır.
        for crew in 1..=MAX_CREW {
            assert!(share(crew, 0) > STAKE);
            assert_eq!(share(crew, 0), STAKE + 500);
            assert_eq!(share(crew, 99), STAKE + 500 + 495);
        }
        // Tek başına soygunun beklenen kazancı pay tutarını çok aşmaz (para basma makinesi olmasın).
        let expected = share(1, 50) * i64::try_from(success_chance(1)).unwrap_or(0) / 100;
        assert!(expected < STAKE * 2, "{expected}");
    }
}
