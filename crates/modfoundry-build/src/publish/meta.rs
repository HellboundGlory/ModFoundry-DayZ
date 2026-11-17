use std::time::Duration;

const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;

/// The `timestamp` value Publisher writes: `DateTime.ToBinary()` of a UTC time.
pub fn dotnet_utc_binary(since_unix: Duration) -> i64 {
    let ticks = (since_unix.as_nanos() / 100) as i64 + UNIX_EPOCH_TICKS;
    ticks | (1 << 62)
}

/// The Workshop id a `meta.cpp` names; `None` when absent, unparsable or `0` (not published yet).
pub fn published_id(meta_cpp: &str) -> Option<u64> {
    let value = meta_cpp
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("publishedid"))?
        .strip_prefix("publishedid")?
        .trim_start()
        .strip_prefix('=')?
        .trim_start()
        .strip_suffix(';')?
        .trim_end();
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let id = value.parse::<u64>().ok()?;
    (id != 0).then_some(id)
}

/// The text of a `meta.cpp`, with CRLF line endings as Publisher writes it.
pub fn render_meta_cpp(published_id: u64, name: &str, timestamp: i64) -> String {
    let name = name.replace('"', "\\\"");
    format!(
        "protocol = 1;\r\npublishedid = {published_id};\r\nname = \"{name}\";\r\ntimestamp = {timestamp};\r\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotnet_utc_binary_matches_publisher_timestamp() {
        assert_eq!(
            dotnet_utc_binary(Duration::from_nanos(1_791_822_159_317_209_600)),
            5_250_960_208_020_560_000
        );
    }

    #[test]
    fn render_meta_cpp_matches_publisher_file() {
        assert_eq!(
            render_meta_cpp(1559212036, "CF", 5250960208020560000),
            "protocol = 1;\r\npublishedid = 1559212036;\r\nname = \"CF\";\r\ntimestamp = 5250960208020560000;\r\n"
        );
    }

    #[test]
    fn render_meta_cpp_escapes_quotes_in_name() {
        assert!(render_meta_cpp(7, "Say \"hi\"", 1).contains("name = \"Say \\\"hi\\\"\";\r\n"));
    }

    #[test]
    fn published_id_reads_the_publishedid_line() {
        let cases = [
            (
                "protocol = 1;\r\npublishedid = 1559212036;\r\nname = \"CF\";\r\ntimestamp = 5250960208020560000;\r\n",
                Some(1559212036),
            ),
            ("publishedid = +5;\n", None),
            ("publishedid = 0;\r\n", None),
            ("protocol = 1;\n", None),
            ("  publishedid=42 ;\n", Some(42)),
        ];
        for (text, expected) in cases {
            assert_eq!(published_id(text), expected, "{text:?}");
        }
    }
}
