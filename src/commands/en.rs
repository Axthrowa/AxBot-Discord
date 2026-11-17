//! İngilizce komut arayüzü: Discord'u İngilizce kullananlar komut, seçenek ve seçim adlarını İngilizce görür
//! (Discord'un yerelleştirme alanları; komutun asıl adı Türkçe kalır, ikisi de aynı komuttur).
//!
//! Çeviriler `en.tsv` dosyasında, `|` ile ayrılmış satırlar (İngilizce metinde `|` kullanma):
//! - `C | <türkçe yol> | <ad> | <açıklama>` — komut (sağ tık komutunda açıklama yok)
//! - `P | <türkçe yol> | <seçenek> | <ad> | <açıklama>` — seçenek
//! - `K | <türkçe yol> | <seçenek> | <türkçe seçim> | <seçim>` — seçim
//!
//! Eksik çeviri `every_node_is_translated` testinde yakalanır; iskelet: `cargo test print_translation_skeleton -- --ignored --nocapture`.

use std::collections::HashMap;

use crate::data::Data;
use crate::error::Error;

type Cmd = poise::Command<Data, Error>;

const LOCALES: [&str; 2] = ["en-US", "en-GB"];
const TSV: &str = include_str!("en.tsv");

#[derive(Default)]
struct Table {
    commands: HashMap<String, (String, String)>,
    params: HashMap<(String, String), (String, String)>,
    choices: HashMap<(String, String, String), String>,
}

fn table() -> Table {
    let mut table = Table::default();
    for line in TSV.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        match cols.as_slice() {
            ["C", path, name, rest @ ..] => {
                table.commands.insert((*path).to_owned(), ((*name).to_owned(), rest.first().copied().unwrap_or("").to_owned()));
            }
            ["P", path, param, name, desc] => {
                table.params.insert(((*path).to_owned(), (*param).to_owned()), ((*name).to_owned(), (*desc).to_owned()));
            }
            ["K", path, param, choice, name] => {
                table.choices.insert(((*path).to_owned(), (*param).to_owned(), (*choice).to_owned()), (*name).to_owned());
            }
            _ => tracing::warn!(line, "en.tsv: okunamayan satır"),
        }
    }
    table
}

fn localize(map: &mut HashMap<String, String>, value: &str) {
    if !value.is_empty() {
        for locale in LOCALES {
            map.insert(locale.to_owned(), value.to_owned());
        }
    }
}

fn walk(cmd: &mut Cmd, prefix: &str, table: &Table) {
    let path = format!("{prefix}{}", cmd.name);
    if let Some((name, desc)) = table.commands.get(&path) {
        localize(&mut cmd.name_localizations, name);
        localize(&mut cmd.description_localizations, desc);
    }
    for param in &mut cmd.parameters {
        let key = (path.clone(), param.name.clone());
        if let Some((name, desc)) = table.params.get(&key) {
            localize(&mut param.name_localizations, name);
            localize(&mut param.description_localizations, desc);
        }
        for choice in &mut param.choices {
            if let Some(name) = table.choices.get(&(path.clone(), param.name.clone(), choice.name.clone())) {
                localize(&mut choice.localizations, name);
            }
        }
    }
    let prefix = format!("{path} ");
    for sub in &mut cmd.subcommands {
        walk(sub, &prefix, table);
    }
}

/// Bütün komut ağacına İngilizce adları ve açıklamaları ekler.
pub fn apply(commands: &mut [Cmd]) {
    let table = table();
    for cmd in commands {
        walk(cmd, "", &table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(cmd: &Cmd, prefix: &str, out: &mut Vec<String>) {
        let path = format!("{prefix}{}", cmd.name);
        let desc = cmd.description.clone().unwrap_or_default();
        out.push(format!("C | {path} | {} | {desc}", cmd.name));
        // Sağ tık komutlarının seçeneği (mesaj / üye) Discord'da görünmez.
        let params = if cmd.context_menu_action.is_some() { &[][..] } else { &cmd.parameters[..] };
        for p in params {
            out.push(format!("P | {path} | {} | {} | {}", p.name, p.name, p.description.clone().unwrap_or_default()));
            for c in &p.choices {
                out.push(format!("K | {path} | {} | {} | {}", p.name, c.name, c.name));
            }
        }
        for sub in &cmd.subcommands {
            nodes(sub, &format!("{path} "), out);
        }
    }

    /// Çevrilecek bütün satırları Türkçe değerleriyle yazdırır (eksikleri bulmak için).
    #[test]
    #[ignore = "yalnızca listeleme"]
    fn print_translation_skeleton() {
        let table = table();
        let mut out = Vec::new();
        for cmd in super::super::every() {
            nodes(&cmd, "", &mut out);
        }
        for line in out {
            let cols: Vec<&str> = line.split('|').map(str::trim).collect();
            let done = match cols.as_slice() {
                ["C", path, ..] => table.commands.contains_key(*path),
                ["P", path, param, ..] => table.params.contains_key(&((*path).to_owned(), (*param).to_owned())),
                ["K", path, param, choice, ..] => table.choices.contains_key(&((*path).to_owned(), (*param).to_owned(), (*choice).to_owned())),
                _ => true,
            };
            if !done {
                println!("{line}");
            }
        }
    }

    /// Her komut, seçenek ve seçim çevrilmiş; adlar Discord kurallarına uyuyor.
    #[test]
    fn every_node_is_translated() -> Result<(), String> {
        let table = table();
        let mut commands = super::super::every();
        apply(&mut commands);
        fn check(cmd: &Cmd, path: &str) -> Result<(), String> {
            let path = format!("{path}{}", cmd.name);
            let slash = cmd.context_menu_action.is_none();
            let name = cmd.name_localizations.get("en-US").ok_or(format!("çevrilmemiş komut: {path}"))?;
            let valid = |n: &str| (1..=32).contains(&n.chars().count()) && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
            if slash && !valid(name) {
                return Err(format!("geçersiz ad: {path} → {name}"));
            }
            if slash {
                let desc = cmd.description_localizations.get("en-US").ok_or(format!("açıklamasız: {path}"))?;
                if desc.is_empty() || desc.chars().count() > 100 {
                    return Err(format!("açıklama 1-100 karakter olmalı: {path}"));
                }
            }
            for p in cmd.parameters.iter().filter(|_| slash) {
                let n = p.name_localizations.get("en-US").ok_or(format!("çevrilmemiş seçenek: {path} {}", p.name))?;
                if !valid(n) {
                    return Err(format!("geçersiz seçenek adı: {path} {n}"));
                }
                let d = p.description_localizations.get("en-US").ok_or(format!("açıklamasız seçenek: {path} {}", p.name))?;
                if d.is_empty() || d.chars().count() > 100 {
                    return Err(format!("seçenek açıklaması 1-100 karakter olmalı: {path} {}", p.name));
                }
                if let Some(c) = p.choices.iter().find(|c| !c.localizations.contains_key("en-US")) {
                    return Err(format!("çevrilmemiş seçim: {path} {} {}", p.name, c.name));
                }
            }
            // Kardeş komutların İngilizce adları çakışmasın.
            let mut seen = std::collections::HashSet::new();
            for sub in &cmd.subcommands {
                if let Some(n) = sub.name_localizations.get("en-US") {
                    if !seen.insert(n.clone()) {
                        return Err(format!("{path} altında iki komut aynı İngilizce adı taşıyor: {n}"));
                    }
                }
                check(sub, &format!("{path} "))?;
            }
            Ok(())
        }
        let mut top = std::collections::HashSet::new();
        for cmd in &commands {
            if let (Some(n), true) = (cmd.name_localizations.get("en-US"), cmd.context_menu_action.is_none()) {
                if !top.insert(n.clone()) {
                    return Err(format!("iki üst komut aynı İngilizce adı taşıyor: {n}"));
                }
            }
            check(cmd, "")?;
        }
        // Artık satır (silinmiş komutun çevirisi) kalmasın.
        let mut known = Vec::new();
        for cmd in super::super::every() {
            nodes(&cmd, "", &mut known);
        }
        let paths: std::collections::HashSet<String> = known.iter().filter_map(|l| l.strip_prefix("C | ")).filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect();
        if let Some(extra) = table.commands.keys().find(|k| !paths.contains(*k)) {
            return Err(format!("en.tsv'de artık komut: {extra}"));
        }
        Ok(())
    }
}
