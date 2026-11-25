//! The arena report: static HTML pages with the standings, the rating of each entrant over the
//! schedule, every match, and the VP of every seat turn by turn. No script, no network: the
//! pages open from the disk.

use crate::config::ArenaConfig;
use crate::leaderboard::{Leaderboard, leaderboard};
use crate::run::{MatchRecord, rating_history};
use crate::schedule::fixtures;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const REPORT: &str = "report";

/// Series colours, chosen to stay apart for up to eight lines.
const COLOURS: [&str; 8] = [
    "#1f5f99", "#b5482a", "#2f7d4f", "#7b4fa3", "#b08410", "#227a7a", "#9c3561", "#5b5b5b",
];

/// Writes `report/index.html` and one page per match into the arena folder `dir`. Returns the
/// path of the index.
pub fn write_report(
    config: &ArenaConfig,
    records: &[MatchRecord],
    dir: &Path,
) -> Result<PathBuf, String> {
    let folder = dir.join(REPORT);
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let order = fixtures(&crate::run::names(config), &config.schedule);
    let mut played: Vec<(usize, &MatchRecord)> = records
        .iter()
        .filter_map(|r| {
            order
                .iter()
                .position(|f| f.id == r.fixture.id)
                .map(|i| (i, r))
        })
        .collect();
    played.sort_by_key(|(i, _)| *i);
    for (index, record) in &played {
        let path = folder.join(match_file(*index));
        std::fs::write(&path, match_page(config, *index, record))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let index = folder.join("index.html");
    let board = leaderboard(config, records);
    std::fs::write(&index, index_page(config, records, &board, &played))
        .map_err(|e| format!("{}: {e}", index.display()))?;
    Ok(index)
}

fn match_file(index: usize) -> String {
    format!("match-{index:04}.html")
}

fn index_page(
    config: &ArenaConfig,
    records: &[MatchRecord],
    board: &Leaderboard,
    played: &[(usize, &MatchRecord)],
) -> String {
    let mut html = String::new();
    let _ = write!(
        html,
        "<h1>Legatum arena</h1><p class=\"lead\">{} of {} matches played. {}-player matches on a {} map{}.</p>",
        board.played,
        board.scheduled,
        config.schedule.players,
        format!("{:?}", config.schedule.map).to_lowercase(),
        config
            .schedule
            .turn_limit
            .map_or_else(String::new, |t| format!(", turn limit {t}"))
    );
    html.push_str("<h2>Standings</h2><div class=\"table\"><table><thead><tr><th>Rank</th><th>Entrant</th><th>Kind</th><th class=\"n\">Elo</th><th class=\"n\">Matches</th><th class=\"n\">Won</th><th class=\"n\">Drawn</th><th class=\"n\">Lost</th><th class=\"n\">Mean VP</th><th class=\"n\">Late</th><th class=\"n\">Errors</th><th class=\"n\">Refused</th></tr></thead><tbody>");
    for s in &board.standings {
        let _ = write!(
            html,
            "<tr><td>{}</td><td class=\"nw\"><span class=\"dot\" style=\"background:{}\"></span>{}</td><td class=\"nw\">{}</td><td class=\"n\"><b>{:.1}</b></td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{:.1}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
            s.rank,
            colour_of(config, &s.name),
            escape(&s.name),
            escape(&s.kind),
            s.elo,
            s.matches,
            s.wins,
            s.draws,
            s.losses,
            f64::from(s.mean_vp_tenths) / 10.0,
            s.timeouts,
            s.errors,
            s.rejected
        );
    }
    html.push_str("</tbody></table></div>");
    let history = rating_history(config, records);
    let series: Vec<(String, &str, Vec<f64>)> = config
        .entrants
        .iter()
        .map(|e| {
            (
                e.name.clone(),
                colour_of(config, &e.name),
                history.iter().map(|r| r.entrants[&e.name].elo).collect(),
            )
        })
        .collect();
    html.push_str("<h2>Rating over the schedule</h2>");
    html.push_str(&line_chart(&series, "Matches played", "Elo", false));
    html.push_str("<h2>Matches</h2><div class=\"table\"><table><thead><tr><th>#</th><th>Seed</th><th>Seats</th><th>Winner</th><th class=\"n\">Turns</th><th>End</th></tr></thead><tbody>");
    for (index, record) in played {
        let result = &record.report.result;
        let winner = result
            .winner
            .and_then(|team| {
                record
                    .report
                    .seats
                    .iter()
                    .position(|s| s.team == team)
                    .map(|seat| escape(&record.fixture.seats[seat]))
            })
            .unwrap_or_else(|| "none".into());
        let seats = record
            .fixture
            .seats
            .iter()
            .map(|n| escape(n))
            .collect::<Vec<_>>()
            .join(" &middot; ");
        let _ = write!(
            html,
            "<tr><td><a href=\"{}\">{}</a></td><td>{}</td><td>{}</td><td>{}</td><td class=\"n\">{}</td><td>{}</td></tr>",
            match_file(*index),
            index + 1,
            record.fixture.seed,
            seats,
            winner,
            result.turn,
            reason(record)
        );
    }
    html.push_str("</tbody></table></div>");
    page("Legatum arena", &html)
}

fn match_page(config: &ArenaConfig, index: usize, record: &MatchRecord) -> String {
    let report = &record.report;
    let result = &report.result;
    let mut html = String::new();
    let _ = write!(
        html,
        "<p><a href=\"index.html\">&larr; Arena</a></p><h1>Match {}</h1><p class=\"lead\">Seed {}. Turn {}: {}.</p>",
        index + 1,
        record.fixture.seed,
        result.turn,
        reason(record)
    );
    html.push_str("<div class=\"table\"><table><thead><tr><th>Seat</th><th>Entrant</th><th>Faction</th><th class=\"n\">VP</th><th class=\"n\">Answered</th><th class=\"n\">Late</th><th class=\"n\">Errors</th><th class=\"n\">Refused</th><th>Last error</th></tr></thead><tbody>");
    for (seat, s) in report.seats.iter().enumerate() {
        let vp = result
            .vp
            .iter()
            .find(|(p, _)| *p == s.player)
            .map_or(0, |(_, vp)| *vp);
        let won = result.winner == Some(s.team);
        let agent = s.agent.as_ref();
        let cell = |n: Option<u32>| n.map_or_else(|| "&ndash;".to_owned(), |n| n.to_string());
        let _ = write!(
            html,
            "<tr><td>{}</td><td><span class=\"dot\" style=\"background:{}\"></span>{}{}</td><td>{:?}</td><td class=\"n\"><b>{}</b></td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"err\">{}</td></tr>",
            seat,
            colour_of(config, &record.fixture.seats[seat]),
            escape(&record.fixture.seats[seat]),
            if won {
                " <span class=\"win\">won</span>"
            } else {
                ""
            },
            s.faction,
            vp,
            cell(agent.map(|a| a.answered)),
            cell(agent.map(|a| a.timeouts)),
            cell(agent.map(|a| a.errors)),
            cell(agent.map(|a| a.rejected)),
            escape(agent.and_then(|a| a.last_error.as_deref()).unwrap_or(""))
        );
    }
    html.push_str("</tbody></table></div><h2>Victory points by turn</h2>");
    let series: Vec<(String, &str, Vec<f64>)> = record
        .fixture
        .seats
        .iter()
        .enumerate()
        .map(|(seat, name)| {
            (
                name.clone(),
                colour_of(config, name),
                std::iter::once(0.0)
                    .chain(report.vp_by_turn.iter().map(|vp| f64::from(vp[seat])))
                    .collect(),
            )
        })
        .collect();
    html.push_str(&line_chart(&series, "Turn", "VP", true));
    let _ = write!(
        html,
        "<p class=\"note\">Save of the last turn: <a href=\"../{0}\"><code>{0}</code></a>. Open it in the game with <b>Load Game</b> to see the map; a match with two or more agent seats opens as an online game to host.</p>",
        escape(&record.save)
    );
    page(&format!("Legatum arena: match {}", index + 1), &html)
}

fn reason(record: &MatchRecord) -> &'static str {
    match record.report.result.reason {
        legatum_agent::protocol::EndReason::Victory => "victory",
        legatum_agent::protocol::EndReason::TurnLimit => "turn limit",
        legatum_agent::protocol::EndReason::Stopped => "stopped",
    }
}

fn colour_of<'a>(config: &ArenaConfig, name: &str) -> &'a str {
    let i = config
        .entrants
        .iter()
        .position(|e| e.name == name)
        .unwrap_or(0);
    COLOURS[i % COLOURS.len()]
}

/// An SVG line chart of `series` (name, colour, one value per step), with axes and a legend.
/// `whole` is for counts such as VP: ticks are whole numbers and lines move in steps.
fn line_chart(
    series: &[(String, &str, Vec<f64>)],
    x_label: &str,
    y_label: &str,
    whole: bool,
) -> String {
    const W: f64 = 760.0;
    const H: f64 = 300.0;
    const LEFT: f64 = 56.0;
    const RIGHT: f64 = 16.0;
    const TOP: f64 = 16.0;
    const BOTTOM: f64 = 40.0;
    let steps = series.iter().map(|s| s.2.len()).max().unwrap_or(0).max(2);
    let values = series.iter().flat_map(|s| s.2.iter().copied());
    let (mut low, mut high) =
        values.fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if low > high {
        (low, high) = (0.0, 1.0);
    }
    let mut tick = nice_step((high - low).max(1.0) / 5.0);
    if whole {
        tick = tick.max(1.0);
    }
    low = (low / tick).floor() * tick;
    high = ((high / tick).ceil() * tick).max(low + tick);
    let x = |i: usize| LEFT + (W - LEFT - RIGHT) * i as f64 / (steps - 1) as f64;
    let y = |v: f64| TOP + (H - TOP - BOTTOM) * (1.0 - (v - low) / (high - low));
    let mut svg = format!(
        "<figure><svg viewBox=\"0 0 {W} {H}\" role=\"img\" aria-label=\"{} by {}\">",
        escape(y_label),
        escape(&x_label.to_lowercase())
    );
    let mut v = low;
    while v <= high + tick / 2.0 {
        let _ = write!(
            svg,
            "<line class=\"grid\" x1=\"{LEFT}\" x2=\"{right}\" y1=\"{at:.1}\" y2=\"{at:.1}\"/><text class=\"tick\" x=\"{label}\" y=\"{text:.1}\" text-anchor=\"end\">{v}</text>",
            right = W - RIGHT,
            at = y(v),
            label = LEFT - 8.0,
            text = y(v) + 4.0,
        );
        v += tick;
    }
    let _ = write!(
        svg,
        "<text class=\"tick\" x=\"{LEFT}\" y=\"{}\">0</text><text class=\"tick\" x=\"{}\" y=\"{}\" text-anchor=\"end\">{}</text><text class=\"axis\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
        H - BOTTOM + 18.0,
        W - RIGHT,
        H - BOTTOM + 18.0,
        steps - 1,
        (LEFT + W - RIGHT) / 2.0,
        H - 6.0,
        escape(x_label)
    );
    for (_, colour, values) in series {
        let mut points = Vec::new();
        for (i, v) in values.iter().enumerate() {
            if whole && i > 0 {
                points.push(format!("{:.1},{:.1}", x(i), y(values[i - 1])));
            }
            points.push(format!("{:.1},{:.1}", x(i), y(*v)));
        }
        let _ = write!(
            svg,
            "<polyline fill=\"none\" stroke=\"{colour}\" stroke-width=\"2.5\" stroke-linejoin=\"round\" points=\"{}\"/>",
            points.join(" ")
        );
    }
    svg.push_str("</svg><figcaption>");
    for (name, colour, _) in series {
        let _ = write!(
            svg,
            "<span class=\"key\"><span class=\"dot\" style=\"background:{colour}\"></span>{}</span>",
            escape(name)
        );
    }
    svg.push_str("</figcaption></figure>");
    svg
}

/// 1, 2 or 5 times a power of ten, at least `raw`.
fn nice_step(raw: f64) -> f64 {
    let power = 10f64.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * power)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><style>{STYLE}</style></head><body><main>{body}</main></body></html>\n",
        escape(title)
    )
}

const STYLE: &str = "\
:root{--ink:#1d2433;--muted:#5f6878;--paper:#f6f3ec;--card:#fffdf8;--line:#e2dccf;--accent:#1f5f99}\
*{box-sizing:border-box}body{margin:0;background:var(--paper);color:var(--ink);font:15px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif}\
main{max-width:1040px;margin:0 auto;padding:32px 24px 56px}\
h1{font:600 30px/1.2 Georgia,'Times New Roman',serif;margin:0 0 6px}h2{font:600 20px/1.3 Georgia,serif;margin:32px 0 12px}\
.lead{color:var(--muted);margin:0 0 8px}a{color:var(--accent)}\
.table{overflow-x:auto;background:var(--card);border:1px solid var(--line);border-radius:10px}table{width:100%;border-collapse:collapse}\
th,td{padding:8px 12px;text-align:left;border-bottom:1px solid var(--line)}th{font-size:12px;letter-spacing:.04em;text-transform:uppercase;color:var(--muted);background:#f0ebe0}\
tr:last-child td{border-bottom:0}.nw{white-space:nowrap}.n{text-align:right;font-variant-numeric:tabular-nums}\
.dot{display:inline-block;width:10px;height:10px;border-radius:50%;margin-right:8px;vertical-align:baseline}\
.win{font-size:11px;background:#e3efe6;color:#2f7d4f;border-radius:4px;padding:1px 6px;margin-left:6px}.err{color:#9c3561;font-size:13px}\
figure{margin:0;background:var(--card);border:1px solid var(--line);border-radius:10px;padding:12px 12px 8px}svg{width:100%;height:auto;display:block}\
.grid{stroke:var(--line)}.tick{fill:var(--muted);font-size:12px}.axis{fill:var(--muted);font-size:12px}\
@media (max-width:1100px){th,td{padding:7px 8px}th{font-size:11px}}\
figcaption{display:flex;flex-wrap:wrap;gap:6px 18px;padding:6px 8px 2px;color:var(--muted);font-size:13px}.note{color:var(--muted);margin-top:16px}\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_escaped_for_html() {
        assert_eq!(
            escape(r#"<b>"Tom & Jerry's"</b>"#),
            "&lt;b&gt;&quot;Tom &amp; Jerry&#39;s&quot;&lt;/b&gt;"
        );
    }

    #[test]
    fn steps_are_round_numbers() {
        assert_eq!(nice_step(0.7), 1.0);
        assert_eq!(nice_step(13.0), 20.0);
        assert_eq!(nice_step(41.0), 50.0);
        assert_eq!(nice_step(120.0), 200.0);
    }

    #[test]
    fn a_chart_has_one_line_per_series_and_a_key() {
        let svg = line_chart(
            &[
                ("A".into(), "#000", vec![0.0, 1.0, 3.0]),
                ("B <x>".into(), "#111", vec![0.0, 2.0, 2.0]),
            ],
            "Turn",
            "VP",
            true,
        );
        assert_eq!(svg.matches("<polyline").count(), 2);
        assert!(!svg.contains(">0.5<"), "whole ticks only");
        assert!(svg.contains("B &lt;x&gt;"));
        assert!(!svg.contains("NaN"));
    }

    #[test]
    fn a_flat_or_empty_chart_still_draws() {
        let flat = line_chart(&[("A".into(), "#000", vec![5.0, 5.0])], "Turn", "VP", true);
        assert!(!flat.contains("NaN") && !flat.contains("inf"));
        let empty = line_chart(&[], "Matches", "Elo", false);
        assert!(empty.contains("<svg"));
    }
}
