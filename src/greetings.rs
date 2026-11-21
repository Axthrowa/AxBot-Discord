//! Hazır karşılama ve veda mesajları: Star Wars, Yüzüklerin Efendisi, Harry Potter, Game of Thrones,
//! Breaking Bad, Marvel, Stranger Things, The Witcher ve Super Mario temaları. Sunucu temaları seçebilir
//! (`WelcomeSettings.themes`; boşsa hepsi karışık).
//!
//! Her metnin kendi GIF'leri vardır (metinde geçen sahne), böylece yazı ile görüntü birbirini tutar.
//! GIF'ler Giphy kimliğiyle tutulur; hepsi elle seçilip tek tek açıldığı doğrulandı.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::rand_util;

/// Bir hazır mesaj ve ona uyan GIF'ler (`https://media.giphy.com/media/<kimlik>/giphy.gif`).
pub struct Entry {
    pub text: &'static str,
    pub gifs: &'static [&'static str],
}

/// Tema: seçim anahtarı, embed başlığı, rengi ve mesajları.
pub struct Theme {
    /// `THEMES` anahtarı; karşılama ve veda temaları aynı anahtarı paylaşır.
    pub key: &'static str,
    pub title: &'static str,
    pub colour: (u8, u8, u8),
    pub entries: &'static [Entry],
}

const STAR_WARS: (u8, u8, u8) = (255, 232, 31);
const MIDDLE_EARTH: (u8, u8, u8) = (212, 175, 55);
const HOGWARTS: (u8, u8, u8) = (174, 0, 1);
/// Özel metinli karşılama / veda renkleri.
const CUSTOM_WELCOME: (u8, u8, u8) = (87, 242, 135);
const CUSTOM_GOODBYE: (u8, u8, u8) = (237, 66, 69);

/// Seçilebilir temalar (anahtar, ad). Sunucu hiçbirini seçmediyse hepsi karışık kullanılır.
pub const THEMES: &[(&str, &str)] = &[
    ("starwars", "Star Wars"),
    ("lotr", "Yüzüklerin Efendisi"),
    ("hp", "Harry Potter"),
    ("got", "Game of Thrones"),
    ("bb", "Breaking Bad"),
    ("marvel", "Marvel"),
    ("stranger", "Stranger Things"),
    ("witcher", "The Witcher"),
    ("mario", "Super Mario"),
];

/// Karşılamalar: `{kullanıcı}` etiketi içerir.
pub const WELCOME_THEMES: &[Theme] = &[
    Theme {
        key: "starwars",
        title: "⚔️ Galaksiye hoş geldin!",
        colour: STAR_WARS,
        entries: &[
            Entry {
                text: "Hello there! {kullanıcı}, General Kenobi bile seni böyle karşılardı. **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["xTiIzJSKB4l7xTouE8", "3owzWkGtQ3us1pV0qc", "xT9DPn9BInfN4w2c2A"],
            },
            Entry {
                text: "Güç'te büyük bir kıpırtı hissettim… {kullanıcı} aramıza katıldı! Seninle birlikte **{üye}** kişi olduk.",
                gifs: &["1ylORXpML0BM6U04Ry", "3o84sw7s09IvnHRFPq", "3ohuAAJPZFBgSa8kDu"],
            },
            Entry {
                text: "Güç bunda çok kuvvetli! Hoş geldin {kullanıcı}, **{sunucu}** tam sana göre.",
                gifs: &["TSmahCISxIiZ2"],
            },
            Entry {
                text: "Güç seninle, genç {kullanıcı}! **{sunucu}** Jedi Tapınağı'na hoş geldin.",
                gifs: &["8ZGtiCgA91i6Y"],
            },
            Entry {
                text: "Hiperuzay sıçraması başarılı! {kullanıcı} **{sunucu}** yörüngesine girdi. Hoş geldin, artık **{üye}** kişilik bir filoyuz.",
                gifs: &["12haGO61oFZ28w", "HjeIqm3MxURFK"],
            },
            Entry {
                text: "Aradığımız droid sen değilsin ama aradığımız üye tam olarak sensin {kullanıcı}! Hoş geldin.",
                gifs: &["4560Nv2656Gv0Lvp9F", "Eg25WblLhK8LEciBwT", "3o84sF21zQYacFcl68"],
            },
            Entry {
                text: "Roger roger! {kullanıcı} üsse giriş yaptı. **{sunucu}** birliğinin **{üye}**. üyesi göreve hazır.",
                gifs: &["181OUQHOCfde0", "3owzWjZeRSNK73D3k4"],
            },
            Entry {
                text: "Birini mi arıyordun {kullanıcı}? Buldun derim ben. **{sunucu}** sunucusuna hoş geldin, hımm.",
                gifs: &["1or67hmpL7mRW"],
            },
            Entry {
                text: "Yap ya da yapma, denemek yok. {kullanıcı} yaptı ve katıldı! Hoş geldin, genç Padawan.",
                gifs: &["pvDp7Ewpzt0o8"],
            },
            Entry {
                text: "Yol budur. {kullanıcı} da bu yolu seçti; **{sunucu}** klanına hoş geldin!",
                gifs: &["Ld77zD3fF3Run8olIt", "rV7ASrmCbqmbw9M2gk", "aCatQNctAK7PC1H4zh"],
            },
            Entry {
                text: "Grogu bile seni görünce heyecanlandı {kullanıcı}! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["j0eRJzyW7XjMpu1Pqd", "krkrHAEodHgzP72rTI"],
            },
            Entry {
                text: "Beklenmedik ama hoş bir sürpriz: {kullanıcı} geldi! Seninle birlikte **{üye}** kişi olduk.",
                gifs: &["Nszyj17J4fUKmIwQwF"],
            },
            Entry {
                text: "Chewbacca bile el sallıyor {kullanıcı}! **{sunucu}** mürettebatına hoş geldin.",
                gifs: &["loQBSxzBuYQLK"],
            },
            Entry {
                text: "Merhaba, burada kimler varmış? {kullanıcı} Bulut Şehri'ne ayak bastı. Hoş geldin!",
                gifs: &["l1KtXCGl63uIX2St2"],
            },
            Entry {
                text: "Ben senin babanım… şaka şaka, ama aileye hoş geldin {kullanıcı}! **{sunucu}** seni bekliyordu.",
                gifs: &["xT9DPpf0zTqRASyzTi"],
            },
            Entry {
                text: "Karanlık Taraf'ın gücünü bir bilsen… ama önce **{sunucu}** sunucusuna hoş geldin {kullanıcı}!",
                gifs: &["4SBtIAp4sEDxC"],
            },
            Entry {
                text: "Grogu çayını yudumlarken {kullanıcı} çıkageldi! Seninle birlikte **{üye}** kişiyiz.",
                gifs: &["Wn74RUT0vjnoU98Hnt"],
            },
            Entry {
                text: "Klan toplandı: {kullanıcı} **{sunucu}** Mandalorlarına katıldı!",
                gifs: &["KXUh7Fzn2P6xY0My42"],
            },
        ],
    },
    Theme {
        key: "lotr",
        title: "💍 Orta Dünya'ya hoş geldin!",
        colour: MIDDLE_EARTH,
        entries: &[
            Entry {
                text: "Bir büyücü asla geç kalmaz {kullanıcı}; tam da gelmesi gereken zamanda gelir. **{sunucu}** sunucusuna hoş geldin!",
                gifs: &["HVaHPyE3DUFW", "a96aV981RJ4je"],
            },
            Entry {
                text: "Çıkın Çıkmazı'nın kapısı açıldı: {kullanıcı} geldi! Dost de ve gir, **{sunucu}** artık senin de evin.",
                gifs: &["POe3JMqS3iu4M"],
            },
            Entry {
                text: "{kullanıcı} bir maceraya atılıyor! Orta Dünya'nın en güzel köşesi **{sunucu}** seni bekliyordu.",
                gifs: &["14bWswbeWGzYEo"],
            },
            Entry {
                text: "Bir macerayı paylaşacak birini arıyorduk… ve {kullanıcı} çıkageldi! **{sunucu}** kafilesine hoş geldin.",
                gifs: &["Cu0Iqwal4cPyU", "eALp83w8p9KTK"],
            },
            Entry {
                text: "Hoş geldin {kullanıcı}! İlk kahvaltıyı kaçırdın ama merak etme, ikinci kahvaltı birazdan.",
                gifs: &["OiNW0PypI1RaE", "nU1c9k6n5p6UM", "I2TqO0oJBaNoc"],
            },
            Entry {
                text: "Hoş geldin dostum! {kullanıcı} geldi, Yeşil Ejder'de bu akşam ziyafet var.",
                gifs: &["1BLN4cSZGvhOo"],
            },
            Entry {
                text: "{kullanıcı}, bizimle olduğuna sevindim. Seninle birlikte **{üye}** yoldaş olduk.",
                gifs: &["6JN4FDPIjRqX6"],
            },
            Entry {
                text: "Kılıcım emrinde {kullanıcı}! Yüzük Kardeşliği seninle birlikte **{üye}** kişi oldu.",
                gifs: &["ewkjQf8NEgzFC", "BPbViDmAlZ83BPFTdo", "ZMaGGkIA5bMME"],
            },
            Entry {
                text: "…ve baltam da! {kullanıcı}, **{sunucu}** cüceleri seni selamlıyor. Hoş geldin!",
                gifs: &["yJ1KSiTxaAw5G"],
            },
            Entry {
                text: "Kıymetlimisss geldi! {kullanıcı} artık bizim, **{sunucu}** onu kolay kolay bırakmaz.",
                gifs: &["VfwIk1LD84CI", "T1286ZxrQbzWw"],
            },
            Entry {
                text: "Shire'da şenlik var: {kullanıcı} aramıza katıldı! Artık **{üye}** kişiyiz.",
                gifs: &["zGnnFpOB1OjMQ", "1xmhhQ2ygZG8CDDTNV"],
            },
            Entry {
                text: "Mordor'a öylece yürüyerek girilmez… ama **{sunucu}** sunucusuna girilir! Hoş geldin {kullanıcı}.",
                gifs: &["smW5FBep69d3q", "m4L2xe2qgTVzq"],
            },
            Entry {
                text: "Pa-ta-tes! Haşla, ez, yahniye at… {kullanıcı} geldi, şölen başlasın!",
                gifs: &["3xRgUawnZyrny", "3X93tXeGxHv4k"],
            },
            Entry {
                text: "Yeni bir yoldaş mı? Aynen öyle. Hoş geldin {kullanıcı}, **{sunucu}** artık daha güçlü.",
                gifs: &["wPb0Er6MG6d9K"],
            },
            Entry {
                text: "Kuşluk yemeği, öğle çayı, akşam yemeği… {kullanıcı} geldi, menüyü ona da anlatalım! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["bkhkf2twSM0k8"],
            },
        ],
    },
    Theme {
        key: "hp",
        title: "⚡ Hogwarts'a hoş geldin!",
        colour: HOGWARTS,
        entries: &[
            Entry {
                text: "Baykuş mektubu yerine ulaştı! {kullanıcı}, **{sunucu}** Cadılık ve Büyücülük Okulu'na kabul edildin. Hoş geldin!",
                gifs: &["UJS4fUKBaTc8o", "SUAFlhUz4QLew"],
            },
            Entry {
                text: "Sen bir büyücüsün {kullanıcı}! Asanı hazırla, **{sunucu}** maceran şimdi başlıyor.",
                gifs: &["eax0rh3OERAYg", "IN8gg3Gci335S"],
            },
            Entry {
                text: "Ant içerim ki hayırlı bir iş peşinde değilim… Şaka şaka, hoş geldin {kullanıcı}! **{üye}**. büyücümüz sensin.",
                gifs: &["Ua7rThoBaaLs4UgX1p", "4xqGxx0rED4ze"],
            },
            Entry {
                text: "Leviosa, leviosaaa değil! {kullanıcı} ilk dersine tam zamanında yetişti. Hoş geldin.",
                gifs: &["RyLtUMBdogHvO", "iIqcyjsrIhxK0", "xUPGcmNe85ElAzFhcI"],
            },
            Entry {
                text: "Gryffindor'a 10 puan! Çünkü {kullanıcı} **{sunucu}** sunucusuna katıldı.",
                gifs: &["XymoMkORCdm4mPoqM4"],
            },
            Entry {
                text: "Seçmen Şapka kararını verdi: {kullanıcı} artık **{sunucu}** binasında! Seninle birlikte **{üye}** öğrenci olduk.",
                gifs: &["bSCVTeVLzRiwg"],
            },
            Entry {
                text: "Peron 9¾'ten kalkan tren az önce vardı ve {kullanıcı} indi. **{sunucu}** sunucusuna hoş geldin!",
                gifs: &["l0HU9ozJc6tyg6UGA", "3o7529o2bwGupeaaoo", "MQvWcYcvRTTa0"],
            },
            Entry {
                text: "**{sunucu}** okuluna hoş geldin {kullanıcı}! Profesör McGonagall seni Büyük Salon'da bekliyor.",
                gifs: &["kDHtottlroaAPDOe9o", "gIRM9VKBpprv9poTvd"],
            },
            Entry {
                text: "Büyük Salon'da bir sandalye daha doldu: hoş geldin {kullanıcı}! Ziyafet sofrası seni bekliyor.",
                gifs: &["5SRPnFvRG918k", "gbErpwcLlizvi"],
            },
            Entry {
                text: "Dumbledore asasını {kullanıcı} için kaldırıyor! Seninle birlikte **{üye}** büyücü olduk.",
                gifs: &["9H279yb0blggo"],
            },
            Entry {
                text: "Zindanda trol var!… Yok yok, sadece {kullanıcı} geldi. **{sunucu}** sunucusuna hoş geldin!",
                gifs: &["dVe7uEy7SpQVG"],
            },
            Entry {
                text: "Vuhuuu! {kullanıcı} süpürgesine atlayıp **{sunucu}** şatosuna uçtu!",
                gifs: &["TJO5x5QQM72Q0weWXN", "26BRzozg4TCBXv6QU"],
            },
            Entry {
                text: "Tekrar hoş geldin {kullanıcı}! **{sunucu}** kapılarını sana açtı; artık **{üye}** kişiyiz.",
                gifs: &["GQjbypHmHhEVGYuxoH"],
            },
            Entry {
                text: "Eveeet! Hagrid'in sevinci görülmeye değer: {kullanıcı} **{sunucu}** sunucusunda!",
                gifs: &["Pn5crrMQi6WyI5tyI0"],
            },
            Entry {
                text: "Adını Ateş Kadehi'ne sen mi attın?! Yok, {kullanıcı} **{sunucu}** sunucusuna kendi isteğiyle katıldı. Hoş geldin!",
                gifs: &["un15GSgV1JHby"],
            },
        ],
    },
    Theme {
        key: "got",
        title: "🐺 Westeros'a hoş geldin!",
        colour: (120, 134, 156),
        entries: &[
            Entry {
                text: "Kış geliyor… ama önce {kullanıcı} geldi! **{sunucu}** Westeros'una hoş geldin.",
                gifs: &["oxLsWbH1rvy2A", "ypQyYdjj6wy9a"],
            },
            Entry {
                text: "Hiçbir şey bilmiyorsun {kullanıcı}… ama **{sunucu}** sunucusunda her şeyi öğreneceksin! Hoş geldin.",
                gifs: &["NCTbhL8AG2s8g"],
            },
            Entry {
                text: "Selam {kullanıcı}! Jon Snow bile kucak açtı: **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["1xdHOERhQiM6TAjhTy"],
            },
            Entry {
                text: "Ben Fırtınada Doğan Daenerys… sen de {kullanıcı}, **{sunucu}** sarayının **{üye}**. üyesisin!",
                gifs: &["zwgmMsUOfyfK0"],
            },
            Entry {
                text: "Tanıştığımıza memnun oldum, Majesteleri {kullanıcı}! **{sunucu}** sarayına hoş geldin.",
                gifs: &["6FDUnOyfSfCk8"],
            },
            Entry {
                text: "Gece Nöbeti'ne yeni bir kardeş katıldı: {kullanıcı}! Nöbetin şimdi başlıyor.",
                gifs: &["5mdjd9dXsZA9a"],
            },
            Entry {
                text: "Kutlamalar başlasın! {kullanıcı} **{sunucu}** sofrasına katıldı; artık **{üye}** kişiyiz.",
                gifs: &["gHWaAs1YM1jmnp6bLB"],
            },
            Entry {
                text: "Khaleesi'den bir mesaj var {kullanıcı}: iyi eğlenceler! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["3o6UBipuQxa6QwnvC8"],
            },
        ],
    },
    Theme {
        key: "bb",
        title: "🧪 Laboratuvara hoş geldin!",
        colour: (52, 168, 83),
        entries: &[
            Entry {
                text: "Adımı söyle. — {kullanıcı}! — Çok haklısın. **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["NUBp5KcV0PJBe", "11z068pqgqbhHa", "zNbiX43QsqUAU"],
            },
            Entry {
                text: "Evet, bilim! {kullanıcı} laboratuvara katıldı. **{sunucu}** sunucusunda artık **{üye}** kişiyiz.",
                gifs: &["QC7UQbxq89MnL9r6AN", "8H4BFnRFNlAGY", "MBVemoHuyw9Ik"],
            },
            Entry {
                text: "Yeah, Bay White! {kullanıcı} geldi! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["hSL5vPZXFAxRZKez7E", "KD5SK9O4hgZoI"],
            },
            Entry {
                text: "Kapıyı çalan o: {kullanıcı}! **{sunucu}** ailesine hoş geldin.",
                gifs: &["3ULya3TXU3S7K", "GXqLGxgpE2tfq"],
            },
            Entry {
                text: "Başın dertte mi? Saul'u ara! Ya da en iyisi {kullanıcı} gibi **{sunucu}** sunucusuna katıl. Hoş geldin!",
                gifs: &["40dEau6bZRO3S", "PhToGscBRZki5GpvT2"],
            },
            Entry {
                text: "El sıkıştık, ortaklık başladı: {kullanıcı} **{sunucu}** ekibinde!",
                gifs: &["3gvOGxUoOXfZHHxqTI"],
            },
            Entry {
                text: "Kimya dersi başlıyor! {kullanıcı}, **{sunucu}** laboratuvarına hoş geldin.",
                gifs: &["PwLYfy05MBVVm"],
            },
        ],
    },
    Theme {
        key: "marvel",
        title: "🛡️ Yenilmezler'e hoş geldin!",
        colour: (226, 54, 54),
        entries: &[
            Entry {
                text: "Yenilmezler, toplanın! {kullanıcı} **{sunucu}** ekibine katıldı.",
                gifs: &["6aPBWbCwAOv9krAUTZ", "oY7nLzsGZk0PmSg53K"],
            },
            Entry {
                text: "Ben Iron Man değilim ama {kullanıcı} de bir efsane! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["hhkxS5FlLSwtq", "rlsHtd2YC8k0g"],
            },
            Entry {
                text: "Ben Groot! (Çevirisi: Hoş geldin {kullanıcı}, **{sunucu}** seni bekliyordu.)",
                gifs: &["xUySTMPTqceh0JkPrW", "aCYHMAJdS9DWc1Z48Y"],
            },
            Entry {
                text: "Solundan geçiyorum! {kullanıcı} koşarak **{sunucu}** sunucusuna katıldı. Artık **{üye}** kişiyiz.",
                gifs: &["1HFW57gpsSLEA"],
            },
            Entry {
                text: "Selam! Mahallenin dostu Örümcek Adam gibi {kullanıcı} da geldi. **{sunucu}** sunucusuna hoş geldin!",
                gifs: &["dUlhTFxtCbspPZVTKS"],
            },
            Entry {
                text: "Bunu bütün gün yapabilirim: yeni üyeleri karşılamak! Hoş geldin {kullanıcı}.",
                gifs: &["NENOgw8mgH0NW", "1lL0ENA9UiySk"],
            },
            Entry {
                text: "Thor Asgard'dan indi, {kullanıcı} de **{sunucu}** sunucusuna! Hoş geldin.",
                gifs: &["EOfarA6ZUqzZu"],
            },
        ],
    },
    Theme {
        key: "stranger",
        title: "🔦 Hawkins'e hoş geldin!",
        colour: (210, 32, 52),
        entries: &[
            Entry {
                text: "Arkadaşlar yalan söylemez: {kullanıcı}, **{sunucu}** sunucusuna hoş geldin!",
                gifs: &["l0MYyIQCbBAHYlcU8", "26xBC4e11Hq2gWzmM"],
            },
            Entry {
                text: "Eggo'lar hazır! {kullanıcı} **{sunucu}** ekibine katıldı.",
                gifs: &["l0HlESI0mzccH7ai4"],
            },
            Entry {
                text: "Dustin'in o kocaman gülümsemesi senin için {kullanıcı}! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["l1J9qrAVNHt1bDi2A", "3o6Ztm0Lv2eAMmNiZq"],
            },
            Entry {
                text: "Merhaba {kullanıcı}! Hawkins'in en yeni sakini oldun; artık **{üye}** kişiyiz.",
                gifs: &["l1J9QMBuKJr9Axeik", "e5RFqMGSEtdYUHM2Yj"],
            },
            Entry {
                text: "Gizli el sıkışmamızı öğrenme vakti {kullanıcı}! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["T9JPznkGDAxYC8m7yC", "9V8RorZtNTN8jf27k0"],
            },
            Entry {
                text: "Hellfire Kulübü'ne yeni bir oyuncu katıldı: {kullanıcı}! Bize ihtimallerden bahsetme.",
                gifs: &["OvVtq56YXsRhn4XK5E"],
            },
        ],
    },
    Theme {
        key: "witcher",
        title: "🐺 Kaer Morhen'e hoş geldin!",
        colour: (176, 180, 190),
        entries: &[
            Entry {
                text: "Cadı avcına bir sikke at! Ve {kullanıcı}'ye hoş geldin de: **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["KAe6LbWoqfbGqAFCZf"],
            },
            Entry {
                text: "Hmm… {kullanıcı}. Geralt bile etkilendi. **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["dW6ZHwCuHC2JKIaP7h", "d88tKlampLOqdCOnad"],
            },
            Entry {
                text: "Selamlar! {kullanıcı} **{sunucu}** meclisine katıldı; artık **{üye}** kişiyiz.",
                gifs: &["KzWAhzWD3HrJyAcLEM", "RlxcyVZj971I8emABn"],
            },
            Entry {
                text: "Eyvah, yine başlıyoruz… {kullanıcı} geldi ve macera başladı! Hoş geldin.",
                gifs: &["P86vZlmPEUrAHgh2pg", "hSQ9SwXfsQdj96hJWI"],
            },
            Entry {
                text: "Kurt Okulu'na yeni bir öğrenci: {kullanıcı}! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["Vd3MpH44gKBT2O5YGb"],
            },
        ],
    },
    Theme {
        key: "mario",
        title: "🍄 Mantar Krallığı'na hoş geldin!",
        colour: (4, 156, 216),
        entries: &[
            Entry {
                text: "Let's-a go! {kullanıcı} **{sunucu}** macerasına katıldı!",
                gifs: &["pVOlzNqLIkPX91xQix", "Z6yUH66FmYVTvBz8EO"],
            },
            Entry {
                text: "It's-a me… {kullanıcı}! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["1NTj40gK9ey0GCbMC5", "4VnJv57HY6LtNeNSEw"],
            },
            Entry {
                text: "Vuhuuu! {kullanıcı} uçarak geldi; artık **{üye}** kişiyiz.",
                gifs: &["YoocFDOj6n86ecblLc"],
            },
            Entry {
                text: "Tek bir mantar yetti: {kullanıcı} büyüdü ve **{sunucu}** sunucusuna katıldı! 🍄",
                gifs: &["XeY8iFqNuEDoNOmuID"],
            },
            Entry {
                text: "+1 can! {kullanıcı} geldi ve **{sunucu}** bir can kazandı.",
                gifs: &["mB0DmKFQdvFUMhpV1H"],
            },
            Entry {
                text: "Mamma mia! {kullanıcı} aramızda! **{sunucu}** sunucusuna hoş geldin.",
                gifs: &["K6qLE28UWGwPYwEXu1"],
            },
        ],
    },
];

/// Vedalar: ayrılan kişi etiketlenmez, `{isim}` kullanılır.
pub const GOODBYE_THEMES: &[Theme] = &[
    Theme {
        key: "starwars",
        title: "🌌 Güç seninle olsun",
        colour: STAR_WARS,
        entries: &[
            Entry {
                text: "**{isim}** hiperuzaya atlayıp gözden kayboldu. Geride **{üye}** kişi kaldık.",
                gifs: &["SiEz6hxdcJuOf2n3TE"],
            },
            Entry {
                text: "Elveda eski dost **{isim}**. Güç seninle olsun.",
                gifs: &["xTiIzuuQqb1fycYLTi", "dnIsXwv78BC4U", "LMgrZf7vBw8Qjb25Me"],
            },
            Entry {
                text: "Güç seninle olsun **{isim}**. Umarız yolun bir gün yine **{sunucu}** sunucusuna düşer.",
                gifs: &["TwmEnGgxdUT4Y", "zhPWlqR2CUQ6s", "SiUV5nib8Rm4U"],
            },
            Entry {
                text: "Hayııııır! **{isim}** aramızdan ayrıldı.",
                gifs: &["LSmULmByAQHQs", "10rHZ6K9jYvLUc", "xHwqspaBmfUMU"],
            },
            Entry {
                text: "Sen seçilmiş kişiydin **{isim}**! Bizi böyle bırakıp gitmeyecektin…",
                gifs: &["3o7abspvhYHpMnHSuc"],
            },
            Entry {
                text: "İçimde kötü bir his vardı zaten… **{isim}** ayrıldı. Artık **{üye}** kişiyiz.",
                gifs: &["uq6ILNBI6g3As"],
            },
            Entry {
                text: "**{isim}** bizi bıraktı. İsyan bir neferini kaybetti.",
                gifs: &["qWzYAuRknkpNK"],
            },
            Entry {
                text: "Yolun açık olsun **{isim}**, sana iyilikler dilerim.",
                gifs: &["3owzVQswjfHrN8DL7G"],
            },
            Entry {
                text: "Lando bile arkandan el sallıyor **{isim}**. Hoşça kal!",
                gifs: &["3ornk7TgUdhjhTYgta"],
            },
            Entry {
                text: "Bay bay **{isim}**! İmparatorluk bir askerini kaybetti.",
                gifs: &["xTiIzKvMhb6ML9gEtG"],
            },
            Entry {
                text: "Hoşça kal prenses… **{isim}** de yola çıktı.",
                gifs: &["3oYRjqJaq6Eow"],
            },
            Entry {
                text: "Bay bay! **{isim}** Ölüm Yıldızı'ndan ayrıldı.",
                gifs: &["xTiIzKvMhb6ML9gEtG"],
            },
            Entry {
                text: "Burada görülecek bir şey yok, ilerleyin… **{isim}** sunucudan ayrıldı.",
                gifs: &["10RgsuetO4uDkY"],
            },
            Entry {
                text: "Grogu kapsülüne bindi, **{isim}** de yola koyuldu. Artık **{üye}** kişiyiz.",
                gifs: &["3eTuk2nKEwcx527GyX"],
            },
        ],
    },
    Theme {
        key: "lotr",
        title: "🍂 Yolun açık olsun, yolcu",
        colour: MIDDLE_EARTH,
        entries: &[
            Entry {
                text: "Kaçın, sizi ahmaklar! **{isim}** çoktan kaçtı bile.",
                gifs: &["rBblrZDuvOaJi", "ONostKY8aj8bK", "A6PcmRqkyMOBy"],
            },
            Entry {
                text: "Hoşça kal **{isim}**. Bilbo gibi bir anda ortadan kayboldu.",
                gifs: &["55f9KaPecifXkHYn86", "ZDpdicZ4nEgZa", "l49FqlUguNsGDNCGk", "97DF94pXkcvIFHsz9P"],
            },
            Entry {
                text: "Gidemezsin **{isim}**! …Gitti bile. Geride **{üye}** kişi kaldık.",
                gifs: &["2tLyBxzziMXYdibWHw"],
            },
            Entry {
                text: "**{isim}** Gri Limanlar'dan yelken açtı. Shire onsuz biraz daha sessiz.",
                gifs: &["6ia6DZpd8ZcBy", "as1PZn0ZUdzPy"],
            },
            Entry {
                text: "Bitti. **{isim}** için **{sunucu}** macerası burada sona erdi.",
                gifs: &["zCME2Cd20Czvy", "Ajhi3tsHaGBB6"],
            },
            Entry {
                text: "Peki o zaman **{isim}**, sırların sende kalsın. Yolun açık olsun!",
                gifs: &["4YY4DnqeUDBXNTcYMu"],
            },
            Entry {
                text: "GEÇEMEZSİN! …Ama çıkabilirsin. **{isim}** aramızdan ayrıldı.",
                gifs: &["njYrp176NQsHS", "A2GXvZmG73hQc"],
            },
            Entry {
                text: "**{isim}** tek başına yola koyuldu. Artık **{üye}** kişilik bir kafileyiz.",
                gifs: &["pzqEdzCFRgGI0"],
            },
            Entry {
                text: "“Evet Bay Frodo, artık bitti.” **{isim}** yolculuğunu tamamladı.",
                gifs: &["hHM8RtQCVrDgc"],
            },
            Entry {
                text: "“Burada hiçbir gücün yok!” **{isim}** sunucudan ayrıldı. Artık **{üye}** kişiyiz.",
                gifs: &["RX3vhj311HKLe"],
            },
            Entry {
                text: "Seni ahmak Took! **{isim}** de gitti bile…",
                gifs: &["67ih46sJYC6kw"],
            },
        ],
    },
    Theme {
        key: "hp",
        title: "🪄 Yaramazlık tamamlandı",
        colour: HOGWARTS,
        entries: &[
            Entry {
                text: "Hagrid peronda el sallıyor: güle güle **{isim}**!",
                gifs: &["2gGEWrIGVioP6", "qJ1IJW0SOtdq8"],
            },
            Entry {
                text: "**{isim}** Hogwarts Ekspresi'ne binip gitti. Geride **{üye}** öğrenci kaldık.",
                gifs: &["VBGoqEIX0TWX5Kl0i0", "hujejOtss7zG0", "7tn6Y2NgbLw8o"],
            },
            Entry {
                text: "**{isim}** artık özgür bir ev cini! Dobby gibi çorabını kapıp gitti.",
                gifs: &["wUrc9zZpRhRrW", "ACIGqfRvzoT3q", "l0K4ahAZsInClnTeE"],
            },
            Entry {
                text: "**{isim}** artık kimsenin hizmetinde değil; **{sunucu}** sunucusundan ayrıldı.",
                gifs: &["B9wVZdspOXzfG"],
            },
            Entry {
                text: "Yaramazlık tamamlandı. **{isim}** haritadan silindi.",
                gifs: &["OWoDhovPw5Cvi00ooi", "HIQqZdkbYIPydNTrM0"],
            },
            Entry {
                text: "\"Bunca zaman sonra bile mi?\" \"Daima.\" Hoşça kal **{isim}**.",
                gifs: &["10osILvZ4ez7ws", "11xGIOJ4ffe9vW"],
            },
            Entry {
                text: "Hızır Otobüs **{isim}** için kapıya yanaştı. Artık **{üye}** kişiyiz.",
                gifs: &["3o6fIXmbgfcGRSTyF2"],
            },
            Entry {
                text: "Hoşça kal dostum! **{isim}** trenin penceresinden el sallayarak Hogwarts'tan ayrıldı.",
                gifs: &["VBGoqEIX0TWX5Kl0i0"],
            },
            Entry {
                text: "OLAMAAAZ! **{isim}** sunucudan ayrıldı. Artık **{üye}** kişiyiz.",
                gifs: &["xEpTspH9hGwHS"],
            },
            Entry {
                text: "**{isim}** Sırlar Odası'ndan son bir kez el salladı. Hoşça kal!",
                gifs: &["7zoOcZshec8nqdFKeG"],
            },
        ],
    },
    Theme {
        key: "got",
        title: "⚔️ Valar morghulis",
        colour: (120, 134, 156),
        entries: &[
            Entry {
                text: "Valar morghulis… **{isim}** aramızdan ayrıldı. Artık **{üye}** kişiyiz.",
                gifs: &["My9qYkrpYLJe", "ghunn7VEJjj32"],
            },
            Entry {
                text: "Ve şimdi onun nöbeti sona erdi. Elveda **{isim}**.",
                gifs: &["qZLRsm8SFzBPAz5L1W", "1eei0cxrWEZby"],
            },
            Entry {
                text: "Kapıyı tut! Ama **{isim}** tutamadı ve gitti… Hodor.",
                gifs: &["3oz8xLAu4F6hypfPpe", "Zrq2FgRy6w1eU"],
            },
            Entry {
                text: "Sanırım bu bir veda… Yolun açık olsun **{isim}**, savaşlarda bol şans.",
                gifs: &["vGfq40OBLIehW", "l41YnSDHZiUpTK5gI"],
            },
            Entry {
                text: "Ölmüş olan asla ölmez… **{isim}** gitti ama hatırası **{sunucu}** sunucusunda kalacak.",
                gifs: &["dN7NWfaBMdG5G"],
            },
        ],
    },
    Theme {
        key: "bb",
        title: "🌵 Çölde iz bırakmadan",
        colour: (52, 168, 83),
        entries: &[
            Entry {
                text: "“Tehlike benim!” diyerek **{isim}** kapıdan çıktı. Artık **{üye}** kişiyiz.",
                gifs: &["kj8M1xRRriY3m"],
            },
            Entry {
                text: "Gitmelisin, hem de hemen… **{isim}** sunucudan ayrıldı.",
                gifs: &["3oFzlZMqJnMNqWeczC"],
            },
            Entry {
                text: "**{isim}** çölün ortasında iz bırakmadan kayboldu. Hoşça kal!",
                gifs: &["Eg58k0CdpOi6T3cm0x"],
            },
            Entry {
                text: "Ah hayır… **{isim}** aramızdan ayrıldı. Artık **{üye}** kişiyiz.",
                gifs: &["3o6gEgkb5xqAyMw5Og", "xT0GqgeTVaAdWZD1uw"],
            },
            Entry {
                text: "“Beni yakaladın.” **{isim}** sonunda pes etti ve sunucudan ayrıldı.",
                gifs: &["a8DfVAblirrOg"],
            },
        ],
    },
    Theme {
        key: "marvel",
        title: "🫰 Şıklat ve gitti",
        colour: (226, 54, 54),
        entries: &[
            Entry {
                text: "“Kendimi pek iyi hissetmiyorum…” dedi **{isim}** ve toz olup gitti. Artık **{üye}** kişiyiz.",
                gifs: &["XzkGfRsUweB9ouLEsE", "AiEr9b7sX5VKIoIvQL"],
            },
            Entry {
                text: "Thanos parmaklarını şıklattı ve **{isim}** yok oldu…",
                gifs: &["iIFS20pNoCg1EEVodC", "mFbSJaPRTOO1VWRg9k"],
            },
            Entry {
                text: "Seni 3000 kere seviyoruz **{isim}**. Hoşça kal!",
                gifs: &["JQMeUrXpiZ3M79iOQl", "J5Xd9gM8TphqCmfy2k"],
            },
            Entry {
                text: "Loki bile el salladı: güle güle **{isim}**.",
                gifs: &["UYzSmRaDu5eVpbvZqw"],
            },
            Entry {
                text: "Tony Stark'tan kısa bir veda: “Bay bay.” **{isim}** sunucudan ayrıldı.",
                gifs: &["108tPuaiBRDYl2"],
            },
        ],
    },
    Theme {
        key: "stranger",
        title: "🌑 Baş Aşağı Dünya'ya geçti",
        colour: (210, 32, 52),
        entries: &[
            Entry {
                text: "**{isim}** Baş Aşağı Dünya'ya geçti… Artık **{üye}** kişiyiz.",
                gifs: &["F1cehBDCMnsukCPUXo"],
            },
            Entry {
                text: "Fenerleri yakın, **{isim}**'i arıyoruz… ama sunucudan ayrılmış.",
                gifs: &["PwNtgAD5ySwUnm3kX3"],
            },
            Entry {
                text: "**{isim}** gitti ve Hawkins biraz daha sessizleşti…",
                gifs: &["ITpkRPeE6QElosh0u5"],
            },
        ],
    },
    Theme {
        key: "witcher",
        title: "🌫️ Yolun açık olsun, cadı avcısı",
        colour: (176, 180, 190),
        entries: &[
            Entry {
                text: "**{isim}** Roach'a atladı ve ufka doğru dörtnala gitti. Yolun açık olsun!",
                gifs: &["QWdT60eDhFfcKgPG2T", "ZRMo8QM6M7lM4"],
            },
            Entry {
                text: "**{isim}** son bir sıcak banyo yaptı ve sessizce yola koyuldu. Hoşça kal!",
                gifs: &["LPZVefuhqMoXq9ITLu", "dWSy8T6SyDVj4ey7qX"],
            },
            Entry {
                text: "Karanlıkta kayboldu… **{isim}** yeni bir sözleşmenin peşinde. Artık **{üye}** kişiyiz.",
                gifs: &["gKBxOSwLmd7lJas3eR"],
            },
            Entry {
                text: "Doğa yürüyüşü vakti! **{isim}** ormana daldı ve gitti.",
                gifs: &["ImzqcPi1FNcFEN7dF6"],
            },
        ],
    },
    Theme {
        key: "mario",
        title: "🏁 Oyun bitti",
        colour: (4, 156, 216),
        entries: &[
            Entry {
                text: "Oyun bitti… **{isim}** oyundan çıktı. Artık **{üye}** kişiyiz.",
                gifs: &["12GphGoFazeoso", "JUSwkiO1Eh5K43ruN0"],
            },
            Entry {
                text: "Sanırım bu bir veda… **{isim}** gitti.",
                gifs: &["MmuJfAxgysL5LajJMj"],
            },
            Entry {
                text: "**{isim}** yeşil boruya girdi ve bir daha çıkmadı…",
                gifs: &["IpXPcJpZZvpW5G9KH2"],
            },
            Entry {
                text: "Byeeee! Prenses Peach bile el salladı: hoşça kal **{isim}**.",
                gifs: &["ftaZvOiuZgTDitY60y"],
            },
        ],
    },
];

/// Son seçilen metin; aynı metin art arda gelmesin diye.
static LAST_WELCOME: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_GOODBYE: AtomicU64 = AtomicU64::new(u64::MAX);

/// Seçilmiş mesaj: şablon metni ve (hazır temalarda) başlık + GIF.
pub struct Greeting {
    pub template: String,
    pub title: Option<&'static str>,
    pub colour: (u8, u8, u8),
    pub gif: Option<String>,
}

/// Özel metin varsa onu (temasız), yoksa hazır temalı karşılamalardan birini seçer.
/// `allowed`: sunucunun seçtiği tema anahtarları (boşsa hepsi).
pub fn pick_welcome(custom: Option<&str>, allowed: &[String]) -> Greeting {
    pick(custom, WELCOME_THEMES, allowed, &LAST_WELCOME, CUSTOM_WELCOME)
}

/// Özel metin varsa onu (temasız), yoksa hazır temalı vedalardan birini seçer.
pub fn pick_goodbye(custom: Option<&str>, allowed: &[String]) -> Greeting {
    pick(custom, GOODBYE_THEMES, allowed, &LAST_GOODBYE, CUSTOM_GOODBYE)
}

fn pick(custom: Option<&str>, themes: &[Theme], allowed: &[String], last: &AtomicU64, plain: (u8, u8, u8)) -> Greeting {
    if let Some(template) = pick_custom(custom, last) {
        return Greeting { template, title: None, colour: plain, gif: None };
    }
    let chosen: Vec<&Theme> = themes.iter().filter(|t| allowed.iter().any(|a| a == t.key)).collect();
    // Seçim yoksa ya da seçilen anahtarlar artık yoksa hepsi kullanılır.
    let pool: Vec<&Theme> = if chosen.is_empty() { themes.iter().collect() } else { chosen };
    pick_themed(&pool, last).unwrap_or(Greeting { template: String::new(), title: None, colour: plain, gif: None })
}

/// Tema anahtarı geçerli mi?
pub fn is_theme(key: &str) -> bool {
    THEMES.iter().any(|(k, _)| *k == key)
}

/// Özel metin `|` ile ayrılmış birden fazla seçenek içerebilir; hiç seçenek yoksa `None`.
fn pick_custom(custom: Option<&str>, last: &AtomicU64) -> Option<String> {
    let options: Vec<&str> = custom?.split('|').map(str::trim).filter(|t| !t.is_empty()).collect();
    options.get(pick_index(options.len(), last)).map(|t| (*t).to_owned())
}

/// Bütün temaların mesajları tek liste gibi sayılır; böylece aynı metin art arda gelmez.
fn pick_themed(themes: &[&Theme], last: &AtomicU64) -> Option<Greeting> {
    let total: usize = themes.iter().map(|t| t.entries.len()).sum();
    let mut index = pick_index(total, last);
    for theme in themes {
        if let Some(entry) = theme.entries.get(index) {
            let gif = usize::try_from(rand_util::below(entry.gifs.len() as u64))
                .ok()
                .and_then(|i| entry.gifs.get(i))
                .map(|id| format!("https://media.giphy.com/media/{id}/giphy.gif"));
            return Some(Greeting {
                template: entry.text.to_owned(),
                title: Some(theme.title),
                colour: theme.colour,
                gif,
            });
        }
        index -= theme.entries.len();
    }
    None
}

/// `0..count` aralığından, mümkünse bir önceki seçimden farklı bir sıra numarası seçer.
fn pick_index(count: usize, last: &AtomicU64) -> usize {
    let count = count as u64;
    if count == 0 {
        return 0;
    }
    let previous = last.load(Ordering::Relaxed);
    let mut index = rand_util::below(count);
    if count > 1 && index == previous {
        // Öncekini atlayıp kalanlardan eşit olasılıkla seç.
        index = (index + 1 + rand_util::below(count - 1)) % count;
    }
    last.store(index, Ordering::Relaxed);
    usize::try_from(index).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn custom_templates_split_on_pipe_and_never_repeat() {
        let last = AtomicU64::new(u64::MAX);
        let mut previous = String::new();
        for _ in 0..50 {
            let picked = pick_custom(Some(" A | B ||C| "), &last).unwrap_or_default();
            assert!(["A", "B", "C"].contains(&picked.as_str()), "beklenmeyen: {picked}");
            assert_ne!(picked, previous);
            previous = picked;
        }
    }

    #[test]
    fn empty_custom_falls_back_to_themes() {
        let last = AtomicU64::new(u64::MAX);
        assert!(pick_custom(Some(" | "), &last).is_none());
        assert!(pick_custom(None, &last).is_none());
        assert!(pick_goodbye(Some(" | "), &[]).gif.is_some());
        let custom = pick_welcome(Some("Selam {isim}"), &[]);
        assert_eq!(custom.template, "Selam {isim}");
        assert!(custom.title.is_none() && custom.gif.is_none());
    }

    #[test]
    fn themed_picks_pair_text_with_its_own_gif_and_never_repeat() {
        for themes in [WELCOME_THEMES, GOODBYE_THEMES] {
            let last = AtomicU64::new(u64::MAX);
            let mut previous = String::new();
            for _ in 0..300 {
                let pool: Vec<&Theme> = themes.iter().collect();
                let picked = pick_themed(&pool, &last);
                assert!(picked.is_some(), "tema boş");
                let Some(picked) = picked else { return };
                let entry = themes
                    .iter()
                    .filter(|t| Some(t.title) == picked.title)
                    .flat_map(|t| t.entries)
                    .find(|e| e.text == picked.template);
                let gif_id = picked
                    .gif
                    .as_deref()
                    .and_then(|g| g.strip_prefix("https://media.giphy.com/media/"))
                    .and_then(|g| g.strip_suffix("/giphy.gif"))
                    .unwrap_or_default();
                assert!(entry.is_some_and(|e| e.gifs.contains(&gif_id)), "GIF metne ait değil: {}", picked.template);
                assert_ne!(picked.template, previous);
                previous = picked.template;
            }
        }
    }

    #[test]
    fn chosen_theme_is_respected() {
        let only = vec!["got".to_owned()];
        for _ in 0..40 {
            let w = pick_welcome(None, &only);
            assert!(WELCOME_THEMES.iter().any(|t| t.key == "got" && Some(t.title) == w.title));
            let g = pick_goodbye(None, &only);
            assert!(GOODBYE_THEMES.iter().any(|t| t.key == "got" && Some(t.title) == g.title));
        }
        // Bilinmeyen anahtar: hepsi kullanılır (boş kalmaz).
        assert!(pick_welcome(None, &["yok".to_owned()]).gif.is_some());
    }

    #[test]
    fn every_theme_has_welcome_and_goodbye() {
        for (key, _) in THEMES {
            assert!(WELCOME_THEMES.iter().any(|t| t.key == *key), "karşılama yok: {key}");
            assert!(GOODBYE_THEMES.iter().any(|t| t.key == *key), "veda yok: {key}");
        }
        for theme in WELCOME_THEMES.iter().chain(GOODBYE_THEMES) {
            assert!(is_theme(theme.key), "listede yok: {}", theme.key);
        }
    }

    #[test]
    fn every_entry_is_well_formed() {
        let mut texts = HashSet::new();
        for theme in WELCOME_THEMES {
            for entry in theme.entries {
                assert!(entry.text.contains("{kullanıcı}"), "etiket yok: {}", entry.text);
            }
        }
        for theme in GOODBYE_THEMES {
            for entry in theme.entries {
                assert!(entry.text.contains("{isim}"), "isim yok: {}", entry.text);
                assert!(!entry.text.contains("{kullanıcı}"), "vedada etiket olmamalı: {}", entry.text);
            }
        }
        for entry in WELCOME_THEMES.iter().chain(GOODBYE_THEMES).flat_map(|t| t.entries) {
            assert!(texts.insert(entry.text), "yinelenen metin: {}", entry.text);
            assert!(!entry.gifs.is_empty(), "GIF yok: {}", entry.text);
            for gif in entry.gifs {
                assert!(gif.chars().all(|c| c.is_ascii_alphanumeric()), "geçersiz kimlik: {gif}");
            }
        }
    }
}
