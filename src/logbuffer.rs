//! Web panelinde gösterilen son loglar: `tracing` olaylarının bellekteki halkalı kopyası.
//!
//! Dosya/konsol loglarıyla aynı filtreden geçer (RUST_LOG). Kilit sadece bir satır eklenirken
//! çok kısa tutulur; async thread'ler beklemez.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Bellekte tutulan en fazla satır.
const CAPACITY: usize = 1000;

#[derive(Clone, Serialize)]
pub struct LogLine {
    /// Artan sıra numarası (panel "şundan sonrakiler" diye ister).
    pub id: u64,
    /// UTC, `2026-10-09 10:46:55`.
    pub time: String,
    pub level: &'static str,
    pub target: String,
    pub text: String,
}

#[derive(Default)]
struct Buffer {
    next_id: u64,
    lines: VecDeque<LogLine>,
}

static BUFFER: OnceLock<Mutex<Buffer>> = OnceLock::new();

fn buffer() -> &'static Mutex<Buffer> {
    BUFFER.get_or_init(|| Mutex::new(Buffer::default()))
}

/// `after` numarasından sonraki satırlar (en fazla `limit`, en yenileri). `min_warn`: sadece uyarı ve hatalar.
pub fn recent(after: u64, limit: usize, min_warn: bool) -> Vec<LogLine> {
    let Ok(buf) = buffer().lock() else {
        return Vec::new();
    };
    let mut lines: Vec<LogLine> = buf
        .lines
        .iter()
        .rev()
        .filter(|l| l.id > after && (!min_warn || matches!(l.level, "WARN" | "ERROR")))
        .take(limit)
        .cloned()
        .collect();
    lines.reverse();
    lines
}

/// Bot açıldığından beri (bellekte kalanlar arasında) kaç uyarı ve hata var.
pub fn counts() -> (usize, usize) {
    let Ok(buf) = buffer().lock() else {
        return (0, 0);
    };
    let warns = buf.lines.iter().filter(|l| l.level == "WARN").count();
    let errors = buf.lines.iter().filter(|l| l.level == "ERROR").count();
    (warns, errors)
}

pub struct PanelLayer;

impl<S: Subscriber> Layer<S> for PanelLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = Collect::default();
        event.record(&mut visitor);
        let level = match *meta.level() {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN",
            Level::INFO => "INFO",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };
        let mut text = visitor.message;
        if !visitor.fields.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&visitor.fields);
        }
        if *meta.level() == Level::ERROR {
            crate::alerts::notify(meta.target(), &text);
        }
        let time = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let Ok(mut buf) = buffer().lock() else {
            return;
        };
        buf.next_id += 1;
        let id = buf.next_id;
        if buf.lines.len() >= CAPACITY {
            buf.lines.pop_front();
        }
        buf.lines.push_back(LogLine { id, time, level, target: meta.target().to_owned(), text });
    }
}

#[derive(Default)]
struct Collect {
    message: String,
    fields: String,
}

impl Visit for Collect {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.push(field, &value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            self.push(field, value);
        }
    }
}

impl Collect {
    fn push(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        let _ = write!(self.fields, "{}={value:?}", field.name());
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt as _;

    #[test]
    fn keeps_recent_lines() {
        let subscriber = tracing_subscriber::registry().with(super::PanelLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(guild = 5, "deneme satırı");
            tracing::warn!("uyarı satırı");
        });
        let all = super::recent(0, 10, false);
        assert!(all.iter().any(|l| l.text.contains("deneme satırı") && l.text.contains("guild=5")));
        let warns = super::recent(0, 10, true);
        assert!(warns.iter().all(|l| l.level == "WARN" || l.level == "ERROR"));
        assert!(warns.iter().any(|l| l.text == "uyarı satırı"));
    }
}
