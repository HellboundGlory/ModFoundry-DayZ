use std::collections::BTreeMap;

use modfoundry_core::{Diagnostic, Severity};
use modfoundry_layout::check::{
    check, Reference, Resolve, BAD_VALUE, DUPLICATE_NAME, UNKNOWN_CLASS, UNKNOWN_PROPERTY,
    UNPARSED, UNRESOLVED,
};
use modfoundry_layout::vocab::{Shape, Vocabulary};

struct Known(Vec<Reference>);

impl Resolve for Known {
    fn resolves(&self, reference: &Reference) -> bool {
        self.0.contains(reference)
    }
}

fn shape(patterns: &[&str], words: Option<&[&str]>) -> Shape {
    Shape {
        patterns: patterns.iter().map(|p| p.to_string()).collect(),
        words: words.map(|w| w.iter().map(|w| w.to_string()).collect()),
    }
}

fn vocabulary() -> Vocabulary {
    let mut vocabulary = Vocabulary::default();
    vocabulary.classes.insert(
        "FrameWidgetClass".into(),
        BTreeMap::from([
            ("position".into(), shape(&["nn"], None)),
            ("style".into(), shape(&["w"], None)),
            ("color".into(), shape(&["nnnn"], None)),
            ("imageTexture".into(), shape(&["s"], None)),
            ("image0".into(), shape(&["s"], None)),
        ]),
    );
    vocabulary.classes.insert(
        "TextWidgetClass".into(),
        BTreeMap::from([
            ("position".into(), shape(&["nn"], None)),
            ("text".into(), shape(&["s"], None)),
            ("valign".into(), shape(&["w"], Some(&["top", "center"]))),
            ("font".into(), shape(&["s"], None)),
        ]),
    );
    vocabulary
        .script_widgets
        .insert("RenderTargetWidgetClass".into());
    vocabulary
}

#[test]
fn clean_layout_has_no_diagnostics() {
    let text = r#"FrameWidgetClass root {
 position 0 0
 style DayZDefaultPanel
 {
  TextWidgetClass label {
   position 0.1 0.2
   text "Hi"
   valign center
  }
 }
}
"#;
    let resolve = Known(vec![Reference::Style("DayZDefaultPanel".into())]);
    assert!(check(text, &vocabulary(), &resolve).is_empty());
}

fn line_of(text: &str, diagnostic: &Diagnostic) -> usize {
    let span = diagnostic.span.expect("every finding has a span");
    text[..span.start].matches('\n').count() + 1
}

#[test]
fn reports_each_finding_with_its_code_severity_line_and_message() {
    let cases = [
        ("FrameWidgetClass root {\n position 0 0\n", UNPARSED, Severity::Error, 1, "closed"),
        (
            "TextWidgetClas label {\n}\n",
            UNKNOWN_CLASS,
            Severity::Error,
            1,
            "did you mean `TextWidgetClass`",
        ),
        (
            "FrameWidgetClass root {\n {\n  Bogus x {\n  }\n }\n}\n",
            UNKNOWN_CLASS,
            Severity::Error,
            3,
            "unknown widget class `Bogus`",
        ),
        (
            "TextWidgetClass label {\n color 1 0 0 1\n}\n",
            UNKNOWN_PROPERTY,
            Severity::Warning,
            2,
            "not used on `TextWidgetClass`",
        ),
        (
            "TextWidgetClass label {\n txt \"Hi\"\n}\n",
            UNKNOWN_PROPERTY,
            Severity::Error,
            2,
            "did you mean `text`",
        ),
        (
            "TextWidgetClass label {\n position 0.5 center\n}\n",
            BAD_VALUE,
            Severity::Warning,
            2,
            "expects value shape `nn`",
        ),
        (
            "TextWidgetClass label {\n valign bottom\n}\n",
            BAD_VALUE,
            Severity::Warning,
            2,
            "`bottom` is not a value `valign`",
        ),
        (
            "FrameWidgetClass root {\n imageTexture \"{5A89D58DD2276E85}Gui/textures/Missing.edds\"\n}\n",
            UNRESOLVED,
            Severity::Error,
            2,
            "texture `Gui/textures/Missing.edds`",
        ),
        (
            "FrameWidgetClass root {\n image0 \"set:dayz_gui image:iconMissing\"\n}\n",
            UNRESOLVED,
            Severity::Error,
            2,
            "image `iconMissing` was not found in set `dayz_gui`",
        ),
        (
            "TextWidgetClass label {\n font \"gui/fonts/Missing\"\n}\n",
            UNRESOLVED,
            Severity::Error,
            2,
            "font `gui/fonts/Missing`",
        ),
        (
            "FrameWidgetClass root {\n style Missing\n}\n",
            UNRESOLVED,
            Severity::Error,
            2,
            "style `Missing`",
        ),
        (
            "RenderTargetWidgetClass target {\n texture 1\n}\n",
            UNKNOWN_PROPERTY,
            Severity::Error,
            2,
            "script widget `RenderTargetWidgetClass`",
        ),
    ];
    for (text, code, severity, line, word) in cases {
        let found = check(text, &vocabulary(), &Known(vec![]));
        let [hit] = found.as_slice() else {
            panic!("expected only {code} for {text:?}: {found:?}");
        };
        assert_eq!(hit.code, code);
        assert_eq!(hit.severity, severity, "{code}");
        assert_eq!(line_of(text, hit), line, "{code}");
        assert!(hit.message.contains(word), "{code}: {}", hit.message);
    }
}

#[test]
fn every_bad_word_in_a_property_is_reported() {
    let mut vocab = Vocabulary::default();
    vocab.classes.insert(
        "FrameWidgetClass".into(),
        BTreeMap::from([(
            "tag".into(),
            shape(&["w", "ww", "www", "wwww"], Some(&["a", "b"])),
        )]),
    );
    let text = "FrameWidgetClass root {\n tag x y a x\n}\n";
    let found: Vec<String> = check(text, &vocab, &Known(vec![]))
        .into_iter()
        .filter(|d| d.code == BAD_VALUE)
        .map(|d| d.message)
        .collect();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("`x`"), "{}", found[0]);
    assert!(found[1].contains("`y`"), "{}", found[1]);

    let mismatched = "FrameWidgetClass root {\n tag x 1\n}\n";
    let found: Vec<_> = check(mismatched, &vocab, &Known(vec![]))
        .into_iter()
        .filter(|d| d.code == BAD_VALUE)
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].message.contains("expects value shape"),
        "{}",
        found[0].message
    );
}

#[test]
fn engine_defaults_are_accepted_only_for_their_own_property() {
    let mut vocab = Vocabulary::default();
    vocab.classes.insert(
        "TextWidgetClass".into(),
        BTreeMap::from([
            ("halign".into(), shape(&["w"], Some(&["left", "center"]))),
            ("valign".into(), shape(&["w"], Some(&["top", "center"]))),
        ]),
    );
    let cases = [
        ("valign top_ref", 0),
        ("halign left_ref", 0),
        ("valign left_ref", 1),
        ("halign top_ref", 1),
        ("valign bottom_ref", 1),
        ("valign top_ref top_ref", 1),
    ];
    for (line, expected) in cases {
        let text = format!("TextWidgetClass label {{\n {line}\n}}\n");
        let found: Vec<_> = check(&text, &vocab, &Known(vec![]))
            .into_iter()
            .filter(|d| d.code == BAD_VALUE)
            .collect();
        assert_eq!(found.len(), expected, "{line}: {found:?}");
    }
}

#[test]
fn an_unresolved_reference_is_reported_once_per_kind() {
    let mut vocab = Vocabulary::default();
    vocab.classes.insert(
        "FrameWidgetClass".into(),
        BTreeMap::from([
            ("imageTexture".into(), shape(&["ss"], None)),
            ("style".into(), shape(&["ww"], None)),
        ]),
    );
    vocab.classes.insert(
        "TextWidgetClass".into(),
        BTreeMap::from([("font".into(), shape(&["s"], None))]),
    );
    let cases = [
        (
            "FrameWidgetClass root {\n imageTexture \"{5A89D58DD2276E85}Gui/textures/One.edds\" \"{5A89D58DD2276E86}Gui/textures/Two.edds\"\n}\n",
            1,
        ),
        (
            "TextWidgetClass label {\n font \"{5A89D58DD2276E85}gui/f\"\n}\n",
            2,
        ),
        (
            "FrameWidgetClass root {\n style One Two\n}\n",
            1,
        ),
    ];
    for (text, expected) in cases {
        let found: Vec<_> = check(text, &vocab, &Known(vec![]))
            .into_iter()
            .filter(|d| d.code == UNRESOLVED)
            .collect();
        assert_eq!(found.len(), expected, "{text:?}: {found:?}");
    }
}

#[test]
fn repeated_sibling_name_points_at_the_first_and_skips_other_parents() {
    let text = "FrameWidgetClass a {\n {\n  TextWidgetClass label {\n  }\n  TextWidgetClass label {\n  }\n }\n}\nFrameWidgetClass b {\n {\n  TextWidgetClass label {\n  }\n }\n}\n";
    let found: Vec<_> = check(text, &vocabulary(), &Known(vec![]))
        .into_iter()
        .filter(|d| d.code == DUPLICATE_NAME)
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(line_of(text, &found[0]), 5);
    assert!(found[0].message.contains("line 3"), "{}", found[0].message);
}

#[test]
fn span_counts_columns_in_characters_and_ends_before_the_line_break() {
    let text = "FrameWidgetClass root {\r\n TextWidgetClass \"é\" { x }\r\n}\r\n";
    let found = check(text, &vocabulary(), &Known(vec![]));
    let hit = found
        .iter()
        .find(|d| d.code == UNPARSED)
        .unwrap_or_else(|| panic!("{found:?}"));
    let span = hit.span.expect("span");
    assert_eq!(&text[span.start..span.end], "x }");
    assert_eq!(line_of(text, hit), 2);
}

#[test]
fn empty_string_values_are_not_references() {
    let text = "TextWidgetClass label {\n font \"\"\n}\n";
    let found = check(text, &vocabulary(), &Known(vec![]));
    assert!(found.iter().all(|d| d.code != UNRESOLVED), "{found:?}");
}

#[test]
fn span_offsets_skip_a_byte_order_mark_and_run_to_a_final_line_without_newline() {
    let text = "\u{feff}TextWidgetClass label {\n color 1 0 0 1\n}";
    let found = check(text, &vocabulary(), &Known(vec![]));
    let hit = found
        .iter()
        .find(|d| d.code == UNKNOWN_PROPERTY)
        .unwrap_or_else(|| panic!("{found:?}"));
    let span = hit.span.expect("span");
    assert_eq!(&text[span.start..span.end], "color 1 0 0 1");
}

#[test]
fn empty_names_and_empty_reference_parts_are_not_findings() {
    let text = "FrameWidgetClass root {\n imageTexture \"{5A89D58DD2276E85}\"\n image0 \"set: image:\"\n {\n  TextWidgetClass \"\" {\n  }\n  TextWidgetClass \"\" {\n  }\n }\n}\n";
    let found = check(text, &vocabulary(), &Known(vec![]));
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_style_the_mod_defines_is_not_judged_against_the_game_style_words() {
    let mut vocab = Vocabulary::default();
    vocab.classes.insert(
        "ButtonWidgetClass".into(),
        BTreeMap::from([("style".into(), shape(&["w"], Some(&["DayZDefaultButton"])))]),
    );
    let text = "ButtonWidgetClass b {\n style SampleMod_Bare\n}\n";
    let resolve = Known(vec![Reference::Style("SampleMod_Bare".into())]);
    assert_eq!(check(text, &vocab, &resolve), vec![]);
}

#[test]
fn a_misspelled_style_is_still_an_unresolved_reference_not_a_bad_value() {
    let mut vocab = Vocabulary::default();
    vocab.classes.insert(
        "ButtonWidgetClass".into(),
        BTreeMap::from([("style".into(), shape(&["w"], Some(&["DayZDefaultButton"])))]),
    );
    let text = "ButtonWidgetClass b {\n style DayZDefaultButon\n}\n";
    let codes: Vec<_> = check(text, &vocab, &Known(vec![]))
        .into_iter()
        .map(|d| d.code)
        .collect();
    assert_eq!(codes, [UNRESOLVED]);
}
