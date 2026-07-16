//! 上等马 / 中等马 / 下等马 评分模块
//!
//! 算法移植自 [hh-lol-prophet](https://github.com/real-web-world/hh-lol-prophet)
//! （`app.go::GetUserScore` / `calcUserGameScore`），默认阈值与加分项与上游一致。
//!
//! 用于在选人/对局会话中给每位玩家打一个「马」标签，方便快速判断队友近期状态。

use crate::lcu::api::match_history::MatchHistory;
use crate::lcu::api::model::{Participant, Stats};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 默认基础分
const DEFAULT_SCORE: f64 = 100.0;

/// 允许计入评分的队列：匹配 / 单双 / 大乱斗 / 灵活 / 斗魂
const ALLOW_QUEUE_IDS: [i32; 5] = [430, 420, 450, 440, 1700];

/// 最短对局时长（秒），短于此时长的局不计入
const GAME_MIN_DURATION: i32 = 900;

/// 近 5 小时内对局权重 80%，其余 20%
const RECENT_HOURS: u64 = 5;
const RECENT_WEIGHT: f64 = 0.8;
const OLDER_WEIGHT: f64 = 0.2;

/// 马的评级结果
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HorseScore {
    /// 综合分（加权平均）
    pub score: f64,
    /// 马名：通天代 / 小代 / 上等马 / 中等马 / 下等马 / 牛马
    pub horse: String,
    /// 是否属于正面标签（上等马及以上）
    pub good: bool,
    /// 用于描述的最近几局 KDA 摘要，例如 "5/2/8  3/4/10"
    pub recent_kda: String,
}

/// 马阈值（从高到低匹配）
const HORSE_TIERS: [(f64, &str, bool); 6] = [
    (180.0, "通天代", true),
    (150.0, "小代", true),
    (125.0, "上等马", true),
    (105.0, "中等马", true),
    (95.0, "下等马", false),
    (0.0001, "牛马", false),
];

/// 从战绩计算玩家的马评分。
///
/// `puuid` 用于在 `game_detail` 里定位该玩家；若定位失败会尝试 `participants[0]`
/// （匹配历史接口「按 puuid 查」时第一个参与者就是本人）。
pub fn calc_horse_score(match_history: &MatchHistory, puuid: &str) -> HorseScore {
    let mut recent_scores: Vec<f64> = Vec::new();
    let mut older_scores: Vec<f64> = Vec::new();
    let mut kda_parts: Vec<String> = Vec::new();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    for game in &match_history.games.games {
        if !ALLOW_QUEUE_IDS.contains(&game.queue_id) {
            continue;
        }
        if game.game_duration < GAME_MIN_DURATION {
            continue;
        }
        if game.game_detail.participants.is_empty() {
            continue;
        }

        let Some((me, team_mates)) = find_player_and_teammates(game, puuid) else {
            continue;
        };

        // 最近 KDA 摘要（最多 5 局，按时间从新到旧）
        if kda_parts.len() < 5 {
            kda_parts.push(format!(
                "{}/{}/{}",
                me.stats.kills, me.stats.deaths, me.stats.assists
            ));
        }

        let score = calc_game_score(&me.stats, &team_mates, game.game_duration);
        if is_within_recent_hours(&game.game_creation_date, now) {
            recent_scores.push(score);
        } else {
            older_scores.push(score);
        }
    }

    let score = weighted_average(&recent_scores, &older_scores);
    let (horse, good) = horse_for_score(score);
    HorseScore {
        score: (score * 10.0).round() / 10.0,
        horse: horse.to_string(),
        good,
        recent_kda: kda_parts.join("  "),
    }
}

fn horse_for_score(score: f64) -> (&'static str, bool) {
    for (threshold, name, good) in HORSE_TIERS {
        if score >= threshold {
            return (name, good);
        }
    }
    ("牛马", false)
}

fn weighted_average(recent: &[f64], older: &[f64]) -> f64 {
    let total_count = recent.len() + older.len();
    if total_count == 0 {
        return DEFAULT_SCORE;
    }
    let total_avg = (recent.iter().sum::<f64>() + older.iter().sum::<f64>()) / total_count as f64;
    let recent_avg = if recent.is_empty() {
        total_avg
    } else {
        recent.iter().sum::<f64>() / recent.len() as f64
    };
    let older_avg = if older.is_empty() {
        total_avg
    } else {
        older.iter().sum::<f64>() / older.len() as f64
    };
    recent_avg * RECENT_WEIGHT + older_avg * OLDER_WEIGHT
}

/// 在对局详情中定位玩家及其队友（同队）。
fn find_player_and_teammates(
    game: &crate::lcu::api::match_history::Game,
    puuid: &str,
) -> Option<(Participant, Vec<Participant>)> {
    let detail = &game.game_detail;
    // 优先按 puuid 找
    let me_identity = detail
        .participant_identities
        .iter()
        .find(|id| !puuid.is_empty() && id.player.puuid == puuid);

    let me_pid = if let Some(id) = me_identity {
        // LCU GameDetail 的 ParticipantIdentity 通常没有 participantId 字段
        // 与 participants 一一对应：按索引对齐
        detail
            .participant_identities
            .iter()
            .position(|x| std::ptr::eq(x, id))
            .and_then(|idx| detail.participants.get(idx).map(|p| p.participant_id))
    } else if !game.participants.is_empty() {
        // 按 puuid 查战绩时 participants[0] 是本人
        Some(game.participants[0].participant_id)
    } else {
        None
    }?;

    let me = detail
        .participants
        .iter()
        .find(|p| p.participant_id == me_pid)?
        .clone();

    let team_mates: Vec<Participant> = detail
        .participants
        .iter()
        .filter(|p| p.team_id == me.team_id)
        .cloned()
        .collect();

    Some((me, team_mates))
}

/// 解析 LCU 的 gameCreationDate（RFC3339 或类似格式），判断是否在近 N 小时内。
fn is_within_recent_hours(creation: &str, now: Duration) -> bool {
    let Some(secs) = parse_creation_secs(creation) else {
        return false;
    };
    let age = now.as_secs().saturating_sub(secs);
    age <= RECENT_HOURS * 3600
}

fn parse_creation_secs(creation: &str) -> Option<u64> {
    // 常见格式：
    // 1) 2024-05-01T12:34:56.789Z
    // 2) 2024-05-01T12:34:56.789+08:00
    // 3) 纯毫秒时间戳字符串
    if let Ok(ms) = creation.parse::<i64>() {
        return Some((ms.max(0) as u64) / 1000);
    }
    // 简易 RFC3339：取日期时间部分，按 UTC 粗算（评分权重不需要秒级精度）
    // YYYY-MM-DDTHH:MM:SS
    if creation.len() < 19 {
        return None;
    }
    let y: i64 = creation.get(0..4)?.parse().ok()?;
    let mo: i64 = creation.get(5..7)?.parse().ok()?;
    let d: i64 = creation.get(8..10)?.parse().ok()?;
    let h: i64 = creation.get(11..13)?.parse().ok()?;
    let mi: i64 = creation.get(14..16)?.parse().ok()?;
    let s: i64 = creation.get(17..19)?.parse().ok()?;
    // 简化：按 30 天月、不考虑闰年，对「5 小时窗口」足够
    let days = (y - 1970) * 365 + (y - 1969) / 4 + (mo - 1) * 30 + (d - 1);
    let secs = days * 86400 + h * 3600 + mi * 60 + s;
    Some(secs.max(0) as u64)
}

/// 单局评分（移植自 hh-lol-prophet `calcUserGameScore`）
fn calc_game_score(me: &Stats, team: &[Participant], game_duration: i32) -> f64 {
    let mut score = DEFAULT_SCORE;

    let total_kill: i32 = team.iter().map(|p| p.stats.kills).sum();
    let total_assist: i32 = team.iter().map(|p| p.stats.assists).sum();
    let total_hurt: i32 = team
        .iter()
        .map(|p| p.stats.total_damage_dealt_to_champions)
        .sum();
    let total_money: i32 = team.iter().map(|p| p.stats.gold_earned).sum();

    // 一血
    if me.first_blood_kill {
        score += 10.0;
    } else if me.first_blood_assist {
        score += 5.0;
    }

    // 多杀（只取最高档）
    if me.penta_kills > 0 {
        score += 20.0;
    } else if me.quadra_kills > 0 {
        score += 10.0;
    } else if me.triple_kills > 0 {
        score += 5.0;
    }

    // 参团率排名
    if total_kill > 0 {
        let my_rate = (me.kills + me.assists) as f64 / total_kill as f64;
        let rank = rank_of(my_rate, team, |p| {
            (p.stats.kills + p.stats.assists) as f64 / total_kill as f64
        });
        match rank {
            1 => score += 10.0,
            2 => score += 5.0,
            4 => score -= 5.0,
            5 => score -= 10.0,
            _ => {}
        }
    }

    // 打钱排名（辅助不扣分）
    if total_money > 0 {
        let rank = rank_of(me.gold_earned as f64, team, |p| p.stats.gold_earned as f64);
        // 没有 timeline 时用「补刀偏低 + 助攻偏高」粗判辅助，避免误伤
        let is_support = is_likely_support(me, team);
        match rank {
            1 => score += 10.0,
            2 => score += 5.0,
            4 if !is_support => score -= 5.0,
            5 if !is_support => score -= 10.0,
            _ => {}
        }
    }

    // 伤害排名
    if total_hurt > 0 {
        let rank = rank_of(me.total_damage_dealt_to_champions as f64, team, |p| {
            p.stats.total_damage_dealt_to_champions as f64
        });
        match rank {
            1 => score += 10.0,
            2 => score += 5.0,
            _ => {}
        }
    }

    // 金钱转伤害比排名
    if total_money > 0 && total_hurt > 0 && me.gold_earned > 0 {
        let my_rate = me.total_damage_dealt_to_champions as f64 / me.gold_earned as f64;
        let rank = rank_of(my_rate, team, |p| {
            if p.stats.gold_earned > 0 {
                p.stats.total_damage_dealt_to_champions as f64 / p.stats.gold_earned as f64
            } else {
                0.0
            }
        });
        match rank {
            1 => score += 10.0,
            2 => score += 5.0,
            _ => {}
        }
    }

    // 视野得分排名
    {
        let rank = rank_of(me.vision_score as f64, team, |p| p.stats.vision_score as f64);
        match rank {
            1 => score += 10.0,
            2 => score += 5.0,
            _ => {}
        }
    }

    // 补兵 / 分钟
    {
        let minutes = (game_duration / 60).max(1);
        let cs_per_min =
            (me.total_minions_killed + me.neutral_minions_killed) / minutes;
        // 从高到低匹配
        if cs_per_min >= 10 {
            score += 20.0;
        } else if cs_per_min >= 9 {
            score += 10.0;
        } else if cs_per_min >= 8 {
            score += 5.0;
        }
    }

    // 击杀占比
    if total_kill > 0 {
        let rate = me.kills as f64 / total_kill as f64 * 100.0;
        if let Some(bonus) = rate_bonus(
            rate,
            me.kills,
            &[(50.0, &[(15, 40.0), (10, 20.0), (5, 10.0)]),
              (40.0, &[(15, 20.0), (10, 10.0), (5, 5.0)])],
        ) {
            score += bonus;
        }
    }

    // 伤害占比
    if total_hurt > 0 {
        let rate = me.total_damage_dealt_to_champions as f64 / total_hurt as f64 * 100.0;
        if let Some(bonus) = rate_bonus(
            rate,
            me.kills,
            &[(40.0, &[(15, 40.0), (10, 20.0), (5, 10.0)]),
              (30.0, &[(15, 20.0), (10, 10.0), (5, 5.0)])],
        ) {
            score += bonus;
        }
    }

    // 助攻占比
    if total_assist > 0 {
        let rate = me.assists as f64 / total_assist as f64 * 100.0;
        // 上游用 kills 作门槛（命名有点怪，保持一致）
        if let Some(bonus) = rate_bonus(
            rate,
            me.kills,
            &[(50.0, &[(20, 30.0), (18, 25.0), (15, 20.0), (10, 10.0), (5, 5.0)]),
              (40.0, &[(20, 15.0), (15, 10.0), (10, 5.0), (5, 3.0)])],
        ) {
            score += bonus;
        }
    }

    // KDA 微调：((k+a)/d - 2 + (k-d)/5) * 参团率
    let deaths = me.deaths.max(1) as f64;
    let join_rate = if total_kill > 0 {
        (me.kills + me.assists) as f64 / total_kill as f64
    } else {
        1.0
    };
    let adjust = ((me.kills + me.assists) as f64 / deaths - 2.0
        + (me.kills - me.deaths) as f64 / 5.0)
        * join_rate;
    score += adjust;

    score
}

/// 返回 `my` 在 `team` 中的名次（1=最高）。并列时按「有多少人严格更高 + 1」。
fn rank_of<F>(my: f64, team: &[Participant], f: F) -> usize
where
    F: Fn(&Participant) -> f64,
{
    1 + team.iter().filter(|p| f(p) > my).count()
}

/// 粗略判断是否辅助：助攻占比高且补刀偏低。
fn is_likely_support(me: &Stats, team: &[Participant]) -> bool {
    let total_assist: i32 = team.iter().map(|p| p.stats.assists).sum();
    let total_cs: i32 = team
        .iter()
        .map(|p| p.stats.total_minions_killed + p.stats.neutral_minions_killed)
        .sum();
    let my_cs = me.total_minions_killed + me.neutral_minions_killed;
    let assist_share = if total_assist > 0 {
        me.assists as f64 / total_assist as f64
    } else {
        0.0
    };
    let cs_share = if total_cs > 0 {
        my_cs as f64 / total_cs as f64
    } else {
        0.0
    };
    assist_share >= 0.3 && cs_share <= 0.15
}

/// 占比加分：先按 limit 从高到低匹配，再按「人头门槛」从高到低匹配。
fn rate_bonus(rate_pct: f64, kills: i32, table: &[(f64, &[(i32, f64)])]) -> Option<f64> {
    for (limit, score_conf) in table {
        if rate_pct > *limit {
            for (kill_min, bonus) in *score_conf {
                if kills > *kill_min {
                    return Some(*bonus);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcu::api::game_detail::GameDetail;
    use crate::lcu::api::match_history::{Game, GamesWrapper, MatchHistory};
    use crate::lcu::api::model::{Participant, ParticipantIdentity, Player, Stats};

    fn stats(k: i32, d: i32, a: i32, dmg: i32, gold: i32, cs: i32) -> Stats {
        Stats {
            kills: k,
            deaths: d,
            assists: a,
            total_damage_dealt_to_champions: dmg,
            gold_earned: gold,
            total_minions_killed: cs,
            vision_score: 20,
            ..Default::default()
        }
    }

    fn team5(me: Stats) -> Vec<Participant> {
        let mut team = vec![Participant {
            participant_id: 1,
            team_id: 100,
            champion_id: 1,
            spell1_id: 0,
            spell2_id: 0,
            stats: me,
        }];
        for i in 2..=5 {
            team.push(Participant {
                participant_id: i,
                team_id: 100,
                champion_id: i,
                spell1_id: 0,
                spell2_id: 0,
                stats: stats(3, 5, 4, 8000, 9000, 120),
            });
        }
        // 对面 5 人
        for i in 6..=10 {
            team.push(Participant {
                participant_id: i,
                team_id: 200,
                champion_id: i,
                spell1_id: 0,
                spell2_id: 0,
                stats: stats(4, 4, 4, 10000, 10000, 150),
            });
        }
        team
    }

    #[test]
    fn horse_tiers_map_correctly() {
        assert_eq!(horse_for_score(200.0).0, "通天代");
        assert_eq!(horse_for_score(150.0).0, "小代");
        assert_eq!(horse_for_score(125.0).0, "上等马");
        assert_eq!(horse_for_score(105.0).0, "中等马");
        assert_eq!(horse_for_score(95.0).0, "下等马");
        assert_eq!(horse_for_score(50.0).0, "牛马");
    }

    #[test]
    fn empty_history_returns_default_mid_horse() {
        let mh = MatchHistory::default();
        let hs = calc_horse_score(&mh, "puuid");
        assert!((hs.score - 100.0).abs() < 0.01);
        assert_eq!(hs.horse, "下等马"); // 100 < 105 → 下等马
    }

    #[test]
    fn carry_game_scores_above_default() {
        let carry = stats(15, 2, 10, 35000, 15000, 220);
        let team = team5(carry.clone());
        // only teammates for calc_game_score
        let mates: Vec<_> = team.iter().filter(|p| p.team_id == 100).cloned().collect();
        let s = calc_game_score(&carry, &mates, 1800);
        assert!(s > 120.0, "carry should score high, got {}", s);
    }

    #[test]
    fn feeder_game_scores_below_default() {
        let feeder = stats(0, 12, 1, 3000, 6000, 40);
        let team = team5(feeder.clone());
        let mates: Vec<_> = team.iter().filter(|p| p.team_id == 100).cloned().collect();
        let s = calc_game_score(&feeder, &mates, 1800);
        assert!(s < 100.0, "feeder should score low, got {}", s);
    }

    #[test]
    fn calc_horse_from_match_history() {
        let carry = stats(12, 2, 8, 30000, 14000, 200);
        let mut parts = team5(carry);
        // ensure pid 1 is me
        let mut identities = Vec::new();
        for i in 1..=10 {
            identities.push(ParticipantIdentity {
                player: Player {
                    puuid: if i == 1 {
                        "me".into()
                    } else {
                        format!("p{i}")
                    },
                    ..Default::default()
                },
            });
        }
        let game = Game {
            game_id: 1,
            game_creation_date: "2099-01-01T00:00:00.000Z".into(), // treated as recent-ish
            game_duration: 1800,
            queue_id: 420,
            participants: vec![parts[0].clone()],
            game_detail: GameDetail {
                participants: parts.drain(..).collect(),
                participant_identities: identities,
                ..Default::default()
            },
            ..Default::default()
        };
        let mh = MatchHistory {
            games: GamesWrapper { games: vec![game] },
            ..Default::default()
        };
        let hs = calc_horse_score(&mh, "me");
        assert!(hs.score > 100.0, "got {}", hs.score);
        assert!(!hs.horse.is_empty());
        assert!(hs.recent_kda.contains('/'));
    }
}
