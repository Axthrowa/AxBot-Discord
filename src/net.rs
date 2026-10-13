//! Ortak HTTP istemcisi (Discord REST, yapay zekâ ve yayın API'leri).
//!
//! İşletim sisteminin `getaddrinfo` çağrısı A ve AAAA kayıtlarını birlikte sorgular.
//! IPv6 sorgularına cevap vermeyen DNS sunucularında (bazı modemler/ISS'ler) bu, her yeni
//! bağlantıda ~12 sn beklemeye yol açar; Discord ise slash komutlara 3 sn içinde cevap ister.
//! Bu yüzden reqwest'e kendi çözücümüzü veriyoruz:
//! - Önce A (IPv4) kaydı sorgulanır, AAAA sadece IPv4 adresi yoksa denenir.
//! - Sonuçlar TTL süresince süreç içinde önbelleklenir; tekrar eden bağlantılar DNS'e hiç gitmez.
//!
//! Tek bir `reqwest::Client` tüm servislerde paylaşılır (bağlantı havuzu + DNS önbelleği ortak).
//! Servise özel zaman aşımları istek başına verilir.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use hickory_resolver::config::{LookupIpStrategy, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::{system_conf, TokioResolver};
use poise::serenity_prelude as serenity;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

struct Ipv4FirstResolver(TokioResolver);

impl Resolve for Ipv4FirstResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.0.clone();
        Box::pin(async move {
            let lookup = resolver.lookup_ip(name.as_str()).await?;
            // Port'u reqwest URL'den kendisi ekler.
            let addrs: Vec<SocketAddr> = lookup.iter().map(|ip| SocketAddr::new(ip, 0)).collect();
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Sistem DNS listesinden sadece IPv4 sunucularını alır.
///
/// Windows listesinde çoğu zaman cevap vermeyen IPv6 sunucuları da olur (`fe80::1`,
/// `fec0:0:0:ffff::1` gibi); çözücü bunlardan birini seçince sorgu 5 sn takılıyordu.
/// IPv4 sunucusu yoksa Cloudflare kullanılır.
fn resolver() -> anyhow::Result<TokioResolver> {
    let (system, mut options) = system_conf::read_system_conf().context("sistem DNS ayarları okunamadı")?;
    let mut config = ResolverConfig::from_parts(system.domain().cloned(), system.search().to_vec(), vec![]);
    for server in system.name_servers().iter().filter(|s| s.ip.is_ipv4()) {
        config.add_name_server(server.clone());
    }
    if config.name_servers().is_empty() {
        config = ResolverConfig::udp_and_tcp(&hickory_resolver::config::CLOUDFLARE);
    }
    options.ip_strategy = LookupIpStrategy::Ipv4thenIpv6;
    options.timeout = Duration::from_secs(2);

    let mut builder = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
    *builder.options_mut() = options;
    builder.build().context("DNS çözücü kurulamadı")
}

/// Tüm süreçte tek çözücü: önbelleği HTTP istemcisiyle ortaktır.
static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();

fn shared_resolver() -> anyhow::Result<&'static TokioResolver> {
    if let Some(r) = RESOLVER.get() {
        return Ok(r);
    }
    let built = resolver()?;
    Ok(RESOLVER.get_or_init(|| built))
}

/// Bir alan adının IP adreslerini hızlı çözücüyle bulur (işletim sisteminin yavaş
/// IPv6 sorgularına takılmaz).
pub async fn lookup_ips(host: &str) -> anyhow::Result<Vec<IpAddr>> {
    Ok(shared_resolver()?.lookup_ip(host).await?.iter().collect())
}

pub fn http_client() -> anyhow::Result<reqwest::Client> {
    builder()?.build().context("HTTP istemcisi oluşturulamadı")
}

/// Kullanıcının verdiği adreslerden dosya indirmek için: yönlendirmeleri izlemez. Adres önceden
/// "herkese açık mı" diye denetlenir; yönlendirme izlenseydi açık bir adres botu iç ağdaki bir
/// servise yönlendirebilirdi.
pub fn fetch_client() -> anyhow::Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let built = builder()?
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("HTTP istemcisi oluşturulamadı")?;
    Ok(CLIENT.get_or_init(|| built))
}

fn builder() -> anyhow::Result<reqwest::ClientBuilder> {
    Ok(reqwest::Client::builder()
        .use_rustls_tls()
        .user_agent(concat!("discord-bot/", env!("CARGO_PKG_VERSION")))
        .dns_resolver(Arc::new(Ipv4FirstResolver(shared_resolver()?.clone())))
        // Bazı CDN adresleri bu ağda ara sıra hiç cevap vermiyor; süre adreslere bölünür,
        // böylece takılan adresten birkaç saniyede bir sonrakine geçilir.
        .connect_timeout(Duration::from_secs(10))
        // Hız: Discord HTTP/2 konuşur; tek bağlantıda istekler paralel gider. Boşta kalan
        // bağlantı ping'lerle sıcak tutulur; böylece komut cevabı her seferinde yeni
        // DNS + TLS el sıkışmasını (~100-400 ms) beklemez.
        .http2_keep_alive_interval(Duration::from_secs(20))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .http2_keep_alive_while_idle(true)
        .tcp_keepalive(Duration::from_secs(30))
        .tcp_nodelay(true)
        .pool_idle_timeout(Duration::from_secs(300)))
}

/// Tarayıcı gibi görünen, yalnızca HTTP/1.1 konuşan istemci: Google'ın resmî olmayan uçları (çeviri, seslendirme)
/// botun HTTP/2 isteklerini otomatik trafik sayıp "429" ile reddediyor.
pub fn plain_client() -> anyhow::Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let built = reqwest::Client::builder()
        .use_rustls_tls()
        .http1_only()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36")
        .dns_resolver(Arc::new(Ipv4FirstResolver(shared_resolver()?.clone())))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("HTTP istemcisi oluşturulamadı")?;
    Ok(CLIENT.get_or_init(|| built))
}

/// Discord REST istemcisi: genel istemciyle aynı ayarlar + okuma süresi sınırı. Yarı kopmuş bir
/// bağlantıda bir komut cevabı sonsuza kadar beklemez (müzik akışları bu istemciyi kullanmaz).
pub fn discord_client() -> anyhow::Result<reqwest::Client> {
    builder()?.read_timeout(Duration::from_secs(30)).build().context("Discord HTTP istemcisi oluşturulamadı")
}

pub fn discord_http(token: &str, client: reqwest::Client) -> serenity::Http {
    serenity::HttpBuilder::new(token).client(client).build()
}

#[cfg(test)]
mod live_tests {
    use std::time::Instant;

    /// Gerçek DNS: her ad yeni bir çözücüyle (boş önbellek) 1 sn içinde çözülmeli.
    #[tokio::test]
    #[ignore = "gerçek DNS sorgusu yapar"]
    async fn live_dns_is_fast() -> anyhow::Result<()> {
        let hosts = ["discord.com", "gateway.discord.gg", "cdn.discordapp.com", "integrate.api.nvidia.com"];
        let mut slowest = 0u128;
        for round in 0..5 {
            for host in hosts {
                let resolver = super::resolver()?;
                let started = Instant::now();
                let found = resolver.lookup_ip(host).await.map(|l| l.iter().count());
                let ms = started.elapsed().as_millis();
                println!("tur {round} {host}: {ms} ms {found:?}");
                slowest = slowest.max(ms);
            }
        }
        println!("en yavaş: {slowest} ms");
        anyhow::ensure!(slowest < 1000, "DNS çok yavaş: {slowest} ms");
        Ok(())
    }
}
