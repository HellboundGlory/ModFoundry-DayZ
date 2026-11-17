use modfoundry_platform::steam::KeyValueTag;
use std::collections::BTreeSet;

/// Every key-value tag key an upload replaces.
pub const KNOWN_KEYS: [&str; 12] = [
    "bis_platform",
    "bis_signature",
    "bis_shortHash",
    "bis_longHash",
    "bis_displayName",
    "bis_size",
    "bis_version",
    "bis_website",
    "facebook",
    "youtube",
    "twitter",
    "reddit",
];

/// What a build folder tells us about the mod.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModFacts {
    /// Base64, `bis_longHash`.
    pub long_hash: String,
    /// Base64, `bis_shortHash`.
    pub short_hash: String,
    pub signers: BTreeSet<String>,
    pub win32: bool,
    pub win64: bool,
    pub display_name: Option<String>,
    pub size: u64,
}

/// The key-value tags an upload sends for `facts`, then the non-empty `info` pairs.
pub fn key_value_tags(facts: &ModFacts, info: &[(&str, &str)]) -> Vec<KeyValueTag> {
    let mut platforms = Vec::new();
    if facts.win32 {
        platforms.push("win32");
    }
    if facts.win64 {
        platforms.push("win64");
    }
    if platforms.is_empty() {
        platforms.push("-");
    }

    let mut tags: Vec<KeyValueTag> = platforms
        .into_iter()
        .map(|platform| tag("bis_platform", platform))
        .collect();
    tags.extend(
        facts
            .signers
            .iter()
            .map(|signer| tag("bis_signature", signer)),
    );
    tags.push(tag("bis_shortHash", &facts.short_hash));
    tags.push(tag("bis_longHash", &facts.long_hash));
    if let Some(name) = facts
        .display_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
    {
        tags.push(tag("bis_displayName", name));
    }
    tags.push(tag("bis_size", &facts.size.to_string()));
    tags.extend(
        info.iter()
            .filter(|(_, value)| !value.trim().is_empty())
            .map(|(key, value)| tag(key, value)),
    );
    tags
}

fn tag(key: &str, value: &str) -> KeyValueTag {
    KeyValueTag {
        key: key.to_owned(),
        value: value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG_HASH: &str = "ipzZRHTSXAN4KCpW8Ui65MBpvv0=";
    const SHORT_HASH: &str = "MfifOA==";

    #[test]
    fn plain_mod_sends_platform_signature_hashes_and_size() {
        let facts = ModFacts {
            long_hash: LONG_HASH.to_owned(),
            short_hash: SHORT_HASH.to_owned(),
            signers: BTreeSet::from(["IceBlade".to_owned()]),
            size: 12345,
            ..ModFacts::default()
        };
        assert_eq!(
            key_value_tags(&facts, &[]),
            vec![
                tag("bis_platform", "-"),
                tag("bis_signature", "IceBlade"),
                tag("bis_shortHash", SHORT_HASH),
                tag("bis_longHash", LONG_HASH),
                tag("bis_size", "12345"),
            ]
        );
    }

    #[test]
    fn every_optional_tag_is_sent_and_blank_info_is_skipped() {
        let facts = ModFacts {
            long_hash: LONG_HASH.to_owned(),
            short_hash: SHORT_HASH.to_owned(),
            signers: ["b", "a"].map(str::to_owned).into_iter().collect(),
            win64: true,
            display_name: Some("Gardening Plus".to_owned()),
            size: 12345,
            ..ModFacts::default()
        };
        let info = [
            ("bis_version", "1.2"),
            ("facebook", " "),
            ("reddit", "r/dayz"),
        ];
        assert_eq!(
            key_value_tags(&facts, &info),
            vec![
                tag("bis_platform", "win64"),
                tag("bis_signature", "a"),
                tag("bis_signature", "b"),
                tag("bis_shortHash", SHORT_HASH),
                tag("bis_longHash", LONG_HASH),
                tag("bis_displayName", "Gardening Plus"),
                tag("bis_size", "12345"),
                tag("bis_version", "1.2"),
                tag("reddit", "r/dayz"),
            ]
        );
    }
}
