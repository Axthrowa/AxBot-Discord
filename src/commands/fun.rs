//! Eğlence: `/oyun` (XOX, 4'lü bağla, adam asmaca, bilgi yarışması), `/eğlence` (8top, zar, seç,
//! doğruluk-cesaret, anket), `/hayvan` (yakalama oyunu).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

use dashmap::DashMap;
use std::time::{Duration, Instant};

use poise::futures_util::StreamExt;
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, ComponentInteractionCollector, CreateActionRow,
    CreateButton, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreatePoll, CreatePollAnswer, Mentionable, MessageCollector, ReactionType, UserId,
};
use poise::CreateReply;

use super::moderation::{guild_id, reply_private};
use crate::data::Context;
use crate::economy;
use crate::error::Error;
use crate::games::boards::{self, Grid, COLS, EMPTY, P1, P2, ROWS};
use crate::games::content::{self, ANIMALS};
use crate::games::hangman::{Guess, Hangman, MAX_WRONG};
use crate::games::animals;
use crate::guild_config::Setting;

const MOVE_TIMEOUT: Duration = Duration::from_secs(60);
const HANGMAN_IDLE: Duration = Duration::from_secs(120);
const TRIVIA_TIME: Duration = Duration::from_secs(20);
const WIN_REWARD: i64 = 50;
const TRIVIA_REWARD: i64 = 40;
/// Üye başına saatte en fazla bu kadar ödüllü oyun (iki arkadaş sırayla kaybederek para kasmasın).
const REWARDS_PER_HOUR: i64 = 6;

/// Son sorulan sorular (sunucu/tür başına) — aynı sorular art arda gelmesin.
static RECENT: LazyLock<DashMap<(u64, &'static str), VecDeque<usize>>> = LazyLock::new(DashMap::new);

fn fresh(guild: u64, kind: &'static str, len: usize, keep: usize) -> usize {
    let mut recent = RECENT.entry((guild, kind)).or_default();
    content::pick_fresh(len, &mut recent, keep)
}

/// Kanal başına tek sohbet oyunu (adam asmaca / bilgi yarışması) — mesajlar karışmasın.
struct ChannelLock<'a> {
    ctx: Context<'a>,
}

impl Drop for ChannelLock<'_> {
    fn drop(&mut self) {
        self.ctx.data().game_channels.remove(&self.ctx.channel_id());
    }
}

async fn lock_channel(ctx: Context<'_>) -> Result<Option<ChannelLock<'_>>, Error> {
    if !ctx.data().game_channels.insert(ctx.channel_id()) {
        reply_private(ctx, "Bu kanalda zaten bir oyun sürüyor; bitmesini bekle.").await?;
        return Ok(None);
    }
    Ok(Some(ChannelLock { ctx }))
}

async fn reject(ctx: Context<'_>, press: &ComponentInteraction, text: &str) -> Result<(), Error> {
    press
        .create_response(
            ctx,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true)),
        )
        .await?;
    Ok(())
}

/// Kazananı (bot değilse) ödüllendirir; saatlik sınır dolduysa ödül verilmez.
async fn reward(ctx: Context<'_>, user: UserId, amount: i64) -> String {
    let Some(guild) = ctx.guild_id() else {
        return String::new();
    };
    let key = format!("eko:oyun:{guild}:{user}");
    let mut conn = ctx.data().redis.clone();
    let won: redis::RedisResult<(i64, bool)> = redis::pipe().atomic().incr(&key, 1).expire(&key, 3600).arg("NX").query_async(&mut conn).await;
    if won.is_ok_and(|(count, _)| count > REWARDS_PER_HOUR) {
        return " _(saatlik oyun ödülü sınırı doldu)_".to_owned();
    }
    match economy::add(&ctx.data().db, guild, user, amount).await {
        Ok(_) => format!(" **+{}**", economy::cash(guild, amount)),
        Err(_) => String::new(),
    }
}

// ===================== /oyun =====================

/// Oyunlar: XOX, 4'lü bağla, adam asmaca, bilgi yarışması.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("xox", "dort_bagla", "adam_asmaca", "bilgi_yarismasi", "muzik_yarismasi"),
    subcommand_required,
    category = "Eğlence"
)]
pub async fn oyun(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// İki oyunculu sıra tabanlı oyunlarda oyuncular.
struct Players {
    p1: UserId,
    /// `None` = bot.
    p2: Option<UserId>,
}

impl Players {
    fn of(&self, piece: u8) -> Option<UserId> {
        if piece == P1 { Some(self.p1) } else { self.p2 }
    }
    fn name(&self, piece: u8) -> String {
        self.of(piece).map_or_else(|| "🤖 AxBot".to_owned(), |u| u.mention().to_string())
    }
}

fn opponent(ctx: Context<'_>, rakip: Option<serenity::User>) -> Result<Option<UserId>, String> {
    match rakip {
        None => Ok(None),
        Some(u) if u.bot => Ok(None),
        Some(u) if u.id == ctx.author().id => Err("Kendinle oynayamazsın; rakip seçme, benimle oyna! 🤖".to_owned()),
        Some(u) => Ok(Some(u.id)),
    }
}

/// XOX (tic-tac-toe). Rakip seçmezsen botla oynarsın.
#[poise::command(slash_command, guild_only)]
async fn xox(
    ctx: Context<'_>,
    #[description = "Rakip (boşsa benimle oynarsın)"] rakip: Option<serenity::User>,
) -> Result<(), Error> {
    let p2 = match opponent(ctx, rakip) {
        Ok(p) => p,
        Err(m) => return reply_private(ctx, m).await,
    };
    let players = Players { p1: ctx.author().id, p2 };
    let mut board = [EMPTY; 9];
    let mut turn = P1;
    let id = ctx.id();

    let render = |board: &[u8; 9], disabled: bool| -> Vec<CreateActionRow> {
        (0..3)
            .map(|r| {
                CreateActionRow::Buttons(
                    (0..3)
                        .map(|c| {
                            let i = r * 3 + c;
                            let button = CreateButton::new(format!("xox:{id}:{i}")).disabled(disabled || board[i] != EMPTY);
                            match board[i] {
                                P1 => button.emoji('❌').style(ButtonStyle::Danger),
                                P2 => button.emoji('⭕').style(ButtonStyle::Primary),
                                _ => button.label("\u{2800}").style(ButtonStyle::Secondary),
                            }
                        })
                        .collect(),
                )
            })
            .collect()
    };
    let status = |turn: u8| format!("❌ {} vs ⭕ {}\nSıra: {}", players.name(P1), players.name(P2), players.name(turn));

    let reply = ctx
        .send(CreateReply::default().embed(game_embed("❌⭕ XOX", status(turn))).components(render(&board, false)))
        .await?;
    let message = reply.message().await?.id;

    let result = loop {
        let Some(press) = ComponentInteractionCollector::new(ctx.serenity_context())
            .message_id(message)
            .timeout(MOVE_TIMEOUT)
            .await
        else {
            break format!("⏰ {} 60 saniye içinde oynamadı; oyun bitti.", players.name(turn));
        };
        if Some(press.user.id) != players.of(turn) {
            let text = if press.user.id == players.p1 || Some(press.user.id) == players.p2 { "Sıra sende değil." } else { "Bu oyunda değilsin." };
            reject(ctx, &press, text).await?;
            continue;
        }
        let Some(cell) = press.data.custom_id.rsplit(':').next().and_then(|s| s.parse::<usize>().ok()).filter(|i| *i < 9) else {
            continue;
        };
        if board[cell] != EMPTY {
            reject(ctx, &press, "Bu kare dolu.").await?;
            continue;
        }
        board[cell] = turn;
        turn = boards::other(turn);
        // Botun hamlesi.
        if players.p2.is_none() && turn == P2 && boards::xox_winner(&board).is_none() && !boards::xox_full(&board) {
            if let Some(i) = boards::xox_bot_move(&board, P2, 25) {
                board[i] = P2;
            }
            turn = P1;
        }
        let finished = boards::xox_winner(&board).or_else(|| boards::xox_full(&board).then_some(EMPTY));
        let (text, done) = match finished {
            Some(EMPTY) => ("🤝 Berabere!".to_owned(), true),
            Some(w) => {
                let bonus = match players.of(w) {
                    Some(u) => reward(ctx, u, WIN_REWARD).await,
                    None => String::new(),
                };
                (format!("🏆 {} kazandı!{bonus}", players.name(w)), true)
            }
            None => (status(turn), false),
        };
        press
            .create_response(
                ctx,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new().embed(game_embed("❌⭕ XOX", &text)).components(render(&board, done)),
                ),
            )
            .await?;
        if done {
            return Ok(());
        }
    };
    reply
        .edit(ctx, CreateReply::default().embed(game_embed("❌⭕ XOX", result)).components(render(&board, true)))
        .await?;
    Ok(())
}

fn game_embed(title: &str, description: impl Into<String>) -> CreateEmbed {
    CreateEmbed::new().title(title).description(description).colour(serenity::Colour::BLURPLE)
}

fn c4_text(grid: &Grid) -> String {
    let mut out = String::from("1️⃣2️⃣3️⃣4️⃣5️⃣6️⃣7️⃣\n");
    for row in grid {
        for cell in row {
            out.push_str(match *cell {
                P1 => "🔴",
                P2 => "🟡",
                _ => "⚫",
            });
        }
        out.push('\n');
    }
    out
}

/// 4'lü bağla. Rakip seçmezsen botla oynarsın.
#[poise::command(slash_command, guild_only, rename = "dört-bağla")]
async fn dort_bagla(
    ctx: Context<'_>,
    #[description = "Rakip (boşsa benimle oynarsın)"] rakip: Option<serenity::User>,
) -> Result<(), Error> {
    let p2 = match opponent(ctx, rakip) {
        Ok(p) => p,
        Err(m) => return reply_private(ctx, m).await,
    };
    let players = Players { p1: ctx.author().id, p2 };
    let mut grid: Grid = [[EMPTY; COLS]; ROWS];
    let mut turn = P1;
    let id = ctx.id();
    let buttons = |grid: &Grid, disabled: bool| -> Vec<CreateActionRow> {
        let make = |c: usize| {
            CreateButton::new(format!("c4:{id}:{c}"))
                .label((c + 1).to_string())
                .style(ButtonStyle::Secondary)
                .disabled(disabled || grid[0][c] != EMPTY)
        };
        vec![
            CreateActionRow::Buttons((0..4).map(make).collect()),
            CreateActionRow::Buttons((4..COLS).map(make).collect()),
        ]
    };
    let text = |grid: &Grid, line: &str| format!("🔴 {} vs 🟡 {}\n\n{}\n{line}", players.name(P1), players.name(P2), c4_text(grid));

    let reply = ctx
        .send(
            CreateReply::default()
                .embed(game_embed("🔴🟡 Dört Bağla", text(&grid, &format!("Sıra: {}", players.name(turn)))))
                .components(buttons(&grid, false)),
        )
        .await?;
    let message = reply.message().await?.id;

    let timeout_line = loop {
        let Some(press) = ComponentInteractionCollector::new(ctx.serenity_context())
            .message_id(message)
            .timeout(MOVE_TIMEOUT)
            .await
        else {
            break format!("⏰ {} 60 saniye içinde oynamadı; oyun bitti.", players.name(turn));
        };
        if Some(press.user.id) != players.of(turn) {
            let text = if press.user.id == players.p1 || Some(press.user.id) == players.p2 { "Sıra sende değil." } else { "Bu oyunda değilsin." };
            reject(ctx, &press, text).await?;
            continue;
        }
        let Some(col) = press.data.custom_id.rsplit(':').next().and_then(|s| s.parse::<usize>().ok()) else {
            continue;
        };
        if boards::drop_piece(&mut grid, col, turn).is_none() {
            reject(ctx, &press, "Bu sütun dolu.").await?;
            continue;
        }
        turn = boards::other(turn);
        if players.p2.is_none() && turn == P2 && boards::c4_winner(&grid).is_none() && !boards::c4_full(&grid) {
            if let Some(c) = boards::c4_bot_move(&grid, P2) {
                boards::drop_piece(&mut grid, c, P2);
            }
            turn = P1;
        }
        let (line, done) = match (boards::c4_winner(&grid), boards::c4_full(&grid)) {
            (Some(w), _) => {
                let bonus = match players.of(w) {
                    Some(u) => reward(ctx, u, WIN_REWARD).await,
                    None => String::new(),
                };
                (format!("🏆 {} kazandı!{bonus}", players.name(w)), true)
            }
            (None, true) => ("🤝 Berabere!".to_owned(), true),
            (None, false) => (format!("Sıra: {}", players.name(turn)), false),
        };
        press
            .create_response(
                ctx,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embed(game_embed("🔴🟡 Dört Bağla", text(&grid, &line)))
                        .components(buttons(&grid, done)),
                ),
            )
            .await?;
        if done {
            return Ok(());
        }
    };
    reply
        .edit(ctx, CreateReply::default().embed(game_embed("🔴🟡 Dört Bağla", text(&grid, &timeout_line))).components(buttons(&grid, true)))
        .await?;
    Ok(())
}

/// Adam asmaca: kanaldaki herkes harf veya kelime yazarak tahmin eder.
#[poise::command(
    slash_command,
    guild_only,
    rename = "adam-asmaca",
    // Tahminler kanal mesajlarından okunur; bot kanalı göremiyorsa oyun hiç başlamaz.
    required_bot_permissions = "VIEW_CHANNEL | SEND_MESSAGES | READ_MESSAGE_HISTORY | ADD_REACTIONS"
)]
async fn adam_asmaca(ctx: Context<'_>) -> Result<(), Error> {
    let Some(_lock) = lock_channel(ctx).await? else {
        return Ok(());
    };
    let (category, words) = content::pick(content::WORDS).copied().unwrap_or(("Hayvan", &["kedi"]));
    let word = content::pick(words).copied().unwrap_or("kedi");
    let mut game = Hangman::new(word);
    let render = |game: &Hangman, line: &str| {
        let stage = content::HANGMAN_STAGES.get(game.wrong_count().min(MAX_WRONG)).copied().unwrap_or_default();
        let wrong = game.wrong_letters();
        let wrong = if wrong.is_empty() { "—".to_owned() } else { wrong };
        game_embed(
            "🪢 Adam Asmaca",
            format!(
                "{stage}\n**{}**\n\nKategori: **{category}** · Harf: **{}**\nYanlış: {wrong} ({}/{MAX_WRONG})\n\n{line}",
                game.masked(),
                word.chars().count(),
                game.wrong_count()
            ),
        )
    };
    let reply = ctx
        .send(CreateReply::default().embed(render(&game, "Bu kanala **bir harf** ya da **kelimenin tamamını** yaz!")))
        .await?;

    let word_len = word.chars().count();
    let mut stream = MessageCollector::new(ctx.serenity_context())
        .channel_id(ctx.channel_id())
        .filter(move |m| {
            let n = m.content.trim().chars().count();
            !m.author.bot && (n == 1 || n == word_len) && m.content.trim().chars().all(char::is_alphabetic)
        })
        .timeout(Duration::from_secs(15 * 60))
        .stream();

    let outcome = loop {
        let next = tokio::time::timeout(HANGMAN_IDLE, stream.next()).await;
        let Ok(Some(message)) = next else {
            break format!("⏰ Kimse tahmin yapmadı. Kelime **{}** idi.", game.word());
        };
        let result = game.guess(&message.content);
        let emoji = match result {
            Guess::Hit => Some('✅'),
            Guess::Miss => Some('❌'),
            Guess::AlreadyTried => Some('🔁'),
            Guess::Invalid => None,
        };
        if let Some(emoji) = emoji {
            let _ = message.react(ctx, ReactionType::Unicode(emoji.to_string())).await;
        }
        if game.is_won() {
            let bonus = reward(ctx, message.author.id, WIN_REWARD).await;
            break format!("🎉 {} bildi! Kelime **{}**.{bonus}", message.author.id.mention(), game.word());
        }
        if game.is_lost() {
            break format!("💀 Adam asıldı! Kelime **{}** idi.", game.word());
        }
        if matches!(result, Guess::Hit | Guess::Miss) {
            let _ = reply.edit(ctx, CreateReply::default().embed(render(&game, "Tahmin etmeye devam!"))).await;
        }
    };
    // Oyun mesajı silinmişse (Unknown Message) sonuç yeni mesajla duyurulur.
    if reply.edit(ctx, CreateReply::default().embed(render(&game, &outcome))).await.is_err() {
        ctx.channel_id().send_message(ctx, serenity::CreateMessage::new().embed(render(&game, &outcome))).await?;
    }
    Ok(())
}

/// Klip kanalında en çok klip paylaşanlar (AxT Capture ya da elle yüklenen videolar).
#[poise::command(slash_command, guild_only, rename = "klipler")]
async fn klipler(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let channel = data.settings.get::<crate::community::CommunitySettings>(&data.db, guild_id).await.ok().and_then(|s| s.clip_channel);
    let Some(channel) = channel else {
        return reply_private(ctx, "Bu sunucuda klip kanalı ayarlı değil (yöneticiler panelden açabilir).").await;
    };
    let board = crate::community::clip_board(&data.redis, guild_id, 10).await;
    let text = if board.is_empty() {
        format!("Henüz klip paylaşılmadı. <#{channel}> kanalına bir video yükle ya da AxT Capture'dan gönder!")
    } else {
        board.iter().enumerate().map(|(i, (owner, count))| format!("`#{}` {} — **{count}** klip", i + 1, crate::community::clip_label(owner))).collect::<Vec<_>>().join("\n")
    };
    ctx.send(CreateReply::default().embed(game_embed("🎬 Klip sıralaması", text)).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Müzik yarışmasında çalınan şarkılar: (sanatçı, şarkı).
const QUIZ_SONGS: &[(&str, &str)] = &[
    ("Tarkan", "Şımarık"),
    ("Tarkan", "Dudu"),
    ("Sezen Aksu", "Hadi Bakalım"),
    ("Barış Manço", "Gülpembe"),
    ("Barış Manço", "Dönence"),
    ("Cem Karaca", "Resimdeki Gözyaşları"),
    ("Sertab Erener", "Everyway That I Can"),
    ("Mor ve Ötesi", "Bir Derdim Var"),
    ("Duman", "Senden Daha Güzel"),
    ("Manga", "We Could Be The Same"),
    ("Athena", "For Real"),
    ("Teoman", "Paramparça"),
    ("Şebnem Ferah", "Sil Baştan"),
    ("Kenan Doğulu", "Çakkıdı"),
    ("Mustafa Sandal", "İsyankar"),
    ("Ajda Pekkan", "Bambaşka Biri"),
    ("Zeki Müren", "Gitme Sana Muhtacım"),
    ("Müslüm Gürses", "Nilüfer"),
    ("Sagopa Kajmer", "Galiba"),
    ("Ceza", "Holocaust"),
    ("Queen", "Bohemian Rhapsody"),
    ("Michael Jackson", "Billie Jean"),
    ("Nirvana", "Smells Like Teen Spirit"),
    ("Eminem", "Lose Yourself"),
    ("Adele", "Rolling in the Deep"),
    ("Coldplay", "Viva La Vida"),
    ("The Weeknd", "Blinding Lights"),
    ("Dua Lipa", "Levitating"),
    ("Imagine Dragons", "Believer"),
    ("Linkin Park", "In the End"),
    ("Daft Punk", "Get Lucky"),
    ("AC/DC", "Thunderstruck"),
    ("Metallica", "Enter Sandman"),
    ("Shakira", "Waka Waka"),
    ("Luis Fonsi", "Despacito"),
    ("Ed Sheeran", "Shape of You"),
    ("Billie Eilish", "Bad Guy"),
    ("a-ha", "Take On Me"),
    ("Survivor", "Eye of the Tiger"),
    ("Europe", "The Final Countdown"),
];
const QUIZ_ROUND: Duration = Duration::from_secs(30);
const QUIZ_REWARD: i64 = 150;

/// Tahmin karşılaştırması için: küçük harf, Türkçe harfler sadeleşir, yalnızca harf ve rakam kalır.
fn quiz_fold(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            'İ' | 'I' | 'ı' => vec!['i'],
            'Ş' | 'ş' => vec!['s'],
            'Ç' | 'ç' => vec!['c'],
            'Ğ' | 'ğ' => vec!['g'],
            'Ö' | 'ö' => vec!['o'],
            'Ü' | 'ü' => vec!['u'],
            c => c.to_lowercase().collect(),
        })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Tahmin doğru mu? Şarkının ya da sanatçının adını (yazım farklarına aldırmadan) içermesi yeterli.
fn quiz_match(guess: &str, artist: &str, title: &str) -> bool {
    let guess = quiz_fold(guess);
    [quiz_fold(title), quiz_fold(artist)].iter().any(|answer| answer.len() >= 3 && guess.contains(answer.as_str()))
}

/// Müzik yarışması: ses kanalında şarkı çalar, adını ya da sanatçısını ilk yazan puanı alır.
#[poise::command(slash_command, guild_only, rename = "müzik-yarışması", required_bot_permissions = "CONNECT | SPEAK")]
async fn muzik_yarismasi(
    ctx: Context<'_>,
    #[description = "Kaç şarkı çalınsın (varsayılan 5)"]
    #[min = 1]
    #[max = 10]
    tur: Option<usize>,
) -> Result<(), Error> {
    let guild_id = guild_id(ctx)?;
    let data = ctx.data();
    let voice = ctx.guild().and_then(|g| g.voice_states.get(&ctx.author().id).and_then(|v| v.channel_id));
    let Some(voice) = voice else {
        return reply_private(ctx, "🔊 Önce bir ses kanalına gir; şarkıları orada çalacağım.").await;
    };
    if data.music.card_state(guild_id).await.is_some() {
        return reply_private(ctx, "Şu an müzik çalıyor. Yarışma için önce `/müzik durdur` ile müziği bitir.").await;
    }
    if data.music.settings(guild_id).autoplay {
        return reply_private(ctx, "Otomatik çalma açıkken yarışma yapılamaz (araya şarkı ekler). Önce `/müzik otomatik` ile kapat.").await;
    }
    let Some(_lock) = lock_channel(ctx).await? else {
        return Ok(());
    };
    let rounds = tur.unwrap_or(5);
    let mut order: Vec<usize> = (0..QUIZ_SONGS.len()).collect();
    crate::rand_util::shuffle(&mut order);
    ctx.send(CreateReply::default().embed(game_embed(
        "🎧 Müzik Yarışması",
        format!("**{rounds}** şarkı çalacağım. Her şarkıda **şarkının ya da sanatçının adını** ilk yazan puanı alır; şarkı başına 30 saniye var.\nSes kanalı: <#{voice}>"),
    )))
    .await?;

    let http = std::sync::Arc::clone(&ctx.serenity_context().http);
    let mut scores: HashMap<UserId, u32> = HashMap::new();
    let mut played = 0;
    for index in order {
        if played >= rounds {
            break;
        }
        let Some((artist, title)) = QUIZ_SONGS.get(index).copied() else { continue };
        // Bulunamayan şarkı atlanır, yerine sıradaki denenir.
        let Ok((input, info)) = data.music.resolve(&format!("{artist} {title} audio"), ctx.author().id, guild_id).await else {
            continue;
        };
        if let Err(e) = data.music.enqueue(std::sync::Arc::clone(&http), guild_id, voice, ctx.channel_id(), input, info).await {
            tracing::warn!(guild = %guild_id, error = ?e, "Müzik yarışması: şarkı çalınamadı");
            ctx.channel_id().say(&http, "⚠️ Ses kanalına bağlanamadım; yarışma bitti.").await?;
            break;
        }
        played += 1;
        ctx.channel_id().say(&http, format!("🎵 **{played}. şarkı çalıyor…** Adını ya da sanatçısını yaz!")).await?;

        let deadline = Instant::now() + QUIZ_ROUND;
        let mut winner = None;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let Some(guess) = MessageCollector::new(ctx.serenity_context()).channel_id(ctx.channel_id()).timeout(left).await else {
                break;
            };
            if !guess.author.bot && quiz_match(&guess.content, artist, title) {
                winner = Some(guess.author.id);
                break;
            }
        }
        // Çalan şarkı kesilir; olay dinleyicileri kilit beklemesin diye kilit hemen bırakılır.
        if let Some(call) = data.music.songbird().get(guild_id) {
            let queue = call.lock().await.queue().clone();
            let _ = queue.skip();
        }
        let text = match winner {
            Some(user) => {
                *scores.entry(user).or_default() += 1;
                format!("✅ <@{user}> bildi! **{artist} — {title}**")
            }
            None => format!("⌛ Bilen çıkmadı: **{artist} — {title}**"),
        };
        ctx.channel_id().send_message(&http, serenity::CreateMessage::new().content(text).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    data.music.stop_and_leave(guild_id).await;

    let mut ranking: Vec<(UserId, u32)> = scores.into_iter().collect();
    ranking.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut text = if ranking.is_empty() {
        "Kimse puan alamadı. 😅".to_owned()
    } else {
        ranking.iter().take(10).enumerate().map(|(i, (user, points))| format!("**{}.** <@{user}> — {points} puan", i + 1)).collect::<Vec<_>>().join("\n")
    };
    if let Some((champion, _)) = ranking.first() {
        text.push_str(&reward(ctx, *champion, QUIZ_REWARD).await);
    }
    ctx.channel_id()
        .send_message(&http, serenity::CreateMessage::new().embed(game_embed("🏁 Müzik Yarışması bitti", text)).allowed_mentions(serenity::CreateAllowedMentions::new()))
        .await?;
    Ok(())
}

/// Bilgi yarışması: 20 saniyede doğru cevabı seç (herkes katılabilir).
#[poise::command(slash_command, guild_only, rename = "bilgi-yarışması")]
async fn bilgi_yarismasi(ctx: Context<'_>) -> Result<(), Error> {
    let Some(_lock) = lock_channel(ctx).await? else {
        return Ok(());
    };
    let guild = ctx.guild_id().map_or(0, |g| g.get());
    let Some(question) = content::QUESTIONS.get(fresh(guild, "trivia", content::QUESTIONS.len(), 40)) else {
        return Ok(());
    };
    let mut order: Vec<usize> = (0..4).collect();
    crate::rand_util::shuffle(&mut order);
    let correct_slot = order.iter().position(|i| *i == 0).unwrap_or(0);
    let letters = ['A', 'B', 'C', 'D'];
    let id = ctx.id();
    let options: Vec<&str> = order.iter().map(|i| question.options.get(*i).copied().unwrap_or_default()).collect();
    let buttons = |reveal: bool| {
        vec![CreateActionRow::Buttons(
            (0..4)
                .map(|slot| {
                    let style = match (reveal, slot == correct_slot) {
                        (true, true) => ButtonStyle::Success,
                        (true, false) => ButtonStyle::Secondary,
                        _ => ButtonStyle::Primary,
                    };
                    CreateButton::new(format!("trivia:{id}:{slot}"))
                        .label(letters.get(slot).map(char::to_string).unwrap_or_default())
                        .style(style)
                        .disabled(reveal)
                })
                .collect(),
        )]
    };
    let listing = options
        .iter()
        .enumerate()
        .map(|(slot, text)| format!("**{}** · {text}", letters.get(slot).copied().unwrap_or('?')))
        .collect::<Vec<_>>()
        .join("\n");
    let deadline = Instant::now() + TRIVIA_TIME;
    let ends = serenity::Timestamp::now().unix_timestamp() + TRIVIA_TIME.as_secs() as i64;
    let embed = game_embed(&format!("🧠 Bilgi Yarışması · {}", question.category), format!("**{}**\n\n{listing}\n\n⏳ Bitiş: <t:{ends}:R>", question.text));
    let reply = ctx.send(CreateReply::default().embed(embed).components(buttons(false))).await?;
    let message = reply.message().await?.id;

    // Kullanıcı → (seçtiği şık, sıra)
    let mut answers: HashMap<UserId, usize> = HashMap::new();
    let mut correct_order: Vec<UserId> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let Some(press) = ComponentInteractionCollector::new(ctx.serenity_context())
            .message_id(message)
            .timeout(left)
            .await
        else {
            break;
        };
        if answers.contains_key(&press.user.id) {
            reject(ctx, &press, "Zaten cevap verdin.").await?;
            continue;
        }
        let Some(slot) = press.data.custom_id.rsplit(':').next().and_then(|s| s.parse::<usize>().ok()) else {
            continue;
        };
        answers.insert(press.user.id, slot);
        if slot == correct_slot {
            correct_order.push(press.user.id);
        }
        reject(ctx, &press, &format!("Cevabın alındı: **{}**", letters.get(slot).copied().unwrap_or('?'))).await?;
    }

    let right = options.get(correct_slot).copied().unwrap_or_default();
    let mut lines = vec![format!(
        "Doğru cevap: **{} · {right}**",
        letters.get(correct_slot).copied().unwrap_or('?')
    )];
    if correct_order.is_empty() {
        lines.push(format!("Kimse bilemedi. ({} kişi cevapladı)", answers.len()));
    } else {
        let mut winners = Vec::new();
        for (i, user) in correct_order.iter().enumerate() {
            let amount = if i == 0 { TRIVIA_REWARD * 2 } else { TRIVIA_REWARD };
            let bonus = reward(ctx, *user, amount).await;
            winners.push(format!("{}{} {}{bonus}", if i == 0 { "🥇 " } else { "• " }, user.mention(), if i == 0 { "(ilk bilen)" } else { "" }));
        }
        lines.push(format!("✅ Bilenler ({}/{}):\n{}", correct_order.len(), answers.len(), winners.join("\n")));
    }
    let embed = game_embed(&format!("🧠 Bilgi Yarışması · {}", question.category), format!("**{}**\n\n{listing}\n\n{}", question.text, lines.join("\n")));
    reply.edit(ctx, CreateReply::default().embed(embed).components(buttons(true))).await?;
    Ok(())
}

// ===================== /eğlence =====================

/// Küçük eğlenceler: 8top, zar, seç, doğruluk-cesaret, anket.
#[poise::command(
    slash_command,
    guild_only,
    rename = "eğlence",
    subcommands("sekiz_top", "zar", "sec", "dogruluk_cesaret", "anket", "klipler"),
    subcommand_required,
    category = "Eğlence"
)]
pub async fn eglence(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Sihirli 8 top'a bir evet/hayır sorusu sor.
#[poise::command(slash_command, guild_only, rename = "8top")]
async fn sekiz_top(
    ctx: Context<'_>,
    #[rename = "soru"]
    #[description = "Evet/hayır sorusu"]
    #[max_length = 200]
    question: String,
) -> Result<(), Error> {
    let (answer, verdict) = content::eight_ball();
    let (icon, colour) = match verdict {
        content::Verdict::Yes => ("🟢", serenity::Colour::from_rgb(87, 242, 135)),
        content::Verdict::Maybe => ("🟡", serenity::Colour::from_rgb(254, 231, 92)),
        content::Verdict::No => ("🔴", serenity::Colour::from_rgb(237, 66, 69)),
    };
    let embed = CreateEmbed::new()
        .title("🎱 Sihirli 8 Top")
        .description(format!("❓ **{question}**\n\n{icon} **{answer}**"))
        .colour(colour)
        .footer(CreateEmbedFooter::new(
            "Evet/hayır sorusu sor, 8 top rastgele cevaplasın. Sadece eğlence, ciddiye alma 😄",
        ));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Zar at.
#[poise::command(slash_command, guild_only)]
async fn zar(
    ctx: Context<'_>,
    #[rename = "adet"]
    #[description = "Kaç zar (1-10, varsayılan 1)"]
    #[min = 1]
    #[max = 10]
    count: Option<u8>,
    #[rename = "yüz"]
    #[description = "Zarın yüz sayısı (2-100, varsayılan 6)"]
    #[min = 2]
    #[max = 100]
    sides: Option<u8>,
) -> Result<(), Error> {
    let (count, sides) = (count.unwrap_or(1), sides.unwrap_or(6));
    let rolls: Vec<u64> = (0..count).map(|_| 1 + crate::rand_util::below(u64::from(sides))).collect();
    let total: u64 = rolls.iter().sum();
    let list = rolls.iter().map(|r| format!("**{r}**")).collect::<Vec<_>>().join(" + ");
    let text = if count > 1 { format!("{list} = **{total}**") } else { list };
    ctx.say(format!("🎲 {count}d{sides}: {text}")).await?;
    Ok(())
}

/// Seçenekler arasından rastgele seçer (virgülle ayır).
#[poise::command(slash_command, guild_only, rename = "seç")]
async fn sec(
    ctx: Context<'_>,
    #[rename = "seçenekler"]
    #[description = "Virgülle ayır (örn. pizza, döner, lahmacun)"]
    #[max_length = 500]
    options: String,
) -> Result<(), Error> {
    let list: Vec<&str> = options.split([',', '|']).map(str::trim).filter(|o| !o.is_empty()).collect();
    if list.len() < 2 {
        return reply_private(ctx, "En az iki seçenek yaz (virgülle ayır).").await;
    }
    let choice = content::pick(&list).copied().unwrap_or_default();
    ctx.say(format!("🤔 {} seçenek arasından seçimim: **{choice}**", list.len())).await?;
    Ok(())
}

pub const TOD_PREFIX: &str = "tod:";

fn tod_buttons() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("tod:truth").label("Doğruluk").emoji('😇').style(ButtonStyle::Success),
        CreateButton::new("tod:dare").label("Cesaret").emoji('😈').style(ButtonStyle::Danger),
        CreateButton::new("tod:random").label("Rastgele").emoji('🎲').style(ButtonStyle::Primary),
    ])]
}

fn tod_embed(guild: u64, kind: &str, requester: UserId) -> CreateEmbed {
    let (title, text, colour) = match kind {
        "dare" => {
            let i = fresh(guild, "dare", content::DARES.len(), 20);
            ("😈 Cesaret", content::DARES.get(i).copied().unwrap_or_default(), serenity::Colour::from_rgb(237, 66, 69))
        }
        _ => {
            let i = fresh(guild, "truth", content::TRUTHS.len(), 20);
            ("😇 Doğruluk", content::TRUTHS.get(i).copied().unwrap_or_default(), serenity::Colour::from_rgb(87, 242, 135))
        }
    };
    CreateEmbed::new()
        .title(title)
        .description(format!("{}\n\n{text}", requester.mention()))
        .colour(colour)
        .footer(CreateEmbedFooter::new("Yeni soru için butonlara bas."))
}

/// Doğruluk mu cesaret mi?
#[poise::command(slash_command, guild_only, rename = "doğruluk-cesaret")]
async fn dogruluk_cesaret(ctx: Context<'_>) -> Result<(), Error> {
    let embed = game_embed("😇 Doğruluk mu 😈 Cesaret mi?", "Aşağıdan seç! Herkes butonlara basabilir.");
    ctx.send(CreateReply::default().embed(embed).components(tod_buttons())).await?;
    Ok(())
}

/// Doğruluk/cesaret butonları (bot yeniden başlasa da çalışır).
pub async fn handle_tod(ctx: &serenity::Context, press: &ComponentInteraction) -> anyhow::Result<()> {
    let kind = match press.data.custom_id.as_str() {
        "tod:truth" => "truth",
        "tod:dare" => "dare",
        _ => {
            if crate::rand_util::below(2) == 0 { "truth" } else { "dare" }
        }
    };
    press
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .embed(tod_embed(press.guild_id.map_or(0, |g| g.get()), kind, press.user.id))
                    .components(tod_buttons()),
            ),
        )
        .await?;
    Ok(())
}

/// Discord anketi oluşturur.
#[poise::command(slash_command, guild_only)]
async fn anket(
    ctx: Context<'_>,
    #[rename = "soru"]
    #[description = "Anket sorusu"]
    #[max_length = 300]
    question: String,
    #[rename = "seçenekler"]
    #[description = "Virgülle ayır (2-10 seçenek)"]
    #[max_length = 1000]
    options: String,
    #[rename = "süre"]
    #[description = "Örn. 5dk, 30dk, 1sa30dk, 2sa, 1g (sadece sayı = dakika; varsayılan 1g)"]
    #[max_length = 20]
    length: Option<String>,
) -> Result<(), Error> {
    let length = match length.as_deref().map(str::trim) {
        None | Some("") => Duration::from_secs(24 * 3600),
        // Birimsiz sayı dakika sayılır.
        Some(raw) if raw.chars().all(|c| c.is_ascii_digit()) => match raw.parse::<u64>() {
            Ok(m) if m > 0 => Duration::from_secs(m * 60),
            _ => return reply_private(ctx, "❌ Süre 0'dan büyük olmalı.").await,
        },
        Some(raw) => match crate::moderation::duration::parse(raw) {
            Ok(d) => d,
            Err(e) => return reply_private(ctx, format!("❌ Geçersiz süre: {e}. Örnek: `30dk`, `2sa`, `1g`.")).await,
        },
    };
    if length < Duration::from_secs(60) {
        return reply_private(ctx, "❌ Anket en az 1 dakika sürmeli.").await;
    }
    if length > NATIVE_POLL_MAX {
        return reply_private(ctx, "❌ Anket en fazla 32 gün sürebilir.").await;
    }
    let mut seen = HashSet::new();
    let list: Vec<String> = options
        .split([',', '|'])
        .map(|o| o.trim().chars().take(55).collect::<String>())
        .filter(|o| !o.is_empty() && seen.insert(o.to_lowercase()))
        .take(10)
        .collect();
    if list.len() < 2 {
        return reply_private(ctx, "En az iki farklı seçenek yaz (virgülle ayır).").await;
    }
    // Discord'un yerleşik anketi sadece tam saat kabul eder; dakikalı süreler için butonlu anket.
    if length.as_secs() % 3600 == 0 {
        let poll = CreatePoll::new()
            .question(question)
            .answers(list.into_iter().map(|o| CreatePollAnswer::new().text(o)).collect())
            .duration(length);
        // Komutun cevabı olarak gönderilir: botun kanala ayrıca yazma izni gerekmez.
        ctx.send(CreateReply::default().poll(poll)).await?;
        return Ok(());
    }
    if length > BUTTON_POLL_MAX {
        return reply_private(
            ctx,
            "❌ Dakikalı anketler en fazla 6 saat sürebilir. Daha uzun anket için tam saat yaz (örn. `8sa`, `1g`).",
        )
        .await;
    }
    button_poll(ctx, &question, &list, length).await
}

/// Discord anketlerinin en uzun süresi (32 gün).
const NATIVE_POLL_MAX: Duration = Duration::from_secs(32 * 24 * 3600);
/// Butonlu anket bellekte yürür; bot yeniden başlarsa sonuç kaybolmasın diye kısa tutulur.
const BUTTON_POLL_MAX: Duration = Duration::from_secs(6 * 3600);
const NUMBER_EMOJIS: [&str; 10] = ["1️⃣", "2️⃣", "3️⃣", "4️⃣", "5️⃣", "6️⃣", "7️⃣", "8️⃣", "9️⃣", "🔟"];

/// Oy dağılımı çubukları: "1️⃣ Kedi ▰▰▰▱▱▱▱▱▱▱ %30 (3)"
fn poll_results(options: &[String], votes: &HashMap<UserId, usize>) -> String {
    let total = votes.len();
    options
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let count = votes.values().filter(|v| **v == i).count();
            let percent = (count * 100).checked_div(total).unwrap_or(0);
            let filled = percent / 10;
            format!(
                "{} **{option}**\n{}{} %{percent} ({count})",
                NUMBER_EMOJIS.get(i).copied().unwrap_or("▫️"),
                "▰".repeat(filled),
                "▱".repeat(10 - filled)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Butonlu anket: her üye bir oy verir (başka şıkka basarak değiştirir, aynısına basarak geri alır).
async fn button_poll(ctx: Context<'_>, question: &str, options: &[String], length: Duration) -> Result<(), Error> {
    let id = ctx.id();
    let buttons = |disabled: bool| -> Vec<CreateActionRow> {
        options
            .chunks(5)
            .enumerate()
            .map(|(row, chunk)| {
                CreateActionRow::Buttons(
                    chunk
                        .iter()
                        .enumerate()
                        .map(|(j, option)| {
                            let i = row * 5 + j;
                            let label: String = option.chars().take(70).collect();
                            CreateButton::new(format!("poll:{id}:{i}"))
                                .label(label)
                                .emoji(ReactionType::Unicode(NUMBER_EMOJIS.get(i).copied().unwrap_or("▫️").to_owned()))
                                .style(ButtonStyle::Secondary)
                                .disabled(disabled)
                        })
                        .collect(),
                )
            })
            .collect()
    };
    let ends = serenity::Timestamp::now().unix_timestamp() + i64::try_from(length.as_secs())?;
    let render = |votes: &HashMap<UserId, usize>, footer: String| {
        CreateEmbed::new()
            .title(format!("📊 {question}"))
            .description(poll_results(options, votes))
            .colour(serenity::Colour::BLURPLE)
            .footer(CreateEmbedFooter::new(footer))
    };
    let open_footer = |n: usize| format!("{n} oy · Bitiş: {} · Oyunu değiştirmek için başka şıkka bas", crate::moderation::duration::format(length));

    let mut votes: HashMap<UserId, usize> = HashMap::new();
    let reply = ctx
        .send(
            CreateReply::default()
                .content(format!("⏳ Anket <t:{ends}:R> bitiyor."))
                .embed(render(&votes, open_footer(0)))
                .components(buttons(false)),
        )
        .await?;
    let message = reply.message().await?.id;
    let deadline = Instant::now() + length;

    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let Some(press) = ComponentInteractionCollector::new(ctx.serenity_context())
            .message_id(message)
            .timeout(left)
            .await
        else {
            break;
        };
        let Some(choice) = press.data.custom_id.rsplit(':').next().and_then(|s| s.parse::<usize>().ok()).filter(|i| *i < options.len()) else {
            continue;
        };
        if votes.get(&press.user.id) == Some(&choice) {
            votes.remove(&press.user.id);
        } else {
            votes.insert(press.user.id, choice);
        }
        press
            .create_response(
                ctx,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new().embed(render(&votes, open_footer(votes.len()))),
                ),
            )
            .await?;
    }

    // Sonuç: en çok oy alan(lar).
    let counts: Vec<usize> = (0..options.len()).map(|i| votes.values().filter(|v| **v == i).count()).collect();
    let best = counts.iter().copied().max().unwrap_or(0);
    let summary = if best == 0 {
        "Kimse oy vermedi.".to_owned()
    } else {
        let winners: Vec<&str> = options
            .iter()
            .zip(&counts)
            .filter(|(_, c)| **c == best)
            .map(|(o, _)| o.as_str())
            .collect();
        if winners.len() == 1 {
            format!("🏆 Kazanan: **{}** ({best} oy)", winners.join(""))
        } else {
            format!("🤝 Berabere: **{}** ({best}'er oy)", winners.join("**, **"))
        }
    };
    reply
        .edit(
            ctx,
            CreateReply::default()
                .content(format!("✅ Anket bitti. {summary}"))
                .embed(render(&votes, format!("Toplam {} oy · Anket sona erdi", votes.len())))
                .components(buttons(true)),
        )
        .await?;
    Ok(())
}

// ===================== /hayvan =====================

/// Hayvan yakalama oyunu.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("hayvan_koleksiyon", "hayvan_siralama", "hayvan_liste", "hayvan_kanal"),
    subcommand_required,
    category = "Eğlence"
)]
pub async fn hayvan(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Yakaladığın hayvanlar.
#[poise::command(slash_command, guild_only, rename = "koleksiyon")]
async fn hayvan_koleksiyon(
    ctx: Context<'_>,
    #[rename = "kullanıcı"]
    #[description = "Bakılacak üye (boşsa sen)"]
    user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let caught: HashMap<String, i32> = animals::collection(&ctx.data().db, guild, user.id).await?.into_iter().collect();
    let total: i32 = caught.values().sum();
    let lines = ANIMALS
        .iter()
        .map(|a| match caught.get(a.id) {
            Some(n) => format!("{} **{}** ×{n} · _{}_", a.emoji, a.name, a.rarity.label()),
            None => format!("❔ ??? · _{}_", a.rarity.label()),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let embed = CreateEmbed::new()
        .title(format!("🐾 {} koleksiyonu", user.name))
        .description(lines)
        .colour(serenity::Colour::from_rgb(46, 204, 113))
        .footer(CreateEmbedFooter::new(format!("{}/{} tür · toplam {total} yakalama", caught.len(), ANIMALS.len())));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// En çok tür toplayanlar.
#[poise::command(slash_command, guild_only, rename = "sıralama")]
async fn hayvan_siralama(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let top = animals::leaderboard(&ctx.data().db, guild, 10).await?;
    let description = if top.is_empty() {
        "Henüz kimse hayvan yakalamadı. 🐾".to_owned()
    } else {
        top.iter()
            .enumerate()
            .map(|(i, (u, total, unique))| {
                let medal = ["🥇", "🥈", "🥉"].get(i).map_or_else(|| format!("`#{}`", i + 1), |m| (*m).to_owned());
                format!("{medal} {} — **{unique}/{}** tür · {total} yakalama", u.mention(), ANIMALS.len())
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    ctx.send(CreateReply::default().embed(game_embed("🐾 Hayvan sıralaması", description))).await?;
    Ok(())
}

/// Tüm hayvanlar ve nadirlikleri.
#[poise::command(slash_command, guild_only, rename = "liste")]
async fn hayvan_liste(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let lines = ANIMALS
        .iter()
        .map(|a| format!("{} **{}** · _{}_ · {}", a.emoji, a.name, a.rarity.label(), economy::cash(guild_id, a.rarity.reward())))
        .collect::<Vec<_>>()
        .join("\n");
    let embed = game_embed("📖 Hayvan ansiklopedisi", lines)
        .footer(CreateEmbedFooter::new("Yaygın %60 · Nadir %25 · Epik %11 · Efsanevi %4"));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Hayvanların belireceği kanalı ayarlar (boşsa oyun kapanır).
#[poise::command(slash_command, guild_only, rename = "kanal", required_permissions = "MANAGE_GUILD")]
async fn hayvan_kanal(
    ctx: Context<'_>,
    #[description = "Hayvanların belireceği sohbet kanalı"]
    #[channel_types("Text")]
    kanal: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let guild = guild_id(ctx)?;
    let data = ctx.data();
    data.guild_configs
        .set(&data.db, guild, Setting::AnimalChannel(kanal.as_ref().map(|c| c.id)))
        .await?;
    let text = match &kanal {
        Some(c) => format!(
            "🐾 Hayvanlar {} kanalında belirecek (her 10-20 mesajda bir). Butona ilk basan yakalar ve para kazanır!",
            c.id.mention()
        ),
        None => "Hayvan yakalama oyunu kapatıldı.".to_owned(),
    };
    reply_private(ctx, text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_bars_show_percentages() {
        let options = vec!["Kedi".to_owned(), "Köpek".to_owned()];
        let mut votes = HashMap::new();
        votes.insert(UserId::new(1), 0);
        votes.insert(UserId::new(2), 0);
        votes.insert(UserId::new(3), 1);
        votes.insert(UserId::new(4), 0);
        let text = poll_results(&options, &votes);
        assert!(text.contains("▰▰▰▰▰▰▰▱▱▱ %75 (3)"), "{text}");
        assert!(text.contains("▰▰▱▱▱▱▱▱▱▱ %25 (1)"), "{text}");
        assert!(poll_results(&options, &HashMap::new()).contains("%0 (0)"));
    }
}
