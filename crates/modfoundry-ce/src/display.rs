//! Text formatting for the record table: durations, percentages, name lists
//! and tier ranges.

/// Formats a duration in seconds using at most two units, `~`-prefixed when
/// truncated.
pub fn seconds(value: i64) -> String {
    if value == 0 {
        return "0".to_string();
    }
    const UNITS: [(u64, &str); 4] = [(86400, "d"), (3600, "h"), (60, "min"), (1, "s")];
    let mut rem = value.unsigned_abs();
    let mut idx = 0;
    while idx < UNITS.len() && rem / UNITS[idx].0 == 0 {
        idx += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for (size, label) in UNITS.iter().skip(idx).take(2) {
        let q = rem / size;
        rem %= size;
        if q > 0 {
            parts.push(format!("{q} {label}"));
        }
    }
    let mut out = parts.join(" ");
    if rem != 0 {
        out.insert(0, '~');
    }
    if value < 0 {
        out.insert(0, '-');
    }
    out
}

/// Formats a min/max pair as a percentage range, `—` when both are `-1`.
pub fn percent_pair(min: Option<i64>, max: Option<i64>) -> String {
    match (min, max) {
        (None, None) => String::new(),
        (Some(-1), Some(-1)) => "\u{2014}".to_string(),
        (Some(a), None) => format!("{a} %"),
        (None, Some(b)) => format!("{b} %"),
        (Some(a), Some(b)) if a == b => format!("{a} %"),
        (Some(a), Some(b)) => format!("{a}\u{2013}{b} %"),
    }
}

/// Joins names with a comma and a space.
pub fn name_list(names: &[String]) -> String {
    names.join(", ")
}

/// Collapses `Tier<N>` names into consecutive ranges, else [`name_list`].
pub fn tier_range(names: &[String]) -> String {
    let Some(mut numbers) = names
        .iter()
        .map(|n| tier_number(n))
        .collect::<Option<Vec<i64>>>()
    else {
        return name_list(names);
    };
    if numbers.is_empty() {
        return name_list(names);
    }
    numbers.sort_unstable();
    numbers.dedup();
    let mut groups = Vec::new();
    let (mut start, mut end) = (numbers[0], numbers[0]);
    for &n in &numbers[1..] {
        if n == end + 1 {
            end = n;
        } else {
            groups.push(group(start, end));
            start = n;
            end = n;
        }
    }
    groups.push(group(start, end));
    format!("Tier {}", groups.join(", "))
}

fn group(start: i64, end: i64) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}\u{2013}{end}")
    }
}

fn tier_number(name: &str) -> Option<i64> {
    if !name.get(..4)?.eq_ignore_ascii_case("tier") {
        return None;
    }
    let digits = &name[4..];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_table() {
        for (value, want) in [
            (14400, "4 h"),
            (1800, "30 min"),
            (5400, "1 h 30 min"),
            (45, "45 s"),
            (3888000, "45 d"),
            (129600, "1 d 12 h"),
            (0, "0"),
            (3600, "1 h"),
            (90, "1 min 30 s"),
            (3661, "~1 h 1 min"),
            (3601, "~1 h"),
            (-45, "-45 s"),
        ] {
            assert_eq!(seconds(value), want, "seconds({value})");
        }
    }

    #[test]
    fn percent_pair_table() {
        for (min, max, want) in [
            (Some(0), Some(40), "0\u{2013}40 %"),
            (Some(-1), Some(-1), "\u{2014}"),
            (Some(50), Some(50), "50 %"),
            (Some(40), None, "40 %"),
            (None, Some(40), "40 %"),
            (None, None, ""),
            (Some(-1), Some(40), "-1\u{2013}40 %"),
        ] {
            assert_eq!(
                percent_pair(min, max),
                want,
                "percent_pair({min:?}, {max:?})"
            );
        }
    }

    #[test]
    fn tier_range_table() {
        let v = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for (names, want) in [
            (v(&["Tier1", "Tier2"]), "Tier 1\u{2013}2"),
            (v(&["Tier2"]), "Tier 2"),
            (v(&["Tier1", "Tier3"]), "Tier 1, 3"),
            (v(&["Tier1", "Tier2", "Tier4"]), "Tier 1\u{2013}2, 4"),
            (v(&["tier1", "TIER2"]), "Tier 1\u{2013}2"),
            (v(&["Village", "Farm"]), "Village, Farm"),
            (v(&["Tier1", "Village"]), "Tier1, Village"),
            (v(&[]), ""),
        ] {
            assert_eq!(tier_range(&names), want, "tier_range({names:?})");
        }
    }
}
