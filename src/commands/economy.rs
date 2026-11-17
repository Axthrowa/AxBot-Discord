//! Ekonomi komutları: bakiye, günlük, çalış, banka, gönder, soygun, şans oyunları, mağaza.

use std::time::Duration;

use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteractionCollector, CreateActionRow, CreateButton,
    CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
    GuildId, Mentionable,
};
use poise::CreateReply;

use super::moderation::{guild_id, reply_private};
use crate::data::Context;
use crate::economy::games::{self, Card, Outcome};
use crate::economy::{self, money, Claim, Rob, Work, MAX_BET, MIN_BET};
use crate::error::Error;

const COLOUR: serenity::Colour = serenity::Colour::from_rgb(241, 196, 15);
const BLACKJACK_TIMEOUT: Duration = Duration::from_secs(60);

fn embed(title: impl Into<String>, description: impl Into<String>) -> CreateEmbed {
    CreateEmbed::new().title(title).description(description).colour(COLOUR)
}

async fn send(ctx: Context<'_>, embed: CreateEmbed) -> Result<(), Error> {
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Bahsi okur, sınırları kontrol eder ve cüzdandan düşer. Olmadıysa kullanıcıya söyler.
async fn take_bet(ctx: Context<'_>, guild: GuildId, input: &str) -> Result<Option<i64>, Error> {
    let db = &ctx.data().db;
    let wallet = economy::account(db, guild, ctx.author().id).await?.wallet;
    let available = wallet.min(i64::try_from(MAX_BET)?);
    let bet = match economy::parse_amount(input, available) {
        Ok(bet) => bet,
        Err(message) => {
            reply_private(ctx, format!("❌ {message}")).await?;
            return Ok(None);
        }
    };
    if bet < i64::try_from(MIN_BET)? || bet > i64::try_from(MAX_BET)? {
        reply_private(ctx, format!("❌ Bahis {} ile {} arasında olmalı.", money(MIN_BET as i64), money(MAX_BET as i64))).await?;
        return Ok(None);
    }
    if economy::spend(db, guild, ctx.author().id, bet).await?.is_none() {
        reply_private(ctx, format!("❌ Cüzdanında yeterli para yok (cüzdan: {}).", money(wallet))).await?;
        return Ok(None);
    }
    Ok(Some(bet))
}

// ===================== Temel =====================

/// Cüzdan ve banka bakiyesi.
#[poise::command(slash_command, guild_only, category = "Ekonomi")]
pub async fn bakiye(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Bakılacak üye (boşsa sen)"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let acc = economy::account(&ctx.data().db, guild, user.id).await?;
    let e = CreateEmbed::new()
        .author(serenity::CreateEmbedAuthor::new(user.tag()).icon_url(user.face()))
        .colour(COLOUR)
        .field("👛 Cüzdan", money(acc.wallet), true)
        .field("🏦 Banka", money(acc.bank), true)
        .field("💰 Toplam", money(acc.wallet + acc.bank), true)
        .footer(CreateEmbedFooter::new("Bankadaki para soyulamaz. /ekonomi günlük ve /ekonomi çalış ile para kazan."));
    send(ctx, e).await
}

/// Günlük ödülünü al (her gün alırsan seri bonusu artar).
#[poise::command(slash_command, guild_only, rename = "günlük", category = "Ekonomi")]
pub async fn gunluk(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    match economy::claim_daily(&ctx.data().db, guild, ctx.author().id).await? {
        Claim::Ok { amount, wallet, streak } => {
            let streak_text = if streak > 1 { format!("\n🔥 **{streak} günlük seri!** Yarın da gelirsen bonus artar.") } else { String::new() };
            send(ctx, embed("🎁 Günlük ödül", format!("**{}** kazandın!{streak_text}\nCüzdan: {}", money(amount), money(wallet)))).await
        }
        Claim::Wait(wait) => reply_private(ctx, format!("⏳ Günlük ödülünü zaten aldın. **{}** sonra tekrar gel.", economy::format_wait(wait))).await,
    }
}

/// Çalışıp para kazan: mesai (saatte bir), balık tutma ya da madencilik (20 dakikada bir).
#[poise::command(slash_command, guild_only, rename = "çalış", category = "Ekonomi")]
pub async fn calis(
    ctx: Context<'_>,
    #[rename = "tür"]
    #[description = "Ne yapacaksın? (boşsa mesai)"]
    job: Option<economy::gather::Job>,
) -> Result<(), Error> {
    use economy::gather::{self, Job, Outcome};
    let guild = guild_id(ctx)?;
    if let Some(job @ (Job::Fishing | Job::Mining)) = job {
        let data = ctx.data();
        let (title, verb) = if job == Job::Fishing { ("🎣 Balık tutma", "Oltana") } else { ("⛏️ Madencilik", "Kazmana") };
        return match gather::gather(&data.db, &data.redis, guild, ctx.author().id, job).await? {
            Outcome::Found { find, amount, wallet } => {
                send(ctx, embed(title, format!("{verb} {} **{}** takıldı ve **{}** kazandın.\nCüzdan: {}", find.emoji, find.name, money(amount), money(wallet)))).await
            }
            Outcome::Wait(seconds) => reply_private(ctx, format!("😮‍💨 Biraz dinlen; **{}** sonra tekrar deneyebilirsin.", economy::format_wait(std::time::Duration::from_secs(seconds)))).await,
        };
    }
    match economy::work(&ctx.data().db, guild, ctx.author().id).await? {
        Work::Ok { job, amount, wallet } => {
            send(ctx, embed("💼 Mesai bitti", format!("{job} ve **{}** kazandın.\nCüzdan: {}", money(amount), money(wallet)))).await
        }
        Work::Wait(wait) => reply_private(ctx, format!("😮‍💨 Biraz dinlen; **{}** sonra tekrar çalışabilirsin.", economy::format_wait(wait))).await,
    }
}

/// Bankaya para yatır (bankadaki para soyulamaz).
#[poise::command(slash_command, guild_only, rename = "yatır", category = "Ekonomi")]
pub async fn yatir(
    ctx: Context<'_>,
    #[rename = "miktar"]
    #[description = "Miktar (örn. 500, 1.5k, hepsi, yarısı)"]
    amount: String,
) -> Result<(), Error> {
    bank_move(ctx, &amount, true).await
}

/// Bankadan para çek.
#[poise::command(slash_command, guild_only, rename = "çek", category = "Ekonomi")]
pub async fn cek(
    ctx: Context<'_>,
    #[rename = "miktar"]
    #[description = "Miktar (örn. 500, 1.5k, hepsi, yarısı)"]
    amount: String,
) -> Result<(), Error> {
    bank_move(ctx, &amount, false).await
}

async fn bank_move(ctx: Context<'_>, input: &str, to_bank: bool) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let db = &ctx.data().db;
    let acc = economy::account(db, guild, ctx.author().id).await?;
    let available = if to_bank { acc.wallet } else { acc.bank };
    let amount = match economy::parse_amount(input, available) {
        Ok(a) => a,
        Err(m) => return reply_private(ctx, format!("❌ {m}")).await,
    };
    match economy::move_bank(db, guild, ctx.author().id, amount, to_bank).await? {
        Some((wallet, bank)) => {
            let verb = if to_bank { "bankaya yatırıldı" } else { "bankadan çekildi" };
            send(ctx, embed("🏦 Banka", format!("**{}** {verb}.\n👛 Cüzdan: {}\n🏦 Banka: {}", money(amount), money(wallet), money(bank)))).await
        }
        None => {
            let place = if to_bank { "Cüzdanında" } else { "Bankanda" };
            reply_private(ctx, format!("❌ {place} yeterli para yok ({}).", money(available))).await
        }
    }
}

/// Bir üyeye para gönder.
#[poise::command(slash_command, guild_only, rename = "gönder", category = "Ekonomi")]
pub async fn gonder(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Parayı alacak üye"]
    user: serenity::User,
    #[rename = "miktar"]
    #[description = "Miktar (örn. 500, 1.5k, hepsi)"]
    amount: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    if user.bot || user.id == ctx.author().id {
        return reply_private(ctx, "❌ Kendine veya botlara para gönderemezsin.").await;
    }
    let db = &ctx.data().db;
    let wallet = economy::account(db, guild, ctx.author().id).await?.wallet;
    let amount = match economy::parse_amount(&amount, wallet) {
        Ok(a) => a,
        Err(m) => return reply_private(ctx, format!("❌ {m}")).await,
    };
    if !economy::transfer(db, guild, ctx.author().id, user.id, amount).await? {
        return reply_private(ctx, format!("❌ Cüzdanında yeterli para yok ({}).", money(wallet))).await;
    }
    send(ctx, embed("💸 Para gönderildi", format!("{} → {}: **{}**", ctx.author().id.mention(), user.id.mention(), money(amount)))).await
}

/// Bir üyeyi soy (riskli!) ya da üye seçmeden sunucu çapında banka soygunu başlat.
#[poise::command(slash_command, guild_only, rename = "soy", category = "Ekonomi")]
pub async fn soy(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Soyulacak üye (boş bırakırsan herkesin katılabildiği banka soygunu başlar)"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    let Some(user) = user else {
        return super::heist::run(ctx).await;
    };
    let guild = guild_id(ctx)?;
    if user.bot || user.id == ctx.author().id {
        return reply_private(ctx, "❌ Kendini veya botları soyamazsın.").await;
    }
    let text = match economy::rob(&ctx.data().db, guild, ctx.author().id, user.id).await? {
        Rob::Success(stolen) => format!("🦹 Başardın! {} adlı üyenin cüzdanından **{}** aşırdın.", user.id.mention(), money(stolen)),
        Rob::Caught(fine) => format!("🚓 Yakalandın! {} adlı üyeye **{}** ceza ödedin.", user.id.mention(), money(fine)),
        Rob::Wait(wait) => return reply_private(ctx, format!("⏳ Polis peşinde; **{}** sonra tekrar dene.", economy::format_wait(wait))).await,
        Rob::TargetTooPoor => return reply_private(ctx, format!("🙄 {} adlı üyenin cüzdanında soymaya değecek para yok (en az {}).", user.id.mention(), money(economy::ROB_MIN_TARGET))).await,
        Rob::RobberTooPoor => return reply_private(ctx, format!("❌ Soygun için cüzdanında en az {} olmalı (yakalanırsan ceza ödersin).", money(economy::ROB_MIN_TARGET / 2))).await,
    };
    send(ctx, embed("🦹 Soygun", text)).await
}

/// En zengin 10 üye.
#[poise::command(slash_command, guild_only, category = "Ekonomi")]
pub async fn zenginler(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let top = economy::leaderboard(&ctx.data().db, guild, 10).await?;
    let description = if top.is_empty() {
        "Henüz kimsenin parası yok. `/ekonomi günlük` ile başla! 🪙".to_owned()
    } else {
        top.iter()
            .enumerate()
            .map(|(i, (user, total))| {
                let medal = ["🥇", "🥈", "🥉"].get(i).map_or_else(|| format!("`#{}`", i + 1), |m| (*m).to_owned());
                format!("{medal} {} — **{}**", user.mention(), money(*total))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    send(ctx, embed("💰 En zenginler", description)).await
}

// ===================== Şans oyunları =====================

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum CoinSide {
    #[name = "Yazı"]
    Tails,
    #[name = "Tura"]
    Heads,
}

/// Yazı-tura: bilirsen bahsini ikiye katlarsın.
#[poise::command(slash_command, guild_only, rename = "yazı-tura", category = "Ekonomi")]
pub async fn yazi_tura(
    ctx: Context<'_>,
    #[rename = "seçim"]
    #[description = "Yazı mı tura mı?"]
    side: CoinSide,
    #[rename = "bahis"]
    #[description = "Bahis (örn. 100, 1k, hepsi)"]
    bet: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let Some(bet) = take_bet(ctx, guild, &bet).await? else {
        return Ok(());
    };
    let heads = crate::rand_util::below(2) == 0;
    let result = if heads { "Tura" } else { "Yazı" };
    let won = matches!((side, heads), (CoinSide::Heads, true) | (CoinSide::Tails, false));
    let db = &ctx.data().db;
    let text = if won {
        let wallet = economy::add(db, guild, ctx.author().id, bet * 2).await?;
        format!("🪙 Para havada döndü… **{result}!**\n🎉 Kazandın: **+{}**\nCüzdan: {}", money(bet), money(wallet))
    } else {
        let wallet = economy::account(db, guild, ctx.author().id).await?.wallet;
        format!("🪙 Para havada döndü… **{result}!**\n😢 Kaybettin: **-{}**\nCüzdan: {}", money(bet), money(wallet))
    };
    send(ctx, embed("🪙 Yazı-Tura", text)).await
}

/// Slot makinesi: üç aynı sembol büyük ödül, iki aynı bahsin 1,5 katı.
#[poise::command(slash_command, guild_only, category = "Ekonomi")]
pub async fn slot(
    ctx: Context<'_>,
    #[rename = "bahis"]
    #[description = "Bahis (örn. 100, 1k, hepsi)"]
    bet: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let Some(bet) = take_bet(ctx, guild, &bet).await? else {
        return Ok(());
    };
    let reels = games::spin();
    let payout = i64::try_from(games::slot_payout(reels, u64::try_from(bet)?))?;
    let db = &ctx.data().db;
    let wallet = if payout > 0 {
        economy::add(db, guild, ctx.author().id, payout).await?
    } else {
        economy::account(db, guild, ctx.author().id).await?.wallet
    };
    let line = reels.map(games::slot_symbol).join(" | ");
    let result = match payout {
        0 => format!("😢 Kaybettin: **-{}**", money(bet)),
        p if p >= bet * 10 => format!("🎰 **BÜYÜK İKRAMİYE!** **+{}**", money(p - bet)),
        p => format!("🎉 Kazandın: **+{}**", money(p - bet)),
    };
    let paytable = games::SLOT_SYMBOLS.iter().map(|(s, m)| format!("{s}×{m}")).collect::<Vec<_>>().join(" ");
    let e = embed("🎰 Slot", format!("**[ {line} ]**\n\n{result}\nCüzdan: {}", money(wallet)))
        .footer(CreateEmbedFooter::new(format!("Üçlü: {paytable} · İkili: ×1,5")));
    send(ctx, e).await
}

/// Blackjack: 21'e krupiyeden daha çok yaklaş ama geçme.
#[poise::command(slash_command, guild_only, category = "Ekonomi")]
pub async fn blackjack(
    ctx: Context<'_>,
    #[rename = "bahis"]
    #[description = "Bahis (örn. 100, 1k, hepsi)"]
    bet: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let Some(bet) = take_bet(ctx, guild, &bet).await? else {
        return Ok(());
    };
    let mut deck = games::shuffled_deck();
    let draw = |deck: &mut Vec<Card>| deck.pop().unwrap_or(Card { rank: 10, suit: 0 });
    let mut player = vec![draw(&mut deck), draw(&mut deck)];
    let mut dealer = vec![draw(&mut deck), draw(&mut deck)];

    // Doğal blackjack varsa hemen biter.
    if games::is_blackjack(&player) || games::is_blackjack(&dealer) {
        let outcome = games::settle(&player, &dealer);
        let wallet = pay(ctx, guild, outcome, bet).await?;
        ctx.send(CreateReply::default().embed(bj_embed(&player, &dealer, false, Some((outcome, bet, wallet)))))
            .await?;
        return Ok(());
    }

    let id = ctx.id();
    let hit_id = format!("bj:hit:{id}");
    let stand_id = format!("bj:stand:{id}");
    let buttons = |disabled: bool| {
        vec![CreateActionRow::Buttons(vec![
            CreateButton::new(&hit_id).label("Kart çek").emoji('🃏').style(ButtonStyle::Primary).disabled(disabled),
            CreateButton::new(&stand_id).label("Dur").emoji('✋').style(ButtonStyle::Secondary).disabled(disabled),
        ])]
    };
    // Oyun ekranı gönderilemezse bahis geri verilir (oyun hiç başlamadı).
    let sent = async {
        let reply = ctx.send(CreateReply::default().embed(bj_embed(&player, &dealer, true, None)).components(buttons(false))).await?;
        let id = reply.message().await?.id;
        Ok::<_, serenity::Error>((reply, id))
    }
    .await;
    let (reply, message_id) = match sent {
        Ok(sent) => sent,
        Err(e) => {
            economy::add(&ctx.data().db, guild, ctx.author().id, bet).await?;
            return Err(e.into());
        }
    };

    let author = ctx.author().id;
    let mut last_press = None;
    loop {
        let press = ComponentInteractionCollector::new(ctx.serenity_context())
            .message_id(message_id)
            .author_id(author)
            .timeout(BLACKJACK_TIMEOUT)
            .await;
        let Some(press) = press else {
            break; // Süre doldu → durmuş sayılır.
        };
        if press.data.custom_id == hit_id {
            player.push(draw(&mut deck));
            if games::hand_value(&player) >= 21 {
                last_press = Some(press);
                break;
            }
            // Ekran güncellenemezse oyun sürer; bahis sonda yine ödenir.
            let _ = press
                .create_response(
                    ctx,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .embed(bj_embed(&player, &dealer, true, None))
                            .components(buttons(false)),
                    ),
                )
                .await;
        } else {
            last_press = Some(press);
            break;
        }
    }

    if games::hand_value(&player) <= 21 {
        games::dealer_play(&mut dealer, &mut deck);
    }
    let outcome = games::settle(&player, &dealer);
    let wallet = pay(ctx, guild, outcome, bet).await?;
    let finished = CreateInteractionResponseMessage::new()
        .embed(bj_embed(&player, &dealer, false, Some((outcome, bet, wallet))))
        .components(buttons(true));
    match last_press {
        Some(press) => press.create_response(ctx, CreateInteractionResponse::UpdateMessage(finished)).await?,
        None => {
            reply
                .edit(
                    ctx,
                    CreateReply::default()
                        .embed(bj_embed(&player, &dealer, false, Some((outcome, bet, wallet))))
                        .components(buttons(true)),
                )
                .await?;
        }
    }
    Ok(())
}

/// Oyun sonucunu öder; güncel cüzdanı döner.
async fn pay(ctx: Context<'_>, guild: GuildId, outcome: Outcome, bet: i64) -> Result<i64, Error> {
    let db = &ctx.data().db;
    let payout = i64::try_from(outcome.payout(u64::try_from(bet)?))?;
    if payout > 0 {
        Ok(economy::add(db, guild, ctx.author().id, payout).await?)
    } else {
        Ok(economy::account(db, guild, ctx.author().id).await?.wallet)
    }
}

fn hand_text(hand: &[Card]) -> String {
    hand.iter().map(|c| c.label()).collect::<Vec<_>>().join(" ")
}

fn bj_embed(player: &[Card], dealer: &[Card], hide_dealer: bool, result: Option<(Outcome, i64, i64)>) -> CreateEmbed {
    let dealer_text = if hide_dealer {
        let first = dealer.first().map(|c| c.label()).unwrap_or_default();
        format!("{first} `??`")
    } else {
        format!("{} — **{}**", hand_text(dealer), games::hand_value(dealer))
    };
    let mut e = CreateEmbed::new()
        .title("🃏 Blackjack")
        .colour(COLOUR)
        .field("Senin elin", format!("{} — **{}**", hand_text(player), games::hand_value(player)), false)
        .field("Krupiye", dealer_text, false);
    match result {
        None => e = e.footer(CreateEmbedFooter::new("Kart çek veya dur. 60 saniye içinde seçmezsen durmuş sayılırsın.")),
        Some((outcome, bet, wallet)) => {
            let line = match outcome {
                Outcome::Blackjack => format!("🎉 **Blackjack!** +{}", money(outcome_gain(outcome, bet))),
                Outcome::Win => format!("🎉 **Kazandın!** +{}", money(outcome_gain(outcome, bet))),
                Outcome::Push => "🤝 **Berabere**, bahsin iade edildi.".to_owned(),
                Outcome::Lose if games::hand_value(player) > 21 => format!("💥 **21'i geçtin!** -{}", money(bet)),
                Outcome::Lose => format!("😢 **Kaybettin.** -{}", money(bet)),
            };
            e = e.field("Sonuç", format!("{line}\nCüzdan: {}", money(wallet)), false);
            e = e.colour(match outcome {
                Outcome::Blackjack | Outcome::Win => serenity::Colour::from_rgb(87, 242, 135),
                Outcome::Push => serenity::Colour::LIGHT_GREY,
                Outcome::Lose => serenity::Colour::from_rgb(237, 66, 69),
            });
        }
    }
    e
}

fn outcome_gain(outcome: Outcome, bet: i64) -> i64 {
    let payout = u64::try_from(bet).map(|b| outcome.payout(b)).unwrap_or(0);
    i64::try_from(payout).unwrap_or(0) - bet
}

// ===================== Mağaza =====================

/// Sunucu mağazası.
#[poise::command(slash_command, guild_only, rename = "mağaza", category = "Ekonomi")]
pub async fn magaza(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let items = economy::shop(&ctx.data().db, guild).await?;
    let description = if items.is_empty() {
        "Mağaza boş. Yöneticiler `/mağaza-yönet ekle` ile ürün (veya satılık rol) ekleyebilir.".to_owned()
    } else {
        items
            .iter()
            .map(|item| {
                let role = item.role().map_or_else(String::new, |r| format!(" → {}", r.mention()));
                let desc = item.description.as_deref().map_or_else(String::new, |d| format!("\n  _{d}_"));
                format!("**{}** — {}{role}{desc}", item.name, money(item.price))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let e = embed("🛒 Mağaza", description).footer(CreateEmbedFooter::new("Satın almak için: /ekonomi satın-al ürün"));
    send(ctx, e).await
}

async fn autocomplete_item(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let Some(guild) = ctx.guild_id() else {
        return Vec::new();
    };
    let partial = partial.to_lowercase();
    economy::shop(&ctx.data().db, guild)
        .await
        .map(|items| {
            items
                .into_iter()
                .map(|i| i.name)
                .filter(|n| n.to_lowercase().contains(&partial))
                .take(25)
                .collect()
        })
        .unwrap_or_default()
}

/// Mağazadan ürün satın al.
#[poise::command(slash_command, guild_only, rename = "satın-al", category = "Ekonomi")]
pub async fn satin_al(
    ctx: Context<'_>,
    #[rename = "ürün"]
    #[description = "Ürün adı"]
    #[autocomplete = "autocomplete_item"]
    name: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let db = &ctx.data().db;
    let Some(item) = economy::find_item(db, guild, &name).await? else {
        return reply_private(ctx, format!("❌ Mağazada **{name}** adında ürün yok.")).await;
    };
    let user = ctx.author().id;
    if let Some(role) = item.role() {
        let has = ctx.author_member().await.is_some_and(|m| m.roles.contains(&role));
        if has {
            return reply_private(ctx, format!("Zaten {} rolün var.", role.mention())).await;
        }
    }
    let Some(wallet) = economy::spend(db, guild, user, item.price).await? else {
        let wallet = economy::account(db, guild, user).await?.wallet;
        return reply_private(ctx, format!("❌ Yeterli paran yok. Fiyat: {}, cüzdan: {}.", money(item.price), money(wallet))).await;
    };
    if let Some(role) = item.role() {
        if let Err(e) = ctx.http().add_member_role(guild, user, role, Some("Mağaza satın alımı")).await {
            economy::add(db, guild, user, item.price).await?;
            tracing::warn!(guild = %guild, role = %role, error = %e, "Mağaza rolü verilemedi");
            return reply_private(ctx, "❌ Rol verilemedi, paran iade edildi. (Botun rolü bu rolün üstünde olmalı.)").await;
        }
    } else {
        economy::add_to_inventory(db, guild, user, item.id).await?;
    }
    let what = item.role().map_or_else(|| format!("**{}**", item.name), |r| format!("**{}** ({})", item.name, r.mention()));
    send(ctx, embed("🛍️ Satın alındı", format!("{what} satın aldın! -{}\nCüzdan: {}", money(item.price), money(wallet)))).await
}

/// Satın aldığın ürünler.
#[poise::command(slash_command, guild_only, rename = "envanter", category = "Ekonomi")]
pub async fn envanter(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Bakılacak üye (boşsa sen)"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let items = economy::inventory(&ctx.data().db, guild, user.id).await?;
    let description = if items.is_empty() {
        "Envanter boş. 🛒 `/ekonomi mağaza`".to_owned()
    } else {
        items.iter().map(|(name, n)| format!("• **{name}** ×{n}")).collect::<Vec<_>>().join("\n")
    };
    send(ctx, embed(format!("🎒 {} envanteri", user.name), description)).await
}

/// Mağazayı yönet (yöneticiler).
#[poise::command(
    slash_command,
    guild_only,
    rename = "mağaza-yönet",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("magaza_ekle", "magaza_sil"),
    subcommand_required,
    category = "Ekonomi"
)]
pub async fn magaza_yonet(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Mağazaya ürün veya satılık rol ekler (aynı adla eklersen günceller).
#[poise::command(slash_command, guild_only, rename = "ekle")]
async fn magaza_ekle(
    ctx: Context<'_>,
    #[rename = "ad"]
    #[description = "Ürün adı"]
    #[max_length = 50]
    name: String,
    #[rename = "fiyat"]
    #[description = "Fiyat"]
    #[min = 1]
    price: u32,
    #[description = "Satın alana verilecek rol (opsiyonel)"] rol: Option<serenity::Role>,
    #[rename = "açıklama"]
    #[description = "Kısa açıklama"]
    #[max_length = 150]
    description: Option<String>,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    if let Some(role) = &rol {
        if let Err(reason) = crate::rolepanel::check_role(ctx.serenity_context(), guild, role) {
            return reply_private(ctx, format!("❌ {reason}")).await;
        }
    }
    if let Err(e) = economy::add_item(&ctx.data().db, guild, &name, i64::from(price), rol.as_ref().map(|r| r.id), description.as_deref()).await {
        return reply_private(ctx, format!("❌ {e}")).await;
    }
    reply_private(ctx, format!("✅ **{}** mağazada: {}", name.trim(), money(i64::from(price)))).await
}

/// Mağazadan ürün siler.
#[poise::command(slash_command, guild_only, rename = "sil")]
async fn magaza_sil(
    ctx: Context<'_>,
    #[rename = "ürün"]
    #[description = "Ürün adı"]
    #[autocomplete = "autocomplete_item"]
    name: String,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let text = if economy::remove_item(&ctx.data().db, guild, &name).await? {
        format!("🗑️ **{name}** mağazadan kaldırıldı.")
    } else {
        format!("Mağazada **{name}** adında ürün yok.")
    };
    reply_private(ctx, text).await
}

/// Üyelerin parasını yönet (yöneticiler).
#[poise::command(
    slash_command,
    guild_only,
    rename = "ekonomi-yönet",
    default_member_permissions = "MANAGE_GUILD",
    subcommands("eko_ver", "eko_al", "eko_sifirla"),
    subcommand_required,
    category = "Ekonomi"
)]
pub async fn ekonomi_yonet(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Bir üyeye para verir.
#[poise::command(slash_command, guild_only, rename = "ver")]
async fn eko_ver(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Üye"]
    user: serenity::User,
    #[rename = "miktar"]
    #[description = "Miktar"]
    #[min = 1]
    amount: u32,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let wallet = economy::add(&ctx.data().db, guild, user.id, i64::from(amount)).await?;
    reply_private(ctx, format!("✅ {} adlı üyeye {} verildi. Cüzdanı: {}", user.id.mention(), money(i64::from(amount)), money(wallet))).await
}

/// Bir üyenin cüzdanından para alır.
#[poise::command(slash_command, guild_only, rename = "al")]
async fn eko_al(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Üye"]
    user: serenity::User,
    #[rename = "miktar"]
    #[description = "Miktar"]
    #[min = 1]
    amount: u32,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let wallet = economy::add(&ctx.data().db, guild, user.id, -i64::from(amount)).await?;
    reply_private(ctx, format!("✅ {} adlı üyenin cüzdanından para alındı. Cüzdanı: {}", user.id.mention(), money(wallet))).await
}

/// Bir üyenin tüm parasını ve envanterini sıfırlar.
#[poise::command(slash_command, guild_only, rename = "sıfırla")]
async fn eko_sifirla(
    ctx: Context<'_>,
    #[rename = "üye"]
    #[description = "Üye"]
    user: serenity::User,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    economy::reset(&ctx.data().db, guild, user.id).await?;
    reply_private(ctx, format!("✅ {} adlı üyenin parası ve envanteri sıfırlandı.", user.id.mention())).await
}

/// Ekonomi komutları (bakiye, günlük ödül, iş, oyunlar, mağaza, borsa).
#[poise::command(
    slash_command,
    guild_only,
    rename = "ekonomi",
    subcommands(
        "bakiye",
        "gunluk",
        "calis",
        "yatir",
        "cek",
        "gonder",
        "soy",
        "zenginler",
        "yazi_tura",
        "slot",
        "blackjack",
        "magaza",
        "satin_al",
        "envanter",
        "super::stocks::borsa"
    ),
    subcommand_required,
    category = "Ekonomi"
)]
pub async fn ekonomi(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}
