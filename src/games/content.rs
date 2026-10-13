//! Oyun içerikleri: kelimeler, bilgi soruları, doğruluk/cesaret, 8ball cevapları, hayvanlar.

use crate::rand_util;

pub fn pick<T>(items: &[T]) -> Option<&T> {
    items.get(usize::try_from(rand_util::below(items.len() as u64)).unwrap_or(0))
}

/// Son seçilenlerde olmayan rastgele bir sıra numarası seçer ve geçmişe ekler.
/// `keep` kadar geçmiş tutulur (listenin yarısını geçmez ki seçenek kalsın).
pub fn pick_fresh(len: usize, recent: &mut std::collections::VecDeque<usize>, keep: usize) -> usize {
    let keep = keep.min(len / 2);
    let fresh: Vec<usize> = (0..len).filter(|i| !recent.contains(i)).collect();
    let index = pick(&fresh).copied().unwrap_or(0);
    recent.push_back(index);
    while recent.len() > keep {
        recent.pop_front();
    }
    index
}

// ===================== Adam asmaca =====================

pub const WORDS: &[(&str, &[&str])] = &[
    ("Hayvan", &["kaplan", "zürafa", "penguen", "kelebek", "timsah", "sincap", "papağan", "yunus", "kirpi", "karınca", "baykuş", "ahtapot", "kanguru", "leopar", "örümcek"]),
    ("Meyve", &["karpuz", "çilek", "ananas", "böğürtlen", "mandalina", "kayısı", "şeftali", "armut", "kiraz", "üzüm", "greyfurt", "ahududu"]),
    ("Şehir", &["ankara", "istanbul", "izmir", "trabzon", "eskişehir", "diyarbakır", "gaziantep", "çanakkale", "antalya", "erzurum", "kastamonu", "şanlıurfa"]),
    ("Eşya", &["bilgisayar", "buzdolabı", "şemsiye", "televizyon", "kulaklık", "anahtar", "merdiven", "yastık", "çamaşır", "telefon", "gözlük", "bardak"]),
    ("Spor", &["basketbol", "voleybol", "futbol", "yüzme", "tenis", "okçuluk", "güreş", "hentbol", "kayak", "eskrim"]),
    ("Meslek", &["öğretmen", "doktor", "mühendis", "itfaiyeci", "aşçı", "pilot", "avukat", "hemşire", "mimar", "eczacı"]),
];

pub const HANGMAN_STAGES: [&str; 7] = [
    "```\n  +---+\n  |   |\n      |\n      |\n      |\n      |\n=========```",
    "```\n  +---+\n  |   |\n  O   |\n      |\n      |\n      |\n=========```",
    "```\n  +---+\n  |   |\n  O   |\n  |   |\n      |\n      |\n=========```",
    "```\n  +---+\n  |   |\n  O   |\n /|   |\n      |\n      |\n=========```",
    "```\n  +---+\n  |   |\n  O   |\n /|\\  |\n      |\n      |\n=========```",
    "```\n  +---+\n  |   |\n  O   |\n /|\\  |\n /    |\n      |\n=========```",
    "```\n  +---+\n  |   |\n  O   |\n /|\\  |\n / \\  |\n      |\n=========```",
];

// ===================== Bilgi yarışması =====================

pub struct Question {
    pub category: &'static str,
    pub text: &'static str,
    /// İlk seçenek doğru cevaptır; gösterilirken karıştırılır.
    pub options: [&'static str; 4],
}

macro_rules! q {
    ($cat:literal, $text:literal, $right:literal, $a:literal, $b:literal, $c:literal) => {
        Question { category: $cat, text: $text, options: [$right, $a, $b, $c] }
    };
}

pub const QUESTIONS: &[Question] = &[
    q!("Coğrafya", "Türkiye'nin başkenti neresidir?", "Ankara", "İstanbul", "İzmir", "Bursa"),
    q!("Tarih", "Türkiye Cumhuriyeti hangi yıl ilan edildi?", "1923", "1920", "1919", "1938"),
    q!("Coğrafya", "Dünyanın en büyük okyanusu hangisidir?", "Büyük Okyanus (Pasifik)", "Atlas Okyanusu", "Hint Okyanusu", "Arktik Okyanusu"),
    q!("Bilim", "Güneş sistemindeki en büyük gezegen hangisidir?", "Jüpiter", "Satürn", "Neptün", "Dünya"),
    q!("Bilim", "Suyun kimyasal formülü nedir?", "H₂O", "CO₂", "O₂", "H₂O₂"),
    q!("Edebiyat", "İstiklal Marşı'nın şairi kimdir?", "Mehmet Akif Ersoy", "Namık Kemal", "Nazım Hikmet", "Ziya Gökalp"),
    q!("Coğrafya", "Türkiye'nin en yüksek dağı hangisidir?", "Ağrı Dağı", "Erciyes Dağı", "Uludağ", "Süphan Dağı"),
    q!("Coğrafya", "Tamamı Türkiye sınırları içinde olan en uzun nehir hangisidir?", "Kızılırmak", "Fırat", "Sakarya", "Yeşilırmak"),
    q!("Sanat", "Mona Lisa tablosunu kim yaptı?", "Leonardo da Vinci", "Michelangelo", "Rafael", "Vincent van Gogh"),
    q!("Matematik", "En küçük asal sayı kaçtır?", "2", "1", "3", "0"),
    q!("Spor", "Futbolda bir takım sahada kaç oyuncuyla oynar?", "11", "10", "9", "12"),
    q!("Bilim", "Altının kimyasal sembolü nedir?", "Au", "Ag", "Al", "Fe"),
    q!("Bilim", "\"Kızıl gezegen\" olarak bilinen gezegen hangisidir?", "Mars", "Venüs", "Merkür", "Jüpiter"),
    q!("Tarih", "Osmanlı Devleti'nin kurucusu kimdir?", "Osman Bey", "Orhan Bey", "Ertuğrul Gazi", "Fatih Sultan Mehmet"),
    q!("Tarih", "İstanbul hangi yıl fethedildi?", "1453", "1071", "1299", "1517"),
    q!("Tarih", "Malazgirt Meydan Muharebesi hangi yıl yapıldı?", "1071", "1453", "1299", "1176"),
    q!("Coğrafya", "Türkiye'de kaç il vardır?", "81", "79", "80", "82"),
    q!("Matematik", "Bir saatte kaç saniye vardır?", "3600", "360", "6000", "1440"),
    q!("Edebiyat", "\"Romeo ve Juliet\" kimin eseridir?", "William Shakespeare", "Charles Dickens", "Victor Hugo", "Goethe"),
    q!("Bilim", "İnsan vücudunun en büyük organı hangisidir?", "Deri", "Karaciğer", "Akciğer", "Beyin"),
    q!("Genel", "Gökkuşağında kaç renk vardır?", "7", "5", "6", "8"),
    q!("Coğrafya", "Japonya'nın başkenti neresidir?", "Tokyo", "Kyoto", "Osaka", "Seul"),
    q!("Coğrafya", "Avustralya'nın başkenti neresidir?", "Kanberra", "Sidney", "Melbourne", "Perth"),
    q!("Müzik", "Standart bir piyanoda kaç tuş vardır?", "88", "76", "61", "100"),
    q!("Coğrafya", "Dünyanın en yüksek dağı hangisidir?", "Everest", "K2", "Kilimanjaro", "Mont Blanc"),
    q!("Bilim", "Penisilini kim keşfetti?", "Alexander Fleming", "Louis Pasteur", "Marie Curie", "Isaac Newton"),
    q!("Matematik", "Bir üçgenin iç açıları toplamı kaç derecedir?", "180", "90", "360", "270"),
    q!("Tarih", "Mustafa Kemal Atatürk hangi şehirde doğdu?", "Selanik", "İstanbul", "Manisa", "Bursa"),
    q!("Tarih", "Çanakkale Kara Savaşları hangi yıl yapıldı?", "1915", "1918", "1922", "1912"),
    q!("Oyun", "Minecraft'ı ilk geliştiren kişi kimdir?", "Markus \"Notch\" Persson", "Gabe Newell", "Hideo Kojima", "Shigeru Miyamoto"),
    q!("Oyun", "\"Victory Royale\" hangi oyunda kazanılır?", "Fortnite", "PUBG", "Apex Legends", "Valorant"),
    q!("Teknoloji", "Discord hangi yıl yayınlandı?", "2015", "2013", "2017", "2019"),
    q!("Oyun", "Satrançta oyunun başında her oyuncunun kaç piyonu vardır?", "8", "6", "10", "7"),
    q!("Spor", "Bir futbol maçının normal süresi kaç dakikadır?", "90", "80", "100", "60"),
    q!("Bilim", "Elmas hangi elementten oluşur?", "Karbon", "Silisyum", "Demir", "Kükürt"),
    q!("Tarih", "Ay'a ayak basan ilk insan kimdir?", "Neil Armstrong", "Buzz Aldrin", "Yuri Gagarin", "Michael Collins"),
    q!("Tarih", "Uzaya giden ilk insan kimdir?", "Yuri Gagarin", "Neil Armstrong", "John Glenn", "Valentina Tereşkova"),
    q!("Coğrafya", "Türkiye'nin en büyük gölü hangisidir?", "Van Gölü", "Tuz Gölü", "Beyşehir Gölü", "Eğirdir Gölü"),
    q!("Genel", "Yaygın kabule göre dünyada kaç kıta vardır?", "7", "5", "6", "8"),
    q!("Edebiyat", "\"Sefiller\" romanının yazarı kimdir?", "Victor Hugo", "Lev Tolstoy", "Fyodor Dostoyevski", "Honoré de Balzac"),
    q!("Edebiyat", "Nobel Edebiyat Ödülü'nü kazanan Türk yazar kimdir?", "Orhan Pamuk", "Yaşar Kemal", "Elif Şafak", "Sabahattin Ali"),
    q!("Bilim", "Aşağıdakilerden hangisi memelidir?", "Yunus", "Köpekbalığı", "Timsah", "Penguen"),
    q!("Bilim", "Periyodik tabloda \"O\" sembolü hangi elementtir?", "Oksijen", "Osmiyum", "Altın", "Ozon"),
    q!("Matematik", "12 × 12 kaçtır?", "144", "124", "132", "154"),
    q!("Teknoloji", "Bilgisayarın \"beyni\" olarak bilinen parça hangisidir?", "İşlemci (CPU)", "RAM", "SSD", "Ekran kartı"),
    q!("Tarih", "Cumhuriyet Bayramı hangi gün kutlanır?", "29 Ekim", "23 Nisan", "19 Mayıs", "30 Ağustos"),
    q!("Bilim", "Işık boşlukta saniyede yaklaşık kaç kilometre yol alır?", "300.000", "30.000", "3.000", "3.000.000"),
    q!("Coğrafya", "Hangi ülke hem Avrupa'da hem Asya'da toprağa sahiptir?", "Türkiye", "İtalya", "Mısır", "Japonya"),
    // ---- Coğrafya
    q!("Coğrafya", "Yüzölçümü en büyük ilimiz hangisidir?", "Konya", "Sivas", "Erzurum", "Ankara"),
    q!("Coğrafya", "Hangi ülkenin Karadeniz'e kıyısı yoktur?", "Macaristan", "Romanya", "Bulgaristan", "Gürcistan"),
    q!("Coğrafya", "Mısır'ın başkenti neresidir?", "Kahire", "İskenderiye", "Gize", "Luksor"),
    q!("Coğrafya", "Kanada'nın başkenti neresidir?", "Ottawa", "Toronto", "Vancouver", "Montreal"),
    q!("Coğrafya", "Brezilya'nın başkenti neresidir?", "Brasília", "Rio de Janeiro", "São Paulo", "Salvador"),
    q!("Coğrafya", "Yüzölçümü en büyük ülke hangisidir?", "Rusya", "Kanada", "Çin", "ABD"),
    q!("Coğrafya", "Dünyanın en büyük sıcak çölü hangisidir?", "Sahra", "Gobi", "Kalahari", "Arabistan Çölü"),
    q!("Coğrafya", "Kapadokya hangi coğrafi bölgemizdedir?", "İç Anadolu", "Doğu Anadolu", "Ege", "Akdeniz"),
    q!("Coğrafya", "Pamukkale travertenleri hangi ilimizdedir?", "Denizli", "Aydın", "Muğla", "Afyonkarahisar"),
    q!("Coğrafya", "Sümela Manastırı hangi ilimizdedir?", "Trabzon", "Rize", "Giresun", "Artvin"),
    q!("Coğrafya", "Kommagene heykelleriyle ünlü Nemrut Dağı hangi ilimizdedir?", "Adıyaman", "Malatya", "Şanlıurfa", "Gaziantep"),
    q!("Coğrafya", "İstanbul Boğazı hangi iki denizi birbirine bağlar?", "Karadeniz ve Marmara", "Marmara ve Ege", "Ege ve Akdeniz", "Karadeniz ve Ege"),
    q!("Coğrafya", "Türkiye kaç ülkeyle kara sınırına sahiptir?", "8", "6", "7", "9"),
    q!("Coğrafya", "Dünyanın en derin okyanus çukuru hangisidir?", "Mariana Çukuru", "Porto Riko Çukuru", "Tonga Çukuru", "Java Çukuru"),
    q!("Coğrafya", "İtalya'nın başkenti neresidir?", "Roma", "Milano", "Venedik", "Napoli"),
    q!("Coğrafya", "Almanya'nın başkenti neresidir?", "Berlin", "Münih", "Hamburg", "Frankfurt"),
    // ---- Tarih
    q!("Tarih", "Mustafa Kemal Samsun'a hangi tarihte çıktı?", "19 Mayıs 1919", "23 Nisan 1920", "30 Ağustos 1922", "29 Ekim 1923"),
    q!("Tarih", "TBMM hangi tarihte açıldı?", "23 Nisan 1920", "19 Mayıs 1919", "29 Ekim 1923", "1 Kasım 1922"),
    q!("Tarih", "Büyük Taarruz'un zaferle sonuçlandığı Başkomutanlık Meydan Muharebesi hangi tarihtedir?", "30 Ağustos 1922", "9 Eylül 1922", "26 Ağustos 1921", "29 Ekim 1923"),
    q!("Tarih", "Türkiye Cumhuriyeti'nin ilk cumhurbaşkanı kimdir?", "Mustafa Kemal Atatürk", "İsmet İnönü", "Celal Bayar", "Fevzi Çakmak"),
    q!("Tarih", "Türkiye Cumhuriyeti'nin ikinci cumhurbaşkanı kimdir?", "İsmet İnönü", "Celal Bayar", "Cemal Gürsel", "Adnan Menderes"),
    q!("Tarih", "Türk kadınları milletvekili seçme ve seçilme hakkını hangi yıl kazandı?", "1934", "1923", "1930", "1946"),
    q!("Tarih", "Harf Devrimi (yeni Türk alfabesinin kabulü) hangi yıl yapıldı?", "1928", "1923", "1925", "1934"),
    q!("Tarih", "Birinci Dünya Savaşı hangi yıl başladı?", "1914", "1912", "1918", "1939"),
    q!("Tarih", "İkinci Dünya Savaşı hangi yıl sona erdi?", "1945", "1944", "1946", "1939"),
    q!("Tarih", "Berlin Duvarı hangi yıl yıkıldı?", "1989", "1985", "1990", "1991"),
    q!("Tarih", "Kanuni Sultan Süleyman kaçıncı Osmanlı padişahıdır?", "10.", "8.", "9.", "12."),
    q!("Tarih", "1492'de Amerika kıtasına ulaşan kâşif kimdir?", "Kristof Kolomb", "Vasco da Gama", "Macellan", "Amerigo Vespucci"),
    q!("Tarih", "Soyadı Kanunu hangi yıl kabul edildi?", "1934", "1926", "1928", "1938"),
    // ---- Bilim
    q!("Bilim", "Yetişkin bir insanın iskeletinde kaç kemik vardır?", "206", "180", "212", "250"),
    q!("Bilim", "Bitkiler fotosentezde havadan hangi gazı alır?", "Karbondioksit", "Oksijen", "Azot", "Hidrojen"),
    q!("Bilim", "Güneş'e en yakın gezegen hangisidir?", "Merkür", "Venüs", "Mars", "Dünya"),
    q!("Bilim", "En belirgin halkalarıyla bilinen gezegen hangisidir?", "Satürn", "Jüpiter", "Uranüs", "Neptün"),
    q!("Bilim", "Su deniz seviyesinde kaç derecede kaynar?", "100 °C", "90 °C", "80 °C", "120 °C"),
    q!("Bilim", "Kanı vücuda pompalayan organ hangisidir?", "Kalp", "Akciğer", "Böbrek", "Karaciğer"),
    q!("Bilim", "Düşen elma hikâyesiyle yer çekimi yasasıyla anılan bilim insanı kimdir?", "Isaac Newton", "Galileo Galilei", "Albert Einstein", "Johannes Kepler"),
    q!("Bilim", "Görelilik kuramını kim geliştirdi?", "Albert Einstein", "Isaac Newton", "Nikola Tesla", "Niels Bohr"),
    q!("Bilim", "Ses hangi ortamda yayılamaz?", "Boşlukta", "Suda", "Havada", "Demirde"),
    q!("Bilim", "Dünya'nın tek doğal uydusu hangisidir?", "Ay", "Phobos", "Titan", "Europa"),
    q!("Bilim", "Havada en çok bulunan gaz hangisidir?", "Azot", "Oksijen", "Karbondioksit", "Argon"),
    q!("Bilim", "\"Işık yılı\" neyin birimidir?", "Uzaklık", "Zaman", "Hız", "Parlaklık"),
    q!("Bilim", "Periyodik tabloda \"Fe\" sembolü hangi elementtir?", "Demir", "Flor", "Fosfor", "Fermiyum"),
    q!("Bilim", "Arılar çiçeklerden topladıkları nektarla ne üretir?", "Bal", "Polen", "Balmumu", "Arı sütü"),
    // ---- Matematik
    q!("Matematik", "7 × 8 kaçtır?", "56", "54", "48", "64"),
    q!("Matematik", "100'ün karekökü kaçtır?", "10", "20", "50", "25"),
    q!("Matematik", "Bir düzine kaç tanedir?", "12", "10", "6", "24"),
    q!("Matematik", "Pi sayısı yaklaşık olarak kaçtır?", "3,14", "3,41", "3,12", "3,16"),
    q!("Matematik", "Altıgenin kaç kenarı vardır?", "6", "5", "7", "8"),
    q!("Matematik", "2'nin 10. kuvveti kaçtır?", "1024", "1000", "512", "2048"),
    q!("Matematik", "Bir dik açı kaç derecedir?", "90", "45", "180", "60"),
    // ---- Spor
    q!("Spor", "Basketbolda bir takım sahada kaç oyuncuyla oynar?", "5", "6", "7", "4"),
    q!("Spor", "Olimpiyat bayrağında kaç halka vardır?", "5", "4", "6", "7"),
    q!("Spor", "Voleybolda bir takım sahada kaç oyuncuyla oynar?", "6", "5", "7", "8"),
    q!("Spor", "Toprak kortta oynanan Grand Slam tenis turnuvası hangisidir?", "Fransa Açık", "Wimbledon", "ABD Açık", "Avustralya Açık"),
    // ---- Oyun ve teknoloji
    q!("Oyun", "Pac-Man oyununu hangi şirket geliştirdi?", "Namco", "Nintendo", "Sega", "Atari"),
    q!("Oyun", "Super Mario'nun yaratıcısı kimdir?", "Shigeru Miyamoto", "Hideo Kojima", "Markus Persson", "Gabe Newell"),
    q!("Oyun", "Pikachu hangi serinin karakteridir?", "Pokémon", "Digimon", "Yu-Gi-Oh!", "The Legend of Zelda"),
    q!("Oyun", "Minecraft'ta yaklaşınca patlayan yeşil yaratık hangisidir?", "Creeper", "Enderman", "Zombi", "İskelet"),
    q!("Teknoloji", "Windows işletim sistemini hangi şirket geliştirir?", "Microsoft", "Apple", "Google", "IBM"),
    q!("Teknoloji", "\"WWW\" kısaltmasının açılımı nedir?", "World Wide Web", "World Web Wide", "Wide World Web", "Web World Wide"),
    q!("Teknoloji", "YouTube hangi yıl kuruldu?", "2005", "2003", "2007", "2010"),
    q!("Teknoloji", "iPhone'u hangi şirket üretir?", "Apple", "Samsung", "Google", "Huawei"),
    // ---- Sanat ve edebiyat
    q!("Sanat", "\"Yıldızlı Gece\" tablosunu kim yaptı?", "Vincent van Gogh", "Claude Monet", "Pablo Picasso", "Salvador Dalí"),
    q!("Edebiyat", "\"Çalıkuşu\" romanının yazarı kimdir?", "Reşat Nuri Güntekin", "Halide Edib Adıvar", "Yakup Kadri Karaosmanoğlu", "Ömer Seyfettin"),
    q!("Edebiyat", "\"İnce Memed\" romanının yazarı kimdir?", "Yaşar Kemal", "Orhan Kemal", "Sait Faik Abasıyanık", "Kemal Tahir"),
    q!("Edebiyat", "\"Kürk Mantolu Madonna\" kimin eseridir?", "Sabahattin Ali", "Oğuz Atay", "Ahmet Hamdi Tanpınar", "Peyami Safa"),
    q!("Edebiyat", "\"Tutunamayanlar\" romanının yazarı kimdir?", "Oğuz Atay", "Orhan Pamuk", "Yusuf Atılgan", "Bilge Karasu"),
    q!("Müzik", "\"Neşeye Övgü\" bölümüyle ünlü 9. Senfoni'nin bestecisi kimdir?", "Ludwig van Beethoven", "Wolfgang Amadeus Mozart", "Johann Sebastian Bach", "Frédéric Chopin"),
    q!("Edebiyat", "Nasreddin Hoca fıkralarında Hoca'nın bindiği hayvan hangisidir?", "Eşek", "At", "Deve", "Katır"),
    // ---- Genel
    q!("Genel", "Bir yılda kaç ay 31 gün çeker?", "7", "6", "5", "8"),
    q!("Genel", "Artık yıllarda Şubat ayı kaç gündür?", "29", "28", "30", "31"),
    q!("Genel", "Türk bayrağındaki ay yıldız hangi renktir?", "Beyaz", "Sarı", "Kırmızı", "Mavi"),
    q!("Genel", "Satrançta yalnızca çapraz hareket edebilen taş hangisidir?", "Fil", "Kale", "At", "Vezir"),
];

// ===================== Doğruluk mu cesaret mi =====================

pub const TRUTHS: &[&str] = &[
    "En son ne zaman ağladın ve neden?",
    "Hiç kimseye söylemediğin bir korkun var mı?",
    "Bu sunucudaki en komik kişi sence kim?",
    "En utandığın anı anlat.",
    "Telefonundaki en son aradığın şey neydi?",
    "Hiç bir sınavda kopya çektin mi?",
    "Bir günlüğüne görünmez olsan ne yapardın?",
    "En sevdiğin çizgi film karakteri kim?",
    "Gizlice dinlediğin ama itiraf etmediğin bir şarkı var mı?",
    "Hayatında verdiğin en kötü karar neydi?",
    "Bir süper gücün olsa hangisini seçerdin?",
    "En son kime yalan söyledin?",
    "Çocukken en büyük hayalin neydi?",
    "En garip yemek alışkanlığın ne?",
    "Hiç bir oyunda hile yaptın mı?",
    "Bu sunucuda en çok kiminle konuşuyorsun?",
    "Unutamadığın bir rüyayı anlat.",
    "Bir film karakteri olsan kim olurdun?",
    "En son ne zaman geç kalktın ve ne kaçırdın?",
    "Sana verilen en iyi tavsiye neydi?",
    "En son hangi diziyi/filmi bitirdin ve kaç puan verirsin?",
    "Hiç bir öğretmene ya da hocaya kızıp içinden sövdün mü?",
    "Bir gün boyunca başka biri olabilsen kim olurdun?",
    "Hiç gizlice birinin mesajlarını okudun mu?",
    "En son kimi kıskandın?",
    "Hayatında yaptığın en çılgınca şey neydi?",
    "Bugüne kadar aldığın en kötü hediye neydi?",
    "Hiç yemek yaparken bir şey yaktın mı? Ne oldu?",
    "En son ne zaman özür diledin?",
    "Telefonunda en çok vakit geçirdiğin uygulama hangisi?",
    "Hiç sevmediğin birine sırf nezaketen iltifat ettin mi?",
    "Kimsenin bilmediği bir yeteneğin var mı?",
    "En çok hangi konuda tembellik ediyorsun?",
    "Okulda/işte en çok utandığın an neydi?",
    "Bir yıl internetsiz kalsan en çok neyi özlerdin?",
    "Hiç bir sırrı tutamayıp başkasına anlattın mı?",
    "Sınıfta/işte en sevmediğin ders veya görev hangisi?",
    "Hayatında pişman olduğun bir alışveriş var mı?",
    "Hiç bir oyunda kaybedince sinirlenip oyunu kapattın mı?",
    "En garip lakabın neydi?",
    "En son ne zaman kahkaha atarak güldün? Neye?",
    "Hiç bir arkadaşının doğum gününü unuttun mu?",
    "Bir hayvan olsan hangisi olurdun ve neden?",
    "En son gördüğün rüyada ne vardı?",
    "Hiç birini taklit edip arkasından güldün mü?",
    "Sabah kalkınca ilk yaptığın şey ne?",
    "Seni en çok ne sinirlendirir?",
    "Hayatındaki en büyük başarın sence ne?",
    "Hiç bir şeyi bozup başkasının üstüne attın mı?",
    "Hangi şarkıyı dinleyince hemen modun düzelir?",
    "En çok korktuğun hayvan hangisi?",
    "Hiç bir film izlerken ağladın mı? Hangisi?",
    "Zaman makinen olsa geçmişe mi giderdin, geleceğe mi?",
    "Bu sunucuda ilk kiminle konuştun?",
    "Hiç bir randevuya/buluşmaya geç kaldın mı? Ne bahane uydurdun?",
    "Bir milyon liran olsa ilk neye harcardın?",
    "Hangi uygulamayı telefonundan silsen hayatın düzelir?",
    "En son ne zaman bir şeyi ertelemekten pişman oldun?",
    "Çocukluğundaki en sevdiğin oyuncak neydi?",
    "Hiç bir maçın/oyunun sonucuna iddiaya girdin mi?",
];

pub const DARES: &[&str] = &[
    "Sonraki 3 mesajını sadece büyük harfle yaz.",
    "Bu kanala en son çektiğin ekran görüntüsünü at (uygunsa!).",
    "Bir hayvan taklidi yapıp sesli kanala katıl.",
    "En sevdiğin şarkının ilk dizesini yaz.",
    "Sonraki 10 dakika boyunca her mesajının sonuna 🐸 koy.",
    "Kendini 3 emojiyle anlat.",
    "Sunucudaki birine içten bir iltifat et.",
    "Bir tekerleme yaz ve hiç hata yapmadan söylediğini iddia et.",
    "Sonraki mesajını tersten yaz.",
    "Takma adını 10 dakikalığına komik bir şeyle değiştir.",
    "Bir şiirin ilk iki dizesini kendin uydur.",
    "Bugün yediğin her şeyi sırayla yaz.",
    "En sevdiğin oyundan bir replik yaz.",
    "Sonraki 5 dakika sadece soru cümlesiyle konuş.",
    "Bir ünlüyü taklit ederek bir cümle yaz.",
    "Klavyene bakmadan 'Merhaba dünya' yaz.",
    "Bu kanala bir kedi/köpek fotoğrafı at.",
    "Bir hayalini bu kanalda paylaş.",
    "Sonraki mesajını sadece emojilerle yaz.",
    "Sunucunun yeni sloganını uydur.",
    "Sonraki 3 mesajında her kelimenin baş harfini büyük yaz.",
    "Bir arkadaşına sebepsiz yere 'Seni takdir ediyorum' yaz.",
    "En sevdiğin yemeğin tarifini 3 cümleyle anlat.",
    "Bir film sahnesini sadece emojilerle anlat; diğerleri tahmin etsin.",
    "Sonraki mesajını kafiyeli yaz.",
    "Kendine yeni bir süper kahraman adı ve gücü uydur.",
    "Bu kanaldaki son mesajı dramatik bir şekilde yeniden yaz.",
    "Bir hayvanın gözünden bugünü anlat.",
    "Bir kelimeyi 10 farklı emojiyle anlat.",
    "Sonraki 5 dakika boyunca her mesajına 'efendim' ile başla.",
    "En sevdiğin oyun karakterini 3 kelimeyle tarif et.",
    "Bir reklam sloganı uydur: ürün 'görünmez şemsiye'.",
    "Sunucudaki birinin profil fotoğrafını öv.",
    "Sesli kanalda 10 saniye boyunca bir şarkı mırıldan.",
    "Kendi hayatının film afişi için bir başlık uydur.",
    "Bir atasözünü tersine çevirip yaz.",
    "En komik gif'ini bu kanala at.",
    "Bir tekerlemeyi 3 kez hızlıca yaz (hatasız!).",
    "Sonraki mesajını sadece sesli harflerle yaz.",
    "Bir arkadaşının adıyla akrostiş yaz.",
    "Hayalindeki tatili 5 emojiyle anlat.",
    "Bir şarkı sözünü bu sunucuya uyarla.",
    "Haber spikeri gibi bugünkü havayı anlat.",
    "Sonraki 3 mesajını Shakespeare tarzında yaz.",
    "Bir yemeği hiç adını söylemeden tarif et, diğerleri bilsin.",
    "Bir hayvan sesi yazıyla taklit et, kimse hangisi olduğunu bilmesin.",
    "En sevdiğin çizgi filmin jenerik şarkısından bir dize yaz.",
    "Kendini bir ürün gibi pazarla: 'Şimdi alana…'",
    "Sunucudaki herkese iyi geceler veya günaydın dile.",
    "Bir dakika boyunca hiç 'e' harfi kullanmadan mesaj yaz.",
    "Bugünün mottosunu belirle ve kanala yaz.",
    "Bir arkadaşına komik bir lakap öner (kırıcı olmasın!).",
    "En son çektiğin fotoğrafı tarif et (atmana gerek yok).",
    "İki uydurma kelime icat edip anlamlarını yaz.",
    "Bir oyunun kurallarını hiç oynamamış birine 2 cümleyle anlat.",
    "Sonraki mesajını bir robot gibi yaz: 'BİP BOP…'",
    "Bir şiire 'Ve sonra' diye başlayıp 2 dize ekle.",
    "Kanala en sevdiğin meme'in adını yaz.",
    "Bir dahaki sefere sesli kanala girince ilk cümlen 'Ben geldim!' olsun.",
    "Bugün öğrendiğin ilginç bir bilgiyi kanala yaz.",
    "En sevdiğin şarkıyı öner ve neden sevdiğini tek cümleyle anlat.",
];

// ===================== Sihirli 8 top =====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Yes,
    Maybe,
    No,
}

const EIGHT_BALL_YES: &[&str] = &[
    "Kesinlikle evet.",
    "Hiç şüphesiz.",
    "Bence evet.",
    "Büyük ihtimalle.",
    "İşaretler evet diyor.",
    "Evet.",
    "Görünüşe göre iyi.",
    "Buna güvenebilirsin.",
    "Yıldızlar senden yana! ✨",
    "Hiç düşünme, evet.",
];
const EIGHT_BALL_MAYBE: &[&str] = &[
    "Şu an söyleyemem, tekrar sor.",
    "Biraz bekle ve tekrar sor.",
    "Şimdi söylemesem daha iyi.",
    "Tahmin edemiyorum.",
    "Odaklan ve tekrar sor.",
    "Belki… belki de değil.",
];
const EIGHT_BALL_NO: &[&str] = &[
    "Pek sanmıyorum.",
    "Cevabım hayır.",
    "Kaynaklarım hayır diyor.",
    "Pek iyi görünmüyor.",
    "Çok şüpheli.",
    "Asla.",
    "Hayal bile etme. 😄",
];

/// Klasik 8 topta olduğu gibi: yaklaşık yarısı olumlu, dörtte biri belirsiz, dörtte biri olumsuz.
pub fn eight_ball() -> (&'static str, Verdict) {
    let (list, verdict) = match rand_util::below(4) {
        0 | 1 => (EIGHT_BALL_YES, Verdict::Yes),
        2 => (EIGHT_BALL_MAYBE, Verdict::Maybe),
        _ => (EIGHT_BALL_NO, Verdict::No),
    };
    (pick(list).copied().unwrap_or("Tekrar sor."), verdict)
}

// ===================== Hayvan yakalama =====================

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rarity {
    Common,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Common => "Yaygın",
            Self::Rare => "Nadir",
            Self::Epic => "Epik",
            Self::Legendary => "Efsanevi",
        }
    }
    pub fn colour(self) -> (u8, u8, u8) {
        match self {
            Self::Common => (149, 165, 166),
            Self::Rare => (52, 152, 219),
            Self::Epic => (155, 89, 182),
            Self::Legendary => (241, 196, 15),
        }
    }
    /// Çıkma ağırlığı (toplam 100).
    fn weight(self) -> u64 {
        match self {
            Self::Common => 60,
            Self::Rare => 25,
            Self::Epic => 11,
            Self::Legendary => 4,
        }
    }
    pub fn reward(self) -> i64 {
        match self {
            Self::Common => 20,
            Self::Rare => 50,
            Self::Epic => 120,
            Self::Legendary => 400,
        }
    }
}

pub struct Animal {
    pub id: &'static str,
    pub name: &'static str,
    pub emoji: &'static str,
    pub rarity: Rarity,
}

macro_rules! animal {
    ($id:literal, $name:literal, $emoji:literal, $r:ident) => {
        Animal { id: $id, name: $name, emoji: $emoji, rarity: Rarity::$r }
    };
}

pub const ANIMALS: &[Animal] = &[
    animal!("kedi", "Kedi", "🐈", Common),
    animal!("kopek", "Köpek", "🐕", Common),
    animal!("tavsan", "Tavşan", "🐇", Common),
    animal!("serce", "Serçe", "🐦", Common),
    animal!("kurbaga", "Kurbağa", "🐸", Common),
    animal!("tavuk", "Tavuk", "🐔", Common),
    animal!("tilki", "Tilki", "🦊", Rare),
    animal!("baykus", "Baykuş", "🦉", Rare),
    animal!("kirpi", "Kirpi", "🦔", Rare),
    animal!("rakun", "Rakun", "🦝", Rare),
    animal!("kunduz", "Kunduz", "🦫", Rare),
    animal!("kurt", "Kurt", "🐺", Epic),
    animal!("kartal", "Kartal", "🦅", Epic),
    animal!("panda", "Panda", "🐼", Epic),
    animal!("penguen", "Penguen", "🐧", Epic),
    animal!("ejderha", "Ejderha", "🐉", Legendary),
    animal!("unicorn", "Tek Boynuzlu At", "🦄", Legendary),
    animal!("anka", "Anka Kuşu", "🐦‍🔥", Legendary),
];

/// Önce nadirlik (ağırlıklı), sonra o nadirlikteki bir hayvan seçilir.
pub fn random_animal() -> &'static Animal {
    let tiers = [Rarity::Common, Rarity::Rare, Rarity::Epic, Rarity::Legendary];
    let mut roll = rand_util::below(100);
    let mut rarity = Rarity::Common;
    for tier in tiers {
        if roll < tier.weight() {
            rarity = tier;
            break;
        }
        roll -= tier.weight();
    }
    let pool: Vec<&'static Animal> = ANIMALS.iter().filter(|a| a.rarity == rarity).collect();
    pick(&pool).copied().unwrap_or(&ANIMALS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_lowercase_letters() {
        for (_, words) in WORDS {
            for w in *words {
                assert!(w.chars().all(char::is_alphabetic), "{w}");
                assert_eq!(*w, w.to_lowercase(), "{w}");
            }
        }
    }

    #[test]
    fn questions_have_unique_options() {
        for q in QUESTIONS {
            let mut opts = q.options.to_vec();
            opts.sort_unstable();
            opts.dedup();
            assert_eq!(opts.len(), 4, "{}", q.text);
        }
    }

    #[test]
    fn rarity_weights_sum_to_100() {
        let total: u64 = [Rarity::Common, Rarity::Rare, Rarity::Epic, Rarity::Legendary].iter().map(|r| r.weight()).sum();
        assert_eq!(total, 100);
        for r in [Rarity::Common, Rarity::Rare, Rarity::Epic, Rarity::Legendary] {
            assert!(ANIMALS.iter().any(|a| a.rarity == r));
        }
    }

    #[test]
    fn animal_ids_are_unique() {
        let mut ids: Vec<&str> = ANIMALS.iter().map(|a| a.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ANIMALS.len());
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn fresh_picks_do_not_repeat_recent() {
        let mut recent = std::collections::VecDeque::new();
        let mut seen = Vec::new();
        for _ in 0..40 {
            let i = pick_fresh(QUESTIONS.len(), &mut recent, 40);
            assert!(!seen.iter().rev().take(39).any(|s| *s == i), "son 40 içinde tekrar etti");
            seen.push(i);
        }
    }

    #[test]
    fn content_sizes() {
        assert!(QUESTIONS.len() >= 100, "{} soru", QUESTIONS.len());
        assert!(TRUTHS.len() >= 60 && DARES.len() >= 60);
        let mut texts: Vec<&str> = QUESTIONS.iter().map(|q| q.text).collect();
        texts.sort_unstable();
        let before = texts.len();
        texts.dedup();
        assert_eq!(before, texts.len(), "aynı soru iki kez var");
    }
}
