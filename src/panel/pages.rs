//! Panel sayfaları: genel bakış, sunucu, loglar, sistem. Sunucu tarafında üretilen sade HTML;
//! tek JavaScript dosyası canlı loglar için (`/panel/panel.js`). Tüm dinamik metinler kaçışlanır.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::{Form, Json};
use poise::serenity_prelude::{GuildId, UserId};
use serde::Deserialize;
use tracing::{info, warn};

use super::auth::{self, CsrfForm, Session};
use super::can_manage;
use crate::logbuffer;
use crate::setup_report::{self, State as FeatureState, Target};
use crate::web::{escape, WebState};

pub struct Page<'a> {
    pub title: &'a str,
    pub body: String,
}

/// Sunucu sayfalarında yan menüde eklenti listesi de gösterilir.
pub struct GuildNav<'a> {
    pub id: GuildId,
    pub name: &'a str,
    /// Açık olan eklentinin adresi; sunucu ana sayfasında boş.
    pub active: &'a str,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Nav {
    Home,
    Logs,
    System,
}

const CSS: &str = r#"
:root {
  color-scheme: dark;
  --bg: #0b0c11;
  --panel: #13151f;
  --panel-hover: #181b28;
  --panel-elevated: #1a1d2c;
  --line: rgba(255, 255, 255, 0.08);
  --line-hover: rgba(88, 101, 242, 0.45);
  --text: #f0f1f5;
  --muted: #8b90a4;
  --accent: #5865f2;
  --accent-hover: #4752c4;
  --accent-soft: rgba(88, 101, 242, 0.12);
  --accent-glow: rgba(88, 101, 242, 0.25);
  --ok: #22c55e;
  --warn: #f59e0b;
  --err: #ef4444;
  --font: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", sans-serif;
  --mono: "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--bg); color: var(--text);
  font-family: var(--font); -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale;
  line-height: 1.5; font-size: 14px;
}
a { color: #858df7; text-decoration: none; transition: color .15s ease; }
a:hover { color: #a4adff; text-decoration: underline; }
.shell { display: grid; grid-template-columns: 236px 1fr; min-height: 100vh; }
aside {
  background: #0f1017; border-right: 1px solid var(--line); padding: 14px 10px;
  display: flex; flex-direction: column; gap: 2px;
  position: sticky; top: 0; height: 100vh; overflow-y: auto;
  scrollbar-width: thin; scrollbar-color: rgba(255, 255, 255, 0.08) transparent;
}
.brand {
  font-weight: 800; letter-spacing: .06em; font-size: 1.05rem; margin: 4px 6px 12px;
  color: #fff; display: flex; align-items: center; gap: 8px;
}
.brand span {
  background: linear-gradient(135deg, #5865f2 0%, #858df7 100%);
  -webkit-background-clip: text; -webkit-text-fill-color: transparent;
}
.nav {
  display: flex; align-items: center; gap: 8px; padding: 7px 10px; border-radius: 8px;
  color: #adb0be; font-size: 0.85rem; font-weight: 500; transition: all .12s ease;
}
.nav:hover { background: rgba(255, 255, 255, 0.05); color: #fff; text-decoration: none; }
.nav.on { background: var(--accent); color: #fff; font-weight: 600; box-shadow: 0 2px 10px rgba(88, 101, 242, 0.35); }
.navh {
  margin: 12px 6px 4px; font-weight: 700; font-size: 0.8rem; color: #fff;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  padding-bottom: 4px; border-bottom: 1px solid var(--line);
}
.navg {
  margin: 10px 8px 2px; color: var(--muted); font-size: 0.68rem; font-weight: 700;
  letter-spacing: .08em; text-transform: uppercase;
}
.nav.sm { padding: 5px 9px; font-size: 0.82rem; border-radius: 6px; }
.me {
  margin-top: auto; display: flex; align-items: center; gap: 9px;
  padding: 10px 6px 6px; border-top: 1px solid var(--line);
}
.me img { width: 32px; height: 32px; border-radius: 50%; border: 1px solid var(--line); }
.me div { flex: 1; min-width: 0; font-size: 0.82rem; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.me small { color: var(--muted); display: block; font-size: 0.72rem; font-weight: 400; }
main {
  padding: 22px clamp(16px, 3.5vw, 36px); max-width: 1240px; width: 100%;
  animation: fadeIn .15s ease-out;
}
@keyframes fadeIn {
  from { opacity: 0; transform: translateY(3px); }
  to { opacity: 1; transform: translateY(0); }
}
h1 { margin: 0 0 4px; font-size: 1.45rem; font-weight: 700; letter-spacing: -0.02em; color: #fff; }
h2 { margin: 24px 0 10px; font-size: 1.05rem; font-weight: 600; letter-spacing: -0.01em; color: #fff; }
.sub { color: var(--muted); margin: 0 0 18px; font-size: 0.84rem; }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(160px, 1fr)); gap: 8px; }
.card {
  background: var(--panel); border: 1px solid var(--line); border-radius: 10px;
  padding: 10px 14px; transition: border-color .15s ease, background .15s ease;
}
.card:hover { border-color: var(--line-hover); background: var(--panel-hover); }
.card .k { color: var(--muted); font-size: 0.7rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; }
.card .v { font-size: 1.15rem; font-weight: 700; margin-top: 3px; font-variant-numeric: tabular-nums; }
.guilds { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 10px; }
.guild {
  display: flex; gap: 12px; align-items: center; background: var(--panel);
  border: 1px solid var(--line); border-radius: 12px; padding: 10px 12px;
  color: var(--text); transition: all .15s ease;
}
.guild:hover {
  border-color: var(--accent); background: var(--panel-hover);
  text-decoration: none; transform: translateY(-1px); box-shadow: 0 4px 16px rgba(0,0,0,0.3);
}
.ghead {
  display: flex; gap: 14px; align-items: center; margin: 4px 0 18px;
  padding: 14px 16px; background: var(--panel); border: 1px solid var(--line);
  border-radius: 12px;
}
.guild img, .ghead img, .gicon {
  width: 44px; height: 44px; border-radius: 10px; background: #232634;
  display: grid; place-items: center; font-weight: 700; font-size: 0.95rem; flex: none;
}
.badge { display: inline-flex; align-items: center; padding: 2px 7px; border-radius: 6px; font-size: 0.7rem; font-weight: 700; letter-spacing: 0.03em; }
.b-ok { background: rgba(34, 197, 94, 0.15); color: #4ade80; border: 1px solid rgba(34, 197, 94, 0.3); }
.b-warn { background: rgba(245, 158, 11, 0.15); color: #fde047; border: 1px solid rgba(245, 158, 11, 0.3); }
.b-err { background: rgba(239, 68, 68, 0.15); color: #f87171; border: 1px solid rgba(239, 68, 68, 0.3); }
.b-off { background: rgba(255, 255, 255, 0.06); color: var(--muted); border: 1px solid rgba(255, 255, 255, 0.08); }
table {
  width: 100%; border-collapse: separate; border-spacing: 0; background: var(--panel);
  border: 1px solid var(--line); border-radius: 10px; overflow: hidden;
}
td, th { text-align: left; padding: 9px 13px; border-bottom: 1px solid var(--line); vertical-align: middle; font-size: 0.86rem; }
th {
  background: rgba(255, 255, 255, 0.02); color: var(--muted); font-weight: 600;
  font-size: 0.73rem; text-transform: uppercase; letter-spacing: 0.05em;
}
tr:last-child td { border-bottom: 0; }
tr:hover td { background: rgba(255, 255, 255, 0.015); }
code {
  background: rgba(255, 255, 255, 0.07); padding: 2px 6px; border-radius: 5px;
  font-family: var(--mono); font-size: 0.82em; border: 1px solid rgba(255, 255, 255, 0.06);
}
.box {
  background: var(--panel); border: 1px solid var(--line); border-radius: 10px;
  padding: 13px 15px; line-height: 1.55; font-size: 0.88rem;
}
.box.warn { border-color: rgba(245, 158, 11, 0.35); background: rgba(245, 158, 11, 0.05); }
.box.err { border-color: rgba(239, 68, 68, 0.35); background: rgba(239, 68, 68, 0.05); }
button, .btn {
  background: linear-gradient(180deg, #5865f2 0%, #4752c4 100%);
  color: #fff; border: 0; border-radius: 8px; padding: 8px 15px;
  font: inherit; font-size: 0.84rem; font-weight: 600; cursor: pointer;
  display: inline-flex; align-items: center; justify-content: center; gap: 6px;
  box-shadow: 0 1px 2px rgba(0,0,0,0.2), inset 0 1px 0 rgba(255,255,255,0.15);
  transition: all .15s ease; text-decoration: none;
}
button:hover, .btn:hover {
  filter: brightness(1.08); transform: translateY(-1px);
  box-shadow: 0 3px 12px rgba(88, 101, 242, 0.35); text-decoration: none;
}
button:active, .btn:active { transform: translateY(0); filter: brightness(0.96); }
button.danger, .btn.danger {
  background: linear-gradient(180deg, #ef4444 0%, #dc2626 100%);
  box-shadow: 0 1px 2px rgba(0,0,0,0.2), inset 0 1px 0 rgba(255,255,255,0.15);
}
button.danger:hover, .btn.danger:hover { box-shadow: 0 3px 12px rgba(239, 68, 68, 0.35); }
button.ghost, .btn.ghost {
  background: rgba(255, 255, 255, 0.05); color: var(--text);
  border: 1px solid var(--line); box-shadow: none;
}
button.ghost:hover, .btn.ghost:hover {
  background: rgba(255, 255, 255, 0.09); border-color: rgba(255, 255, 255, 0.14);
  box-shadow: none; filter: none;
}
#log {
  background: #08090d; border: 1px solid var(--line); border-radius: 10px;
  padding: 12px 14px; height: 65vh; overflow: auto;
  font: 12px/1.55 var(--mono); white-space: pre-wrap; word-break: break-word; margin: 0;
  scrollbar-width: thin;
}
.l-WARN { color: #fde047; } .l-ERROR { color: #f87171; } .l-DEBUG, .l-TRACE { color: #6b7082; } .t { color: #6b7082; }
.pcards { display: grid; grid-template-columns: repeat(auto-fill, minmax(235px, 1fr)); gap: 9px; }
.pcard {
  display: flex; flex-direction: column; gap: 7px; background: var(--panel);
  border: 1px solid var(--line); border-radius: 11px; padding: 13px 14px;
  color: var(--text); transition: all .15s ease;
}
.pcard:hover {
  border-color: rgba(88, 101, 242, 0.45); text-decoration: none;
  transform: translateY(-2px); box-shadow: 0 4px 16px rgba(0,0,0,0.3);
}
.pcard.on {
  border-color: rgba(34, 197, 94, 0.3);
  background: linear-gradient(180deg, rgba(34, 197, 94, 0.02) 0%, var(--panel) 100%);
}
.pcard.on:hover { border-color: rgba(34, 197, 94, 0.5); }
.pico {
  width: 36px; height: 36px; border-radius: 9px; background: rgba(88, 101, 242, 0.12);
  display: grid; place-items: center; font-size: 1.2rem; flex: none;
}
.pcard.on .pico { background: rgba(34, 197, 94, 0.14); }
.ptitle { font-weight: 600; font-size: 0.9rem; display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
.plink { margin-top: auto; color: #858df7; font-weight: 600; font-size: 0.8rem; }
.phead { display: flex; justify-content: space-between; align-items: flex-start; gap: 14px; }
.sect {
  background: var(--panel); border: 1px solid var(--line); border-radius: 11px;
  padding: 16px; margin-bottom: 12px;
}
.sect h3 { margin: 0 0 10px; font-size: 0.95rem; font-weight: 600; }
.sect h4 { margin: 12px 0 6px; font-size: 0.74rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.06em; font-weight: 600; }
.sect h4:first-child { margin-top: 0; }
input[type=color] { width: 56px; height: 34px; padding: 2px; background: #0f1017; border: 1px solid var(--line); border-radius: 8px; cursor: pointer; }
input[type=text].wide { max-width: 560px; }
.preview { display: block; width: 100%; max-width: 560px; border-radius: 10px; border: 1px solid var(--line); margin-top: 6px; min-height: 56px; background: #0f1017; }
input[type=text] {
  width: 100%; max-width: 260px; background: #0f1017; color: var(--text);
  border: 1px solid var(--line); border-radius: 8px; padding: 7px 11px; font: inherit; font-size: 0.86rem;
  transition: border-color .15s ease, box-shadow .15s ease;
}
.sect td .note { margin: 2px 0 0; } .sect td select { max-width: 240px; }
.field { display: block; margin-bottom: 12px; } .field:last-child { margin-bottom: 0; }
.field > span { display: block; font-weight: 600; font-size: 0.84rem; margin-bottom: 4px; color: #d8dae5; }
.field small { display: block; color: var(--muted); margin-top: 4px; font-size: 0.77rem; line-height: 1.45; }
select, textarea, input[type=number] {
  width: 100%; max-width: 560px; background: #0f1017; color: var(--text); border: 1px solid var(--line);
  border-radius: 8px; padding: 7px 11px; font: inherit; font-size: 0.86rem;
  transition: border-color .15s ease, box-shadow .15s ease;
}
textarea { max-width: 100%; resize: vertical; } input[type=number] { max-width: 140px; }
select:focus, textarea:focus, input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-glow); outline: none; }
.savebar {
  display: flex; gap: 8px; align-items: center; margin-top: 10px; padding: 10px 14px;
  background: rgba(16, 17, 23, 0.85); border: 1px solid var(--line); border-radius: 10px;
  position: sticky; bottom: 12px; backdrop-filter: blur(10px); z-index: 10;
}
.check { display: flex; align-items: center; gap: 8px; padding: 4px 0; cursor: pointer; font-size: 0.86rem; user-select: none; }
.check input { width: 16px; height: 16px; accent-color: var(--accent); flex: none; }
.checks {
  display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 3px 12px;
  max-height: 240px; overflow-y: auto; background: #0f1017; border: 1px solid var(--line);
  border-radius: 8px; padding: 7px 11px; scrollbar-width: thin;
}
.checks.col { grid-template-columns: 1fr; max-height: none; background: none; border: 0; padding: 0; margin-bottom: 8px; }
.sect table { margin-bottom: 4px; } .sect .note { margin: 6px 0 0; }
.board { display: flex; flex-direction: column; gap: 6px; }
.brow {
  display: flex; align-items: center; gap: 12px; background: var(--panel);
  border: 1px solid var(--line); border-radius: 10px; padding: 10px 14px; transition: transform .15s ease;
}
.brow:hover { border-color: var(--line-hover); transform: translateX(2px); }
.brank { width: 36px; text-align: center; font-weight: 800; font-size: 0.95rem; color: var(--muted); flex: none; }
.bava { width: 38px; height: 38px; border-radius: 50%; background: #232634; flex: none; border: 1px solid var(--line); }
.bmain { flex: 1; min-width: 0; } .bmain b { display: block; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-size: 0.88rem; }
.bmain small, .blevel small, .bxp small { color: var(--muted); font-size: 0.72rem; }
.bbar { height: 6px; border-radius: 99px; background: rgba(255,255,255,0.06); margin: 5px 0 3px; overflow: hidden; }
.bbar span { display: block; height: 100%; background: linear-gradient(90deg, #5865f2, #7983f5); border-radius: 99px; }
.blevel, .bxp { text-align: center; min-width: 65px; } .blevel b, .bxp b { display: block; font-size: 1rem; }
@media (max-width:560px) { .bxp { display: none; } }
.heat {
  display: grid; grid-template-columns: 36px repeat(24, 1fr); gap: 2px;
  background: var(--panel); border: 1px solid var(--line); border-radius: 10px; padding: 11px; align-items: center;
}
.heat small { color: var(--muted); font-size: 0.62rem; text-align: center; font-variant-numeric: tabular-nums; }
.heat i { display: block; aspect-ratio: 1; border-radius: 3px; background: var(--accent); min-height: 8px; transition: transform .1s ease; }
.heat i:hover { transform: scale(1.3); z-index: 2; }
.tpls { display: flex; flex-wrap: wrap; gap: 6px; } .tpls .btn { padding: 6px 12px; font-weight: 600; font-size: 0.8rem; }
.ptop { display: flex; justify-content: space-between; align-items: center; }
.sw {
  width: 32px; height: 18px; border-radius: 99px; background: #2a2c38;
  position: relative; flex: none; transition: background .18s ease; cursor: pointer;
}
.sw::after {
  content: ""; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px;
  border-radius: 50%; background: #9295a5; transition: transform .18s cubic-bezier(0.2,0,0,1), background .18s ease;
}
.sw.on { background: var(--ok); } .sw.on::after { transform: translateX(14px); background: #fff; }
.psearch {
  width: 100%; max-width: 360px !important; margin: 0 0 12px; background: var(--panel);
  color: var(--text); border: 1px solid var(--line); border-radius: 9px; padding: 8px 12px;
  font: inherit; font-size: 0.85rem; transition: all .15s ease;
}
.psearch:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-glow); outline: none; }
h2.pgroup {
  margin: 18px 0 8px; font-size: 0.82rem; font-weight: 700; text-transform: uppercase;
  letter-spacing: 0.06em; color: var(--muted); display: flex; align-items: center; gap: 8px;
}
h2.pgroup::after { content: ""; flex: 1; height: 1px; background: var(--line); }
.check input[type=checkbox] {
  appearance: none; -webkit-appearance: none; width: 32px !important; height: 18px !important;
  border-radius: 99px; background: #2a2c38; position: relative; cursor: pointer;
  transition: background .18s ease; flex: none; margin: 0;
}
.check input[type=checkbox]::after {
  content: ""; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px;
  border-radius: 50%; background: #9295a5; transition: transform .18s cubic-bezier(0.2,0,0,1), background .18s ease;
}
.check input[type=checkbox]:checked { background: var(--accent); }
.check input[type=checkbox]:checked::after { transform: translateX(14px); background: #fff; }
.checks .check input[type=checkbox] {
  width: 16px !important; height: 16px !important; border-radius: 4px;
  background: #181922; border: 1px solid var(--line);
}
.checks .check input[type=checkbox]::after { display: none; }
.checks .check input[type=checkbox]:checked {
  background: var(--accent); border-color: var(--accent);
  background-image: url("data:image/svg+xml,%3Csvg viewBox='0 0 16 16' fill='white' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M12.207 4.793a1 1 0 010 1.414l-5 5a1 1 0 01-1.414 0l-2-2a1 1 0 011.414-1.414L6.5 9.086l4.293-4.293a1 1 0 011.414 0z'/%3E%3C/svg%3E");
  background-repeat: no-repeat; background-position: center; background-size: 11px;
}
.chart {
  display: flex; align-items: flex-end; gap: 4px; height: 140px; background: var(--panel);
  border: 1px solid var(--line); border-radius: 10px; padding: 10px 10px 6px;
}
.chart .col { flex: 1; display: flex; flex-direction: column; justify-content: flex-end; align-items: center; gap: 3px; height: 100%; min-width: 0; }
.chart .bar { width: 100%; max-width: 32px; background: linear-gradient(180deg, #5865f2 0%, #3d47ad 100%); border-radius: 4px 4px 1px 1px; min-height: 2px; }
.chart .col:hover .bar { filter: brightness(1.2); }
.chart small { color: var(--muted); font-size: 0.65rem; white-space: nowrap; } .chart b { font-size: 0.7rem; font-weight: 600; }
.flash {
  border-radius: 9px; padding: 9px 13px; margin-bottom: 12px; font-weight: 600; font-size: 0.85rem;
  display: flex; align-items: center; gap: 8px;
}
.flash.ok { background: rgba(34, 197, 94, 0.1); border: 1px solid rgba(34, 197, 94, 0.3); color: #86efac; }
.flash.err { background: rgba(239, 68, 68, 0.1); border: 1px solid rgba(239, 68, 68, 0.3); color: #fca5a5; }
.toolbar { display: flex; gap: 14px; align-items: center; margin-bottom: 10px; font-size: 0.84rem; }
.toolbar label { display: inline-flex; align-items: center; gap: 6px; cursor: pointer; }
.center { min-height: 100vh; display: grid; place-items: center; padding: 16px; }
.center .box { max-width: 480px; width: 100%; padding: 24px; border-radius: 14px; box-shadow: 0 10px 30px rgba(0,0,0,0.4); }
.lead { line-height: 1.55; color: #c9cbd5; font-size: 0.92rem; } .note { color: var(--muted); font-size: 0.79rem; }
::-webkit-scrollbar { width: 6px; height: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: rgba(255, 255, 255, 0.1); border-radius: 99px; }
::-webkit-scrollbar-thumb:hover { background: rgba(255, 255, 255, 0.2); }
@media (max-width: 760px) {
  .shell { grid-template-columns: 1fr; }
  aside {
    flex-direction: row; flex-wrap: wrap; border-right: 0; border-bottom: 1px solid var(--line);
    height: auto; position: static; padding: 10px 12px;
  }
  .brand { width: 100%; margin-bottom: 4px; } .me { margin-top: 0; border-top: 0; }
}
"#;

fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"tr\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"robots\" content=\"noindex\">\
         <title>{} · AxBot Panel</title><style>{CSS}</style></head><body>{body}</body></html>",
        escape(title)
    )
}

/// Giriş gerektirmeyen geniş sayfalar (herkese açık sıralama).
pub fn layout_public(title: &str, body: &str) -> String {
    document(
        title,
        &format!("<main style=\"margin:0 auto\"><div class=\"brand\" style=\"margin-left:0\">AX<span>BOT</span></div>{body}</main>"),
    )
}

/// Oturumsuz sayfalar (giriş, hata): ortada tek kart.
pub fn layout_bare(page: Page<'_>) -> String {
    document(
        page.title,
        &format!("<div class=\"center\"><div class=\"box\"><div class=\"brand\">AX<span>BOT</span> PANEL</div><h1>{}</h1>{}</div></div>", escape(page.title), page.body),
    )
}

pub async fn layout(state: &WebState, session: &Session, nav: Nav, guild: Option<GuildNav<'_>>, page: Page<'_>) -> String {
    let (name, avatar) = match state.http.get_user(session.user).await {
        Ok(u) => (u.global_name.clone().unwrap_or_else(|| u.name.clone()), u.face()),
        Err(_) => ("Kullanıcı".to_owned(), String::new()),
    };
    let owner = state.panel.is_owner(session.user);
    let link = |target: Nav, href: &str, label: &str| {
        format!("<a class=\"nav{}\" href=\"{href}\">{label}</a>", if nav == target { " on" } else { "" })
    };
    let l = session.lang;
    let t_overview = format!("🏠 {}", crate::i18n::t(l, "panel.overview", &[]));
    let t_logs = format!("📜 {}", crate::i18n::t(l, "panel.logs", &[]));
    let t_system = format!("⚙️ {}", crate::i18n::t(l, "panel.system", &[]));
    let t_plugins = format!("🧩 {}", crate::i18n::t(l, "panel.plugins", &[]));
    let t_wizard = format!("🧭 {}", crate::i18n::t(l, "panel.wizard", &[]));
    let t_stats = format!("📈 {}", crate::i18n::t(l, "panel.stats", &[]));
    let t_transcripts = format!("🎫 {}", crate::i18n::t(l, "panel.transcripts", &[]));
    let t_templates = format!("🧩 {}", crate::i18n::t(l, "panel.templates", &[]));
    let t_health = format!("🩺 {}", crate::i18n::t(l, "panel.health", &[]));
    let t_logout = crate::i18n::t(l, "panel.logout", &[]);
    let t_connecting = crate::i18n::t(l, "panel.connecting", &[]);

    let mut aside = String::from("<div class=\"brand\">AX<span>BOT</span> PANEL</div>");
    aside.push_str(&link(Nav::Home, "/panel", &t_overview));
    if owner {
        aside.push_str(&link(Nav::Logs, "/panel/loglar", &t_logs));
        aside.push_str(&link(Nav::System, "/panel/sistem", &t_system));
    }
    if let Some(guild) = &guild {
        let _ = write!(
            aside,
            "<div class=\"navh\">{}</div><a class=\"nav sm{}\" href=\"/panel/sunucu/{}\">{}</a>",
            escape(guild.name),
            if guild.active.is_empty() { " on" } else { "" },
            guild.id,
            escape(&t_plugins)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/kurulum\">{}</a>",
            if guild.active == "kurulum" { " on" } else { "" },
            guild.id,
            escape(&t_wizard)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/siralama\">📊 Sıralama</a>",
            if guild.active == "siralama" { " on" } else { "" },
            guild.id
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/istatistik\">{}</a>",
            if guild.active == "istatistik" { " on" } else { "" },
            guild.id,
            escape(&t_stats)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/transkriptler\">{}</a>",
            if guild.active == "transkriptler" { " on" } else { "" },
            guild.id,
            escape(&t_transcripts)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/sablonlar\">{}</a>",
            if guild.active == "sablonlar" { " on" } else { "" },
            guild.id,
            escape(&t_templates)
        );
        let _ = write!(
            aside,
            "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/saglik\">{}</a>",
            if guild.active == "saglik" { " on" } else { "" },
            guild.id,
            escape(&t_health)
        );
        let mut group = "";
        for plugin in super::plugins::PLUGINS {
            if plugin.group != group {
                group = plugin.group;
                let _ = write!(aside, "<div class=\"navg\">{}</div>", escape(group));
            }
            let _ = write!(
                aside,
                "<a class=\"nav sm{}\" href=\"/panel/sunucu/{}/eklenti/{}\">{} {}</a>",
                if guild.active == plugin.slug { " on" } else { "" },
                guild.id,
                plugin.slug,
                plugin.icon,
                escape(plugin.name)
            );
        }
    }
    let lang_switch = format!(
        "<div style=\"display:flex;gap:6px;justify-content:center;margin:6px 0 2px;font-size:0.75rem;\">\
         <a href=\"?lang=tr\" onclick=\"document.cookie='lang=tr;path=/;max-age=31536000';location.reload();return false;\" style=\"color:{};font-weight:{};\">TR</a>\
         <span style=\"color:var(--muted)\">|</span>\
         <a href=\"?lang=en\" onclick=\"document.cookie='lang=en;path=/;max-age=31536000';location.reload();return false;\" style=\"color:{};font-weight:{};\">EN</a>\
         </div>",
        if l == crate::i18n::Lang::Tr { "var(--accent)" } else { "var(--muted)" },
        if l == crate::i18n::Lang::Tr { "bold" } else { "normal" },
        if l == crate::i18n::Lang::En { "var(--accent)" } else { "var(--muted)" },
        if l == crate::i18n::Lang::En { "bold" } else { "normal" },
    );
    let _ = write!(
        aside,
        "<div class=\"me\">{}<div>{}<small>{} · <span id=\"ws-live-badge\" style=\"color:#faa81a;font-weight:bold;\">○ {}</span></small></div></div>\
         <form method=\"post\" action=\"/panel/cikis\"><input type=\"hidden\" name=\"csrf\" value=\"{}\">\
         <button class=\"ghost\" style=\"width:100%\">{}</button></form>{lang_switch}",
        if avatar.is_empty() { String::new() } else { format!("<img src=\"{}\" alt=\"\">", escape(&avatar)) },
        escape(&name),
        if owner { "Bot sahibi" } else { "Sunucu yöneticisi" },
        escape(&t_connecting),
        escape(&session.csrf),
        escape(&t_logout),
    );
    document(page.title, &format!("<div class=\"shell\"><aside>{aside}</aside><main>{}</main></div>", page.body))
}

pub fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, Html(layout_bare(Page { title: "Yetkin yok", body: "<p class=\"lead\">Bu sayfayı görme yetkin yok.</p><p><a href=\"/panel\">← Panele dön</a></p>".to_owned() })))
        .into_response()
}

fn human_duration(d: Duration) -> String {
    let s = d.as_secs();
    let (days, hours, mins) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    match (days, hours) {
        (0, 0) => format!("{mins} dk"),
        (0, h) => format!("{h} sa {mins} dk"),
        (d, h) => format!("{d} gün {h} sa"),
    }
}

/// Türkiye saatiyle (UTC+3) tarih.
fn turkish_time(unix: i64) -> String {
    let offset = chrono::FixedOffset::east_opt(3 * 3600);
    match (chrono::DateTime::from_timestamp(unix, 0), offset) {
        (Some(t), Some(o)) => t.with_timezone(&o).format("%d.%m.%Y %H:%M").to_string(),
        _ => "?".to_owned(),
    }
}

pub fn build_time() -> String {
    turkish_time(env!("AXBOT_BUILD_UNIX").parse().unwrap_or(0))
}

fn card(key: &str, value: &str) -> String {
    format!("<div class=\"card\"><div class=\"k\">{}</div><div class=\"v\">{value}</div></div>", escape(key))
}

async fn status_cards(state: &WebState) -> String {
    let (online, latency) = match state.panel.shards.get() {
        Some(manager) => {
            let runners = manager.runners.lock().await;
            let online = !runners.is_empty() && runners.values().all(|r| r.stage == poise::serenity_prelude::ConnectionStage::Connected);
            let latency = runners.values().filter_map(|r| r.latency).max();
            (online, latency)
        }
        None => (false, None),
    };
    let guilds = state.cache.guilds();
    let members: u64 = guilds.iter().filter_map(|g| state.cache.guild(*g).map(|g| g.member_count)).sum();

    let started = Instant::now();
    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();
    let db_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let mut conn = state.redis.clone();
    let redis_ok = redis::cmd("PING").query_async::<String>(&mut conn).await.is_ok();
    let redis_ms = started.elapsed().as_millis();

    let memory = memory_stats::memory_stats().map_or_else(|| "?".to_owned(), |m| format!("{} MB", m.physical_mem / 1_048_576));
    let (warns, errors) = logbuffer::counts();
    let health = |ok: bool, ms: u128| if ok { format!("{ms} ms") } else { "<span class=\"badge b-err\">KAPALI</span>".to_owned() };

    let mut out = String::from("<div class=\"cards\">");
    out.push_str(&card(
        "Durum",
        if online { "<span class=\"badge b-ok\">ÇEVRİMİÇİ</span>" } else { "<span class=\"badge b-warn\">BAĞLANIYOR</span>" },
    ));
    out.push_str(&card("Sunucular", &guilds.len().to_string()));
    out.push_str(&card("Toplam üye", &members.to_string()));
    out.push_str(&card("Çalışma süresi", &human_duration(state.panel.started_at.elapsed())));
    out.push_str(&card("Discord gecikmesi", &latency.map_or_else(|| "?".to_owned(), |l| format!("{} ms", l.as_millis()))));
    out.push_str(&card("Bellek", &memory));
    out.push_str(&card("PostgreSQL", &health(db_ok, db_ms)));
    out.push_str(&card("Redis", &health(redis_ok, redis_ms)));
    out.push_str(&card("Ses bağlantıları", &state.panel.songbird.iter().count().to_string()));
    out.push_str(&card(
        "Uyarı / hata",
        &format!(
            "<a href=\"/panel/loglar\"><span class=\"badge {}\">{warns}</span> <span class=\"badge {}\">{errors}</span></a>",
            if warns > 0 { "b-warn" } else { "b-off" },
            if errors > 0 { "b-err" } else { "b-off" }
        ),
    ));
    out.push_str("</div>");
    out
}

struct GuildCard {
    id: GuildId,
    name: String,
    icon: Option<String>,
    members: u64,
}

async fn manageable_guilds(state: &WebState, user: UserId) -> Vec<GuildCard> {
    let mut list = Vec::new();
    for gid in state.cache.guilds() {
        if !can_manage(state, user, gid).await {
            continue;
        }
        if let Some(g) = state.cache.guild(gid) {
            list.push(GuildCard { id: gid, name: g.name.clone(), icon: g.icon_url(), members: g.member_count });
        }
    }
    list.sort_by_key(|g| g.name.to_lowercase());
    list
}

fn guild_icon(name: &str, icon: Option<&str>) -> String {
    match icon {
        Some(url) => format!("<img src=\"{}\" alt=\"\">", escape(url)),
        None => format!("<div class=\"gicon\">{}</div>", escape(&name.chars().take(2).collect::<String>())),
    }
}

pub async fn home(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let owner = state.panel.is_owner(session.user);
    let mut body = String::from("<h1>Genel bakış</h1>");
    if owner {
        let _ = write!(body, "<p class=\"sub\">Sürüm {} · derleme {}</p>", env!("CARGO_PKG_VERSION"), build_time());
        body.push_str(&status_cards(&state).await);
        body.push_str("<h2>Sunucular</h2>");
    } else {
        body.push_str("<p class=\"sub\">Yönetebildiğin sunucular</p>");
    }
    let guilds = manageable_guilds(&state, session.user).await;
    if guilds.is_empty() {
        body.push_str("<div class=\"box\">Botun bulunduğu ve 'Sunucuyu Yönet' iznin olan bir sunucu yok.</div>");
    } else {
        body.push_str("<div class=\"guilds\">");
        for g in &guilds {
            let _ = write!(
                body,
                "<a class=\"guild\" href=\"/panel/sunucu/{}\">{}<div><b>{}</b><div class=\"note\">{} üye</div></div></a>",
                g.id,
                guild_icon(&g.name, g.icon.as_deref()),
                escape(&g.name),
                g.members
            );
        }
        body.push_str("</div>");
    }
    Html(layout(&state, &session, Nav::Home, None, Page { title: "Genel bakış", body }).await).into_response()
}

pub async fn guild(State(state): State<WebState>, headers: HeaderMap, Path(guild): Path<u64>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    let Some(guild_id) = (guild != 0).then(|| GuildId::new(guild)) else {
        return forbidden();
    };
    if !can_manage(&state, session.user, guild_id).await {
        return forbidden();
    }
    let report = match setup_report::build(&state.http, &state.cache, &state.db, &state.configs, guild_id).await {
        Ok(r) => r,
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Panel: sunucu raporu alınamadı");
            return (StatusCode::INTERNAL_SERVER_ERROR, Html(layout_bare(Page { title: "Bir sorun oluştu", body: "<p>Sunucu bilgisi alınamadı.</p>".to_owned() })))
                .into_response();
        }
    };

    // Kanal / rol adları önbellekten (await yok; önbellek kilidi hemen bırakılır).
    let rows: String = {
        let g = state.cache.guild(guild_id);
        report
            .features
            .iter()
            .map(|f| {
                let state_html = match &f.state {
                    FeatureState::On(Target::Channel(c)) => {
                        let name = g.as_ref().and_then(|g| g.channels.get(c).map(|c| c.name.clone())).unwrap_or_default();
                        format!("<span class=\"badge b-ok\">AÇIK</span> #{}", escape(&name))
                    }
                    FeatureState::On(Target::Role(r)) => {
                        let name = g.as_ref().and_then(|g| g.roles.get(r).map(|r| r.name.clone())).unwrap_or_default();
                        format!("<span class=\"badge b-ok\">AÇIK</span> @{}", escape(&name))
                    }
                    FeatureState::On(Target::Text(t)) => format!("<span class=\"badge b-ok\">AÇIK</span> {}", escape(t)),
                    FeatureState::Broken => "<span class=\"badge b-warn\">SİLİNMİŞ</span>".to_owned(),
                    FeatureState::Off => "<span class=\"badge b-off\">KAPALI</span>".to_owned(),
                };
                format!("<tr><td>{}</td><td>{state_html}</td><td><code>{}</code></td></tr>", escape(f.name), escape(f.command))
            })
            .collect()
    };

    let plugins = match state.configs.get(&state.db, guild_id).await {
        Ok(cfg) => {
            super::plugins::cards(guild_id, &cfg, super::plugins::Extra::load(&state, guild_id).await)
        }
        Err(e) => {
            warn!(guild = %guild_id, error = ?e, "Panel: sunucu ayarları okunamadı");
            String::new()
        }
    };
    let perms = if report.missing.is_empty() {
        "<div class=\"box\">✅ Botun tüm gerekli izinleri var.</div>".to_owned()
    } else {
        format!(
            "<div class=\"box err\">❌ Eksik izinler: <b>{}</b><br><span class=\"note\">Sunucu Ayarları → Roller → botun rolü → izinleri aç.</span></div>",
            escape(&crate::error::permission_names(report.missing))
        )
    };
    let role = if report.role_on_top {
        "<div class=\"box\">✅ Botun rolü en üstte.</div>"
    } else {
        "<div class=\"box warn\">⚠️ Botun rolü en üstte değil: altındaki rolleri veremez, o rollerdeki üyeleri susturamaz/atamaz. \
         Sunucu Ayarları → Roller → botun rolünü en üste sürükle.</div>"
    };
    let body = format!(
        "<p><a href=\"/panel\">← Sunucular</a></p>\
         <div class=\"ghead\">{icon}<div><h1>{name}</h1>\
         <div class=\"sub\" style=\"margin:0\">{members} üye · <a href=\"https://discord.com/channels/{id}\" target=\"_blank\" rel=\"noopener\">Discord'da aç</a></div></div></div>\
         <input type=\"search\" id=\"pfilter\" class=\"psearch\" placeholder=\"🔎 Eklenti ara…\" autocomplete=\"off\">\
         {plugins}<script src=\"/panel/panel.js\"></script><h2>Bot izinleri</h2>{perms}<div style=\"height:10px\"></div>{role}\
         <h2>Tüm özelliklerin durumu</h2><table><tr><th>Özellik</th><th>Durum</th><th>Discord'dan açmak / değiştirmek için</th></tr>{rows}</table>",
        icon = guild_icon(&report.guild_name, report.icon_url.as_deref()),
        name = escape(&report.guild_name),
        members = report.member_count,
        id = guild_id,
    );
    let nav = GuildNav { id: guild_id, name: &report.guild_name, active: "" };
    Html(layout(&state, &session, Nav::Home, Some(nav), Page { title: &report.guild_name, body }).await).into_response()
}

pub async fn logs(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    if !state.panel.is_owner(session.user) {
        return forbidden();
    }
    let body = "<h1>Loglar</h1><p class=\"sub\">Canlı: yeni satırlar 2 saniyede bir eklenir (bot açıldığından beri son 1000 satır).</p>\
                <div class=\"toolbar\"><label><input type=\"checkbox\" id=\"onlywarn\"> Sadece uyarı ve hatalar</label>\
                <label><input type=\"checkbox\" id=\"follow\" checked> Otomatik kaydır</label></div>\
                <pre id=\"log\"></pre><script src=\"/panel/panel.js\"></script>"
        .to_owned();
    Html(layout(&state, &session, Nav::Logs, None, Page { title: "Loglar", body }).await).into_response()
}

#[derive(Deserialize)]
pub struct LogQuery {
    after: Option<u64>,
    uyari: Option<u8>,
}

pub async fn logs_api(State(state): State<WebState>, headers: HeaderMap, Query(q): Query<LogQuery>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !state.panel.is_owner(session.user) {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(logbuffer::recent(q.after.unwrap_or(0), 400, q.uyari == Some(1))).into_response()
}

#[derive(Deserialize)]
pub struct SystemQuery {
    durum: Option<String>,
}

pub async fn system(State(state): State<WebState>, headers: HeaderMap, Query(query): Query<SystemQuery>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    if !state.panel.is_owner(session.user) {
        return forbidden();
    }
    let url = state.panel.public_url.get().unwrap_or_else(|| "—".to_owned());
    let invite = crate::commands::invite_url(state.cache.current_user().id);
    let restart = if state.panel.restartable {
        format!(
            "<form method=\"post\" action=\"/panel/sistem/yeniden-baslat\"><input type=\"hidden\" name=\"csrf\" value=\"{}\">\
             <button class=\"danger\">Botu yeniden başlat</button></form>\
             <p class=\"note\">Bot ~20 saniye çevrimdışı olur, sonra kendiliğinden açılır. Çalan müzik durur.</p>",
            escape(&session.csrf)
        )
    } else {
        "<p class=\"note\">Bot bekçisiz çalışıyor; panelden yeniden başlatılamaz.\
         <code>Botu Durdur.cmd</code> → <code>Botu Başlat.cmd</code> ile aç (bekçiyle açılır).</p>"
            .to_owned()
    };
    let presence = super::botpage::presence_form(&state, &session.csrf, query.durum.as_deref()).await;
    let body = format!(
        "<h1>Sistem</h1><p class=\"sub\">Sürüm, yeniden başlatma ve güncelleme</p>\
         <table><tr><th style=\"width:220px\">Sürüm</th><td>{version}</td></tr>\
         <tr><th>Derleme</th><td>{build}</td></tr>\
         <tr><th>Çalıştığı yer</th><td>{place}</td></tr>\
         <tr><th>Panel adresi</th><td><code>{url}</code></td></tr>\
         <tr><th>Yapay zekâ</th><td>{ai}</td></tr>\
         <tr><th>Üye olayları (intent)</th><td>{intent}</td></tr>\
         <tr><th>Discord ile giriş</th><td>{oauth}</td></tr>\
         <tr><th>Botu sunucuya ekle</th><td><a href=\"{invite}\" target=\"_blank\" rel=\"noopener\">Davet bağlantısı</a></td></tr></table>\
         <h2>Bot durumu</h2><div class=\"box\">{presence}</div>\
         <h2>Yeniden başlat</h2><div class=\"box\">{restart}</div>\
         <h2>Güncelleme</h2><div class=\"box\">1. Bilgisayarında <code>Sunucu Güncellemesi Hazırla.cmd</code>'ye çift tıkla → \
         <code>dist\\AxBot-guncelleme.tar.gz</code> oluşur.<br>2. Dosyayı arkadaşına gönder.<br>\
         3. Arkadaşın sunucuda: <code>tar -xzf AxBot-guncelleme.tar.gz &amp;&amp; cd AxBot-sunucu &amp;&amp; sudo bash kur.sh</code><br>\
         Veriler korunur; güncelleme sonrası bu sayfadaki <b>Derleme</b> tarihi değişir.</div>",
        version = env!("CARGO_PKG_VERSION"),
        build = build_time(),
        place = if state.panel.in_docker { "Sunucu (Docker)" } else { "Bu bilgisayar (Windows)" },
        url = escape(&url),
        ai = escape(&state.panel.ai.as_deref().unwrap_or("kapalı").replace('`', "")),
        intent = if state.panel.members_intent { "açık" } else { "kapalı" },
        oauth = format_args!(
            "{} · Developer Portal → OAuth2 → Redirects'e eklenecek adres: <code>{}</code>",
            if auth::oauth_enabled() { "açık" } else { "kapalı (<code>.env</code>: <code>DISCORD_CLIENT_SECRET</code>)" },
            escape(&auth::oauth_redirect_uri(&state))
        ),
        invite = escape(&invite),
    );
    Html(layout(&state, &session, Nav::System, None, Page { title: "Sistem", body }).await).into_response()
}

pub async fn restart(State(state): State<WebState>, headers: HeaderMap, Form(form): Form<CsrfForm>) -> Response {
    let Some(session) = auth::session(&state, &headers).await else {
        return auth::login_required();
    };
    if !state.panel.is_owner(session.user) || form.csrf != session.csrf || !state.panel.restartable {
        return forbidden();
    }
    let Some(manager) = state.panel.shards.get().cloned() else {
        return forbidden();
    };
    info!(user = %session.user, "Panelden yeniden başlatma istendi");
    // Cevap tarayıcıya ulaşsın diye kısa bir bekleme; kapanınca Docker botu yeniden açar.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        super::RESTART_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
        manager.shutdown_all().await;
    });
    Html(layout_bare(Page {
        title: "Yeniden başlatılıyor",
        body: "<p class=\"lead\">Bot kapanıp yeniden açılıyor. Bu sayfa 30 saniye sonra panele döner.</p>\
               <meta http-equiv=\"refresh\" content=\"30;url=/panel\">"
            .to_owned(),
    }))
    .into_response()
}

const SCRIPT: &str = r#"(() => {
  const filter = document.getElementById('pfilter');
  if (!filter) return;
  window.addEventListener('keydown', (e) => {
    if ((e.key === '/' || (e.ctrlKey && e.key === 'k')) && document.activeElement.tagName !== 'INPUT' && document.activeElement.tagName !== 'TEXTAREA') {
      e.preventDefault(); filter.focus();
    }
  });
  filter.addEventListener('input', () => {
    const q = filter.value.trim().toLocaleLowerCase('tr');
    document.querySelectorAll('.pcards').forEach(grid => {
      let any = false;
      grid.querySelectorAll('.pcard').forEach(card => {
        const show = !q || (card.dataset.ara || '').includes(q);
        card.style.display = show ? '' : 'none';
        any = any || show;
      });
      grid.style.display = any ? '' : 'none';
      const title = grid.previousElementSibling;
      if (title && title.classList.contains('pgroup')) title.style.display = any ? '' : 'none';
    });
  });
})();
(() => {
  document.querySelectorAll('.tpl').forEach(button => button.addEventListener('click', () => {
    let fields;
    try { fields = JSON.parse(button.dataset.doldur); } catch (e) { return; }
    let first = null;
    for (const [name, value] of Object.entries(fields)) {
      document.querySelectorAll('[name="' + name + '"]').forEach(el => {
        if (el.type === 'checkbox') el.checked = value === '1'; else el.value = value;
        first = first || el;
      });
    }
    if (first) first.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }));
})();
(() => {
  const box = document.getElementById('log');
  if (!box) return;
  const onlyWarn = document.getElementById('onlywarn');
  const follow = document.getElementById('follow');
  let after = 0, busy = false;
  async function tick() {
    if (busy) return; busy = true;
    try {
      const r = await fetch('/panel/api/loglar?after=' + after + '&uyari=' + (onlyWarn.checked ? 1 : 0), { credentials: 'same-origin' });
      if (r.ok) {
        const lines = await r.json();
        for (const l of lines) {
          after = Math.max(after, l.id);
          const row = document.createElement('div');
          row.className = 'l-' + l.level;
          const t = document.createElement('span'); t.className = 't'; t.textContent = l.time + ' ';
          row.appendChild(t);
          row.appendChild(document.createTextNode(l.level.padEnd(5) + ' ' + l.target + ': ' + l.text));
          box.appendChild(row);
        }
        while (box.childNodes.length > 1500) box.removeChild(box.firstChild);
        if (lines.length && follow.checked) box.scrollTop = box.scrollHeight;
      } else if (r.status === 401) { location.href = '/panel'; }
    } catch (e) {}
    busy = false;
  }
  onlyWarn.addEventListener('change', () => { box.textContent = ''; after = 0; tick(); });
  tick(); setInterval(tick, 2000);
})();
(() => {
  if (!location.pathname.startsWith('/panel')) return;
  let ws, timer;
  function connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(proto + '//' + location.host + '/panel/ws');
    ws.onopen = () => {
      const badge = document.getElementById('ws-live-badge');
      if (badge) { badge.textContent = '● Canlı'; badge.style.color = '#57f287'; }
    };
    ws.onmessage = (e) => {
      try {
        const ev = JSON.parse(e.data);
        if (window.onRealtimeEvent) window.onRealtimeEvent(ev);
        if (ev.type === 'track_change' && ev.data) {
          const trackEl = document.getElementById('now-track-title');
          if (trackEl) trackEl.textContent = ev.data.title;
        }
      } catch (err) {}
    };
    ws.onclose = () => {
      const badge = document.getElementById('ws-live-badge');
      if (badge) { badge.textContent = '○ Bağlanıyor'; badge.style.color = '#faa81a'; }
      clearTimeout(timer);
      timer = setTimeout(connect, 3000);
    };
  }
  connect();
})();
(() => {
  const preview = document.getElementById('embed-preview');
  if (!preview) return;
  const inTitle = document.querySelector('input[name="embed_title"]');
  const inDesc = document.querySelector('textarea[name="embed_description"]');
  const inColour = document.querySelector('input[name="embed_colour"]');
  const inAuthor = document.querySelector('input[name="embed_author"]');
  const inFooter = document.querySelector('input[name="embed_footer"]');
  const pTitle = document.getElementById('prev-title');
  const pDesc = document.getElementById('prev-desc');
  const pAuthor = document.getElementById('prev-author');
  const pFooter = document.getElementById('prev-footer');

  function update() {
    if (inColour && preview) preview.style.borderLeftColor = inColour.value;
    if (inTitle && pTitle) pTitle.textContent = inTitle.value || 'Gömülü Mesaj Başlığı';
    if (inDesc && pDesc) pDesc.textContent = inDesc.value || 'Mesaj açıklaması burada görünecektir.';
    if (inAuthor && pAuthor) {
      pAuthor.textContent = inAuthor.value;
      pAuthor.style.display = inAuthor.value ? 'block' : 'none';
    }
    if (inFooter && pFooter) {
      pFooter.textContent = inFooter.value;
      pFooter.style.display = inFooter.value ? 'block' : 'none';
    }
  }
  ['input', 'change'].forEach(ev => {
    document.addEventListener(ev, (e) => {
      if (e.target && e.target.name && e.target.name.startsWith('embed_')) update();
    });
  });
  update();
})();"#;

pub async fn script() -> Response {
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], SCRIPT).into_response()
}
