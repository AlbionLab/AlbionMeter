//! CopyDamage.tsx damageImage: the ranking as a picture. The card (ui/ranking.slint) is rendered with the software
//! renderer by a short-lived child process (`--render-ranking`), since the running window's platform cannot also render
//! offscreen, then put on the clipboard as an image.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};

use crate::feed::Snapshot;
use crate::format::{duration, fixed, precise};
use crate::i18n::{Language, t};
use crate::ranking::{Metric, contributions};
use crate::{CardRow, CardStat, RankingCard};

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Row {
    pub rank: String,
    pub name: String,
    pub weapon: String,
    pub amount: String,
    pub rate: String,
    pub color: String,
    pub fraction: f32,
    pub icon: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Card {
    pub title: String,
    pub subtitle: String,
    pub stats: Vec<(String, String)>,
    pub rows: Vec<Row>,
    pub total: String,
    pub sans: String,
    pub mono: String,
}

/// 1.75M / 837.5K / 999 (decimals in the language's format, no grouping), as compactNumber does.
fn compact_number(language: Language, value: f64) -> String {
    if value >= 1e6 {
        format!("{}M", fixed(language, value / 1e6, 2, false))
    } else if value >= 1e3 {
        format!("{}K", fixed(language, value / 1e3, 1, false))
    } else {
        precise(language, value.round())
    }
}

/// toLocaleString(locale, { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' }) of an
/// ISO 8601 time, in UTC.
pub fn when(language: Language, iso: &str) -> String {
    let number = |range: std::ops::Range<usize>| iso.get(range).and_then(|part| part.parse::<u32>().ok());
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute)) = (number(0..4), number(5..7), number(8..10), number(11..13), number(14..16)) else {
        return String::new();
    };
    if !(1..=12).contains(&month) {
        return String::new();
    }
    match language {
        Language::En => {
            const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            let (twelve, half) = (if hour % 12 == 0 { 12 } else { hour % 12 }, if hour < 12 { "AM" } else { "PM" });
            format!("{} {day}, {year}, {twelve:02}:{minute:02} {half}", MONTHS[month as usize - 1])
        }
        Language::Pt => {
            const MONTHS: [&str; 12] = ["jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.", "dez."];
            format!("{day} de {} de {year}, {hour:02}:{minute:02}", MONTHS[month as usize - 1])
        }
    }
}

pub fn card(snapshot: &Snapshot, metric: Metric, language: Language, icons: &HashMap<Arc<str>, PathBuf>, fonts: (String, String)) -> Card {
    let players = contributions(snapshot, metric);
    let total_damage: f64 = players.iter().map(|player| player.damage).sum();
    let total_healing: f64 = players.iter().map(|player| player.healing).sum();
    let elapsed = snapshot.elapsed_seconds.max(1.0);
    let count = format!("{} {}", players.len(), t(language, if players.len() == 1 { "player" } else { "players" }));
    let subtitle = [when(language, &snapshot.started_at), duration(snapshot.elapsed_seconds), count].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("  ·  ");
    let stat = |label: &str, value: String| (t(language, label).to_owned(), value);
    let stats = match metric {
        Metric::Damage => vec![
            stat("DAMAGE", compact_number(language, total_damage)),
            stat("HEALING", compact_number(language, total_healing)),
            stat("PARTY DPS", precise(language, (total_damage / elapsed).round())),
        ],
        Metric::Healing => vec![
            stat("HEALING", compact_number(language, total_healing)),
            stat("DAMAGE", compact_number(language, total_damage)),
            stat("PARTY HPS", precise(language, (total_healing / elapsed).round())),
        ],
    };
    let total: f64 = players.iter().map(|player| metric.of(player)).sum();
    let leader = players.first().map_or(0.0, |player| metric.of(player)).max(1.0);
    let rows = players
        .iter()
        .enumerate()
        .map(|(index, player)| {
            let value = metric.of(player);
            let share = if total > 0.0 { value / total * 100.0 } else { 0.0 };
            let color = &*player.color;
            let valid = color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit());
            Row {
                rank: (index + 1).to_string(),
                name: player.name.to_string(),
                weapon: if player.weapon_id.is_some() {
                    player.weapon.to_string()
                } else if &*player.weapon == "Unarmed" {
                    t(language, "Unarmed").to_owned()
                } else {
                    String::new()
                },
                amount: format!("{}  {}%", compact_number(language, value), fixed(language, share, 1, false)),
                rate: format!("{} {}", precise(language, metric.rate(player).round()), metric.unit()),
                color: if valid { color.to_owned() } else { "#9ebbf3".into() },
                fraction: (value / leader) as f32,
                icon: player.weapon_id.as_ref().and_then(|id| icons.get(id)).cloned(),
            }
        })
        .collect();
    Card {
        title: t(language, if metric == Metric::Damage { "Party damage" } else { "Party healing" }).to_owned(),
        subtitle,
        stats,
        rows,
        total: format!("{}  {}", t(language, "Total"), precise(language, total.round())),
        sans: fonts.0,
        mono: fonts.1,
    }
}

/// Renders the card at twice its size: (width, height, RGBA). Must run in a process of its own (it sets the platform).
pub fn render(card: &Card) -> Result<(u32, u32, Vec<u8>), String> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(crate::headless::Headless { window: window.clone(), start: Instant::now() })).map_err(|error| error.to_string())?;
    let component = RankingCard::new().map_err(|error| error.to_string())?;
    component.set_heading(card.title.as_str().into());
    component.set_subtitle(card.subtitle.as_str().into());
    component.set_total(card.total.as_str().into());
    component.set_sans(card.sans.as_str().into());
    component.set_mono(card.mono.as_str().into());
    component.set_stats(Rc::new(slint::VecModel::from(card.stats.iter().map(|(label, value)| CardStat { label: label.as_str().into(), value: value.as_str().into() }).collect::<Vec<_>>())).into());
    component.set_rows(
        Rc::new(slint::VecModel::from(
            card.rows
                .iter()
                .map(|row| {
                    let icon = row.icon.as_deref().and_then(|path| slint::Image::load_from_path(path).ok());
                    CardRow {
                        rank: row.rank.as_str().into(),
                        name: row.name.as_str().into(),
                        weapon: row.weapon.as_str().into(),
                        amount: row.amount.as_str().into(),
                        rate: row.rate.as_str().into(),
                        color: crate::controller::color(&row.color),
                        fraction: row.fraction,
                        has_icon: icon.is_some(),
                        icon: icon.unwrap_or_default(),
                    }
                })
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    let height = (84 + card.rows.len() as u32 * 39 + 30) * 2;
    let width = 720 * 2;
    window.set_size(slint::PhysicalSize::new(width, height));
    component.show().map_err(|error| error.to_string())?;
    let mut buffer = vec![PremultipliedRgbaColor::default(); (width * height) as usize];
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut buffer, width as usize);
        });
    }
    Ok((width, height, buffer.iter().flat_map(|pixel| [pixel.red, pixel.green, pixel.blue, 255]).collect()))
}

/// `--render-ranking <card.json> <out>`: the child's side; writes width and height (u32 LE) then the RGBA pixels.
pub fn child(input: &Path, output: &Path) -> Result<(), String> {
    let card: Card = serde_json::from_slice(&std::fs::read(input).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    let (width, height, rgba) = render(&card)?;
    let mut bytes = Vec::with_capacity(8 + rgba.len());
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    bytes.extend(rgba);
    std::fs::write(output, bytes).map_err(|error| error.to_string())
}

/// The parent's side: renders `card` in a child process (call it off the UI thread).
pub fn render_in_child(card: &Card) -> Result<(u32, u32, Vec<u8>), String> {
    let folder = std::env::temp_dir().join(format!("albion-meter-ranking-{}", std::process::id()));
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let (input, output) = (folder.join("card.json"), folder.join("card.rgba"));
    std::fs::write(&input, serde_json::to_vec(card).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    let status = std::process::Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
        .arg("--render-ranking")
        .arg(&input)
        .arg(&output)
        .status()
        .map_err(|error| error.to_string())?;
    let bytes = std::fs::read(&output).map_err(|error| error.to_string());
    let _ = std::fs::remove_dir_all(&folder);
    let bytes = bytes?;
    if !status.success() || bytes.len() < 8 {
        return Err("Image rendering failed.".into());
    }
    let width = u32::from_le_bytes(bytes[0..4].try_into().unwrap_or_default());
    let height = u32::from_le_bytes(bytes[4..8].try_into().unwrap_or_default());
    Ok((width, height, bytes[8..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::Player;

    #[test]
    fn dates_and_numbers_follow_damage_image() {
        assert_eq!(when(Language::En, "2026-11-26T21:05:00.000Z"), "Nov 26, 2026, 09:05 PM");
        assert_eq!(when(Language::Pt, "2026-11-26T21:05:00.000Z"), "26 de nov. de 2026, 21:05");
        assert_eq!(when(Language::En, "2026-01-01T00:00:00Z"), "Jan 1, 2026, 12:00 AM");
        assert_eq!(compact_number(Language::En, 1_747_647.0), "1.75M");
        assert_eq!(compact_number(Language::Pt, 837_500.0), "837,5K");
        assert_eq!(compact_number(Language::En, 999.4), "999");
    }

    #[test]
    fn rows_rank_players_like_the_text() {
        let player = |id: &str, name: &str, damage: f64, weapon_id: Option<&str>| Player {
            id: id.into(),
            name: name.into(),
            weapon: "Chillhowl · T8.4".into(),
            weapon_id: weapon_id.map(Into::into),
            color: "#9ebbf3".into(),
            damage,
            encounter_dps: damage / 100.0,
            ..Player::default()
        };
        let snapshot = Snapshot {
            started_at: "2026-11-26T21:05:00.000Z".into(),
            elapsed_seconds: 100.0,
            members: vec![player("a", "B", 1000.0, None), player("b", "A", 3000.0, Some("T8_X"))],
            ..Snapshot::default()
        };
        let icons = HashMap::from([(Arc::from("T8_X"), PathBuf::from("/x.png"))]);
        let card = card(&snapshot, Metric::Damage, Language::En, &icons, (String::new(), String::new()));
        assert_eq!(card.subtitle, "Nov 26, 2026, 09:05 PM  ·  01:40  ·  2 players");
        assert_eq!(card.stats[2], ("PARTY DPS".to_owned(), "40".to_owned()));
        assert_eq!(card.rows[0].name, "A");
        assert_eq!(card.rows[0].amount, "3.0K  75.0%");
        assert_eq!(card.rows[0].weapon, "Chillhowl · T8.4");
        assert_eq!(card.rows[1].weapon, "");
        assert_eq!(card.rows[0].icon, Some(PathBuf::from("/x.png")));
        assert_eq!(card.total, "Total  4,000");
    }
}
