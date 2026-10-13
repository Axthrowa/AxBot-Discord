// `sqlx::migrate!` migration'ları derleme anında gömer; klasör değişince yeniden derlensin.
// Web panelinde gösterilen derleme zamanı da buradan gelir (kaynak kod değişince güncellenir).
fn main() {
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=src");
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=AXBOT_BUILD_UNIX={unix}");
}
