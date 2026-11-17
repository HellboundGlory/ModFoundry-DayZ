use serde::Serialize;

use crate::target::Target;

use super::CaseResult;

const PREFIXES: [&str; 4] = ["decl", "body", "syn", "ord"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub agree: usize,
    pub total: usize,
}

impl Totals {
    fn add(&mut self, agree: bool) {
        self.total += 1;
        if agree {
            self.agree += 1;
        }
    }

    fn text(&self) -> String {
        format!("{}/{}", self.agree, self.total)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ScoreRow {
    pub id: String,
    pub agree: Option<bool>,
    pub expected: String,
    pub got: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub target: Target,
    pub rows: Vec<ScoreRow>,
    pub totals: Totals,
    pub by_prefix: Vec<(String, Totals)>,
}

impl Report {
    pub fn build(target: Target, cases: &[CaseResult]) -> Report {
        let mut totals = Totals::default();
        let mut by_prefix: Vec<(String, Totals)> = PREFIXES
            .iter()
            .map(|prefix| ((*prefix).to_string(), Totals::default()))
            .collect();
        let mut rows = Vec::with_capacity(cases.len());
        for case in cases {
            rows.push(ScoreRow {
                id: case.id.clone(),
                agree: case.expected.recorded.then_some(case.agree),
                expected: case.expected.expected_text(),
                got: match (&case.got, case.expected.recorded) {
                    (Some(got), _) => got.to_game_line(),
                    (None, true) => "nothing".to_string(),
                    (None, false) => "-".to_string(),
                },
            });
            if !case.expected.recorded {
                continue;
            }
            totals.add(case.agree);
            let prefix = super::prefix_of(&case.id);
            if let Some((_, totals)) = by_prefix.iter_mut().find(|(name, _)| name == prefix) {
                totals.add(case.agree);
            }
        }
        Report {
            target,
            rows,
            totals,
            by_prefix,
        }
    }

    pub fn to_table(&self) -> String {
        let width = self.rows.iter().map(|row| row.id.len()).max().unwrap_or(2);
        let mut lines = vec![format!(
            "{:<width$}  {:<10}  {:<58}  got",
            "id", "agree", "expected"
        )];
        lines.extend(self.rows.iter().map(|row| {
            let mark = match row.agree {
                Some(true) => "agree",
                Some(false) => "differ",
                None => "no verdict",
            };
            format!(
                "{:<width$}  {mark:<10}  {:<58}  {}",
                row.id, row.expected, row.got
            )
        }));
        lines.push(String::new());
        lines.push(format!("agree {}", self.totals.text()));
        lines.extend(
            self.by_prefix
                .iter()
                .map(|(prefix, totals)| format!("{prefix} {}", totals.text())),
        );
        lines.join("\n")
    }
}
