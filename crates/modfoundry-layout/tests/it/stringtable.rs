use modfoundry_layout::stringtable::parse_stringtable;

#[test]
fn reads_languages_in_header_order_and_rows_by_key() {
    let csv = "\"Language\",\"original\",\"english\"\n\"STR_hello\",\"Hallo\",\"Hello\"\n";
    let table = parse_stringtable(csv).expect("valid table");
    assert_eq!(table.languages(), ["original", "english"]);
    assert_eq!(table.lookup("#STR_hello", "english"), Some("Hello"));
}

#[test]
fn a_quoted_field_keeps_its_commas() {
    let csv = "\"Language\",\"english\"\n\"STR_a\",\"one, two\"\n";
    let table = parse_stringtable(csv).expect("valid table");
    assert_eq!(table.lookup("STR_a", "english"), Some("one, two"));
}

#[test]
fn a_doubled_quote_is_one_quote() {
    let csv = "\"Language\",\"english\"\n\"STR_a\",\"say \"\"hi\"\"\"\n";
    let table = parse_stringtable(csv).expect("valid table");
    assert_eq!(table.lookup("STR_a", "english"), Some("say \"hi\""));
}

#[test]
fn a_quoted_field_keeps_its_line_break() {
    let csv = "\"Language\",\"english\"\n\"STR_a\",\"line one\nline two\"\n";
    let table = parse_stringtable(csv).expect("valid table");
    assert_eq!(table.lookup("STR_a", "english"), Some("line one\nline two"));
}

#[test]
fn an_empty_language_cell_falls_back_to_english_then_original() {
    let csv = "\"Language\",\"original\",\"english\",\"german\"\n\
               \"STR_a\",\"Orig\",\"Eng\",\"\"\n\
               \"STR_b\",\"Orig\",\"\",\"\"\n\
               \"STR_c\",\"\",\"\",\"\"\n";
    let table = parse_stringtable(csv).expect("valid table");
    assert_eq!(table.lookup("#STR_a", "German"), Some("Eng"));
    assert_eq!(table.lookup("#STR_b", "german"), Some("Orig"));
    assert_eq!(table.lookup("#STR_c", "german"), None);
    assert_eq!(table.lookup("#STR_missing", "german"), None);
}

#[test]
fn short_records_pad_with_empty_cells_and_long_records_drop_extras() {
    let csv = "\"Language\",\"original\",\"english\",\"czech\"\n\
               \"STR_a\",\"Orig\"\n\
               \"STR_b\",\"Orig\",\"Eng\",\"Cz\",\"extra\"\n";
    let table = parse_stringtable(csv).expect("valid table");
    assert_eq!(table.lookup("STR_a", "czech"), Some("Orig"));
    assert_eq!(table.lookup("STR_b", "czech"), Some("Cz"));
    assert_eq!(table.lookup("STR_b", "english"), Some("Eng"));
}

#[test]
fn empty_lines_are_skipped_and_the_later_duplicate_key_wins() {
    let csv = "\"Language\",\"english\"\n\n\"STR_a\",\"First\"\n\n\"STR_a\",\"Second\"\n\n";
    let table = parse_stringtable(csv).expect("valid table");
    assert_eq!(table.len(), 1);
    assert_eq!(table.lookup("STR_a", "english"), Some("Second"));
}

#[test]
fn text_with_no_records_has_no_header_row() {
    for text in ["", "\u{feff}"] {
        let error = parse_stringtable(text).expect_err("no header");
        assert_eq!(error.to_string(), "record 1: no header row");
    }
}

#[test]
fn an_unterminated_quote_names_its_record() {
    let csv = "\"Language\",\"english\"\n\"STR_a\",\"open\n\"STR_b\",\"ok\"\n";
    let error = parse_stringtable(csv).expect_err("unterminated quote");
    assert_eq!(error.to_string(), "record 2: unterminated quote");
}

#[test]
fn merge_replaces_shared_keys_and_keeps_the_rest() {
    let mut game =
        parse_stringtable("\"Language\",\"english\"\n\"STR_a\",\"Old\"\n\"STR_b\",\"Keep\"\n")
            .expect("game table");
    let mod_table =
        parse_stringtable("\"Language\",\"english\"\n\"STR_a\",\"New\"\n").expect("mod table");
    game.merge(mod_table);
    assert_eq!(game.lookup("STR_a", "english"), Some("New"));
    assert_eq!(game.lookup("STR_b", "english"), Some("Keep"));
    assert_eq!(game.len(), 2);
}

#[test]
fn merge_matches_language_columns_by_name() {
    let mut game =
        parse_stringtable("\"Language\",\"original\",\"english\"\n\"STR_a\",\"Orig\",\"Eng\"\n")
            .expect("game table");
    let mod_table = parse_stringtable(
        "\"Language\",\"english\",\"german\"\n\"STR_a\",\"Mod Eng\",\"Mod De\"\n",
    )
    .expect("mod table");
    game.merge(mod_table);
    assert_eq!(game.languages(), ["original", "english", "german"]);
    assert_eq!(game.lookup("STR_a", "english"), Some("Mod Eng"));
    assert_eq!(game.lookup("STR_a", "german"), Some("Mod De"));
}

#[test]
fn a_byte_order_mark_and_crlf_line_ends_read_like_lf() {
    let csv =
        "\u{feff}\"Language\",\"english\"\r\n\"STR_a\",\"Line\\nNext\"\r\n\"STR_b\",\"Hi\"\r\n";
    let table = parse_stringtable(csv).expect("valid table");
    assert_eq!(table.languages(), ["english"]);
    assert_eq!(table.lookup("STR_a", "english"), Some("Line\\nNext"));
    assert_eq!(table.lookup("STR_b", "english"), Some("Hi"));
}

#[test]
#[ignore]
fn game_stringtable_reads_the_first_key_in_english() {
    let Some(path) = std::env::var_os("MF_STRINGTABLE") else {
        return;
    };
    let csv = std::fs::read_to_string(path).expect("stringtable.csv");
    let table = parse_stringtable(&csv).expect("game stringtable");
    let key = csv
        .lines()
        .nth(1)
        .and_then(|line| line.split(',').next())
        .expect("a first key")
        .trim_matches('"');
    let text = table.lookup(key, "english").expect("english text");
    assert!(!text.is_empty());
}

#[test]
fn a_merged_language_does_not_pick_up_extra_cells() {
    let mut game = parse_stringtable(
        "\"Language\",\"original\",\"english\"\n\"STR_a\",\"Orig\",\"Eng\",\"extra\"\n",
    )
    .expect("game table");
    let mod_table =
        parse_stringtable("\"Language\",\"english\",\"german\"\n\"STR_b\",\"Eng b\",\"De b\"\n")
            .expect("mod table");
    game.merge(mod_table);
    assert_eq!(game.lookup("STR_a", "german"), Some("Eng"));
}

#[test]
fn only_one_leading_hash_is_dropped_from_a_key() {
    let table =
        parse_stringtable("\"Language\",\"english\"\n\"STR_a\",\"Hi\"\n").expect("valid table");
    assert_eq!(table.lookup("##STR_a", "english"), None);
}
