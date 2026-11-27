//! web/src/CopyDamage.tsx: the ranking as text other Albion meters paste in chat, and the players' names.

use crate::feed::{Player, Snapshot};
use crate::format::fixed;
use crate::i18n::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Damage,
    Healing,
}

impl Metric {
    pub fn of(self, player: &Player) -> f64 {
        match self {
            Self::Damage => player.damage,
            Self::Healing => player.healing,
        }
    }

    pub fn rate(self, player: &Player) -> f64 {
        match self {
            Self::Damage => player.encounter_dps,
            Self::Healing => player.encounter_hps,
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Self::Damage => "DPS",
            Self::Healing => "HPS",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Damage => "damage",
            Self::Healing => "healing",
        }
    }
}

/// String.prototype.localeCompare, closely enough for player names: case-insensitive first, then by case.
pub fn name_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
}

/// Current and former members once each (the current entry wins), highest first, then by name.
pub fn contributions(snapshot: &Snapshot, metric: Metric) -> Vec<&Player> {
    let mut players: Vec<&Player> = snapshot.members.iter().collect();
    players.extend(snapshot.former_members.iter().filter(|former| !snapshot.members.iter().any(|member| member.id == former.id)));
    players.sort_by(|a, b| metric.of(b).total_cmp(&metric.of(a)).then_with(|| name_order(&a.name, &b.name)));
    players
}

/// "1. Name: 1747647(14,40%)|1234,56 DPS": decimals with a comma whatever the language, like Albion Statistics Analysis.
pub fn ranking_text(snapshot: &Snapshot, metric: Metric) -> String {
    let players = contributions(snapshot, metric);
    let total: f64 = players.iter().map(|player| metric.of(player)).sum();
    players
        .iter()
        .enumerate()
        .map(|(index, player)| {
            let value = metric.of(player);
            let share = if total > 0.0 { value / total * 100.0 } else { 0.0 };
            format!(
                "{}. {}: {}({}%)|{} {}",
                index + 1,
                player.name,
                value.round(),
                fixed(Language::Pt, share, 2, false),
                fixed(Language::Pt, metric.rate(player), 2, false),
                metric.unit()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn player_names(snapshot: &Snapshot, metric: Metric) -> String {
    contributions(snapshot, metric).iter().map(|player| &*player.name).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(id: &str, name: &str, damage: f64, dps: f64) -> Player {
        Player { id: id.into(), name: name.into(), damage, encounter_dps: dps, ..Player::default() }
    }

    #[test]
    fn ranks_like_the_web_copy() {
        let snapshot = Snapshot {
            members: vec![player("a", "Lusphu", 1_000.0, 12.5), player("b", "KPey", 3_000.4, 1234.556)],
            former_members: vec![player("a", "Lusphu", 1.0, 0.0), player("c", "Gone", 0.0, 0.0)],
            ..Snapshot::default()
        };
        assert_eq!(ranking_text(&snapshot, Metric::Damage), "1. KPey: 3000(75,00%)|1234,56 DPS\n2. Lusphu: 1000(25,00%)|12,50 DPS\n3. Gone: 0(0,00%)|0,00 DPS");
        assert_eq!(player_names(&snapshot, Metric::Damage), "KPey\nLusphu\nGone");
    }
}
