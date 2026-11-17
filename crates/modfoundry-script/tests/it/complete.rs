//! Completion at a byte offset: receiver-typed member lists, the
//! no-inference fallback, bare-prefix scope listing, ranking and privacy.

use crate::common::{dependency_key, mission_key, vanilla_core_key, workspace_key};
use modfoundry_script::{complete_at, extract, CompletionKind, Index};

const VANILLA_SRC: &str = "\
class array<Class T> { proto T Get(int i); proto int Count(); }
class map<Class K, Class V> { proto V Get(K k); }
typedef array<string> TStringArray;
class Game { PlayerBase GetPlayer(); }
Game GetGame();
enum EMode { A, B }
class ManBase { void Base(); }
class PlayerBase extends ManBase {
void OnConnect() {}
int m_Health;
private int m_Secret;
}
";

const DEPENDENCY_SRC: &str = "modded class PlayerBase { override void OnConnect() {} }";

const WORKSPACE_SRC: &str =
    "modded class PlayerBase { override void OnConnect() {} void Mine() {} }";

fn build_index() -> Index {
    let mut index = Index::new();
    index.set_file(vanilla_core_key(), extract(VANILLA_SRC));
    index.set_file(dependency_key(), extract(DEPENDENCY_SRC));
    index.set_file(workspace_key(), extract(WORKSPACE_SRC));
    index
}

fn labels(items: &[modfoundry_script::CompletionItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

#[test]
fn member_completion_after_a_typed_param_lists_the_deduped_chain_and_base() {
    let index = build_index();
    let src = "class M { void Run(PlayerBase p) { p. } }";
    let offset = src.find("p.").unwrap() + 2;
    let c = complete_at(&index, src, offset);
    assert!(c.inferred);
    assert_eq!(c.replace.start, offset);
    assert_eq!(c.replace.end, offset);
    let ls = labels(&c.items);
    assert_eq!(
        ls.iter().filter(|l| **l == "OnConnect").count(),
        1,
        "{ls:?}"
    );
    assert!(ls.contains(&"Mine"), "{ls:?}");
    assert!(ls.contains(&"m_Health"), "{ls:?}");
    assert!(ls.contains(&"Base"), "{ls:?}");
    assert!(!ls.contains(&"m_Secret"), "{ls:?}");
}

#[test]
fn member_completion_filters_by_the_typed_prefix() {
    let index = build_index();
    let src = "class M { void Run(PlayerBase p) { p.On } }";
    let offset = src.find("p.On").unwrap() + 4;
    let c = complete_at(&index, src, offset);
    assert_eq!(c.items[0].label, "OnConnect");
    assert_eq!(c.items[0].kind, CompletionKind::Method);
    assert_eq!(c.items[0].detail, "override void OnConnect()");
    assert_eq!(c.replace.start, offset - 2);
    assert_eq!(c.replace.end, offset);
}

#[test]
fn static_enum_receiver_lists_exactly_its_members() {
    let index = build_index();
    let src = "class M { void Run() { EMode. } }";
    let offset = src.find("EMode.").unwrap() + "EMode.".len();
    let c = complete_at(&index, src, offset);
    assert_eq!(labels(&c.items), vec!["A", "B"]);
}

#[test]
fn an_auto_typed_receiver_falls_back_to_every_indexed_member() {
    let index = build_index();
    let src = "class M { void Run() { auto a = GetGame(); a. } }";
    let offset = src.rfind("a.").unwrap() + 2;
    let c = complete_at(&index, src, offset);
    assert!(!c.inferred);
    assert!(labels(&c.items).contains(&"OnConnect"));
}

#[test]
fn bare_prefix_ranks_the_case_sensitive_class_name_before_the_local() {
    let index = build_index();
    let src = "class M { void Run(PlayerBase p) { PlayerBase player; Pla } }";
    let offset = src.rfind("Pla").unwrap() + 3;
    let c = complete_at(&index, src, offset);
    let ls = labels(&c.items);
    let class_pos = ls
        .iter()
        .position(|l| *l == "PlayerBase")
        .expect("class present");
    let local_pos = ls
        .iter()
        .position(|l| *l == "player")
        .expect("local present");
    assert!(class_pos < local_pos, "{ls:?}");
}

#[test]
fn a_camel_hump_prefix_matches_a_class_chain_method() {
    let index = build_index();
    let src = "class Game { void Foo() { GP } }";
    let offset = src.find("GP").unwrap() + 2;
    let c = complete_at(&index, src, offset);
    assert_eq!(c.items.first().map(|i| i.label.as_str()), Some("GetPlayer"));
}

#[test]
fn a_comment_offers_no_items() {
    let index = build_index();
    for comment in ["// p.", "//! p.", "/** p. */"] {
        let src = format!("class M {{ void Run(PlayerBase p) {{ {comment}\nint x; }} }}");
        let offset = src.find("p.").unwrap() + 2;
        let c = complete_at(&index, &src, offset);
        assert!(c.items.is_empty(), "{comment}: {:?}", labels(&c.items));
    }
}

#[test]
fn a_string_literal_offers_no_items() {
    let index = build_index();
    for (src, after) in [
        ("class M { void Run() { string s = \"p.\"; } }", "p."),
        ("class M { void Run() { string s = \"\"; } }", "= \""),
        ("class M { void Run() { Print(\"\\n\"); } }", "\\n"),
    ] {
        let offset = src.find(after).unwrap() + after.len();
        let c = complete_at(&index, src, offset);
        assert!(c.items.is_empty(), "{src}: {:?}", labels(&c.items));
    }
}

#[test]
fn right_after_a_closed_string_or_block_comment_items_are_offered() {
    let index = build_index();
    for (src, after) in [
        ("class M { void Run() { string s = \"a\"; } }", "\"a\""),
        ("class M { void Run() { /* a */ } }", "*/"),
    ] {
        let offset = src.find(after).unwrap() + after.len();
        let c = complete_at(&index, src, offset);
        assert!(!c.items.is_empty(), "{src}");
    }
}

#[test]
fn an_unterminated_block_comment_offers_no_items() {
    let index = build_index();
    for (text, needle) in [
        ("/* note p.", "p."),
        ("/* note Pla", "Pla"),
        ("/** note p.", "p."),
    ] {
        let src = format!("class M {{ void Run(PlayerBase p) {{ {text}\n}}\n}}\n");
        let offset = src.rfind(needle).unwrap() + needle.len();
        let c = complete_at(&index, &src, offset);
        assert!(c.items.is_empty(), "{text}: {:?}", labels(&c.items));
    }
}

#[test]
fn an_unterminated_string_offers_no_items() {
    let index = build_index();
    for (text, needle) in [
        ("string s = \"a p.", "p."),
        ("string s = \"a Pla", "Pla"),
        ("string s = \"a\\\" p.", "p."),
    ] {
        let src = format!("class M {{ void Run(PlayerBase p) {{ {text}\n}}\n}}\n");
        let offset = src.rfind(needle).unwrap() + needle.len();
        let c = complete_at(&index, &src, offset);
        assert!(c.items.is_empty(), "{text}: {:?}", labels(&c.items));
    }
}

#[test]
fn an_unterminated_string_ends_at_its_line() {
    let index = build_index();
    let src = "class M { void Run(PlayerBase p) { string s = \"a\n Pla\n} }\n";
    let offset = src.rfind("Pla").unwrap() + 3;
    let c = complete_at(&index, src, offset);
    assert!(!c.items.is_empty());
}

#[test]
fn members_of_a_base_named_by_a_typedef_are_offered() {
    let mut index = Index::new();
    index.set_file(
        vanilla_core_key(),
        extract("class Real { void Foo(); }\ntypedef Real Alias;\nclass Derived : Alias { void Own(); }"),
    );
    let src = "class M { void Run(Derived d) { d. } }";
    let offset = src.find("d.").unwrap() + 2;
    let c = complete_at(&index, src, offset);
    assert!(labels(&c.items).contains(&"Foo"), "{:?}", labels(&c.items));
}

#[test]
fn a_backslash_at_the_end_of_a_line_continues_the_string() {
    let index = build_index();
    let src = "class M { void Run(PlayerBase p) { string s = \"a\\\n b\"; p.\n} }\n";
    let offset = src.rfind("p.").unwrap() + 2;
    let c = complete_at(&index, src, offset);
    assert!(!c.items.is_empty());
}

#[test]
fn slashes_and_a_closed_escape_inside_a_string_do_not_hide_the_code_after_it() {
    let index = build_index();
    for literal in ["string u = \"http://x\";", "string b = \"\\\\\";"] {
        let src = format!("class M {{ void Run(PlayerBase p) {{ {literal} p. }} }}");
        let offset = src.rfind("p.").unwrap() + 2;
        let c = complete_at(&index, &src, offset);
        assert!(c.inferred, "{literal}");
        assert!(labels(&c.items).contains(&"m_Health"), "{literal}");
    }
}

#[test]
fn a_private_field_is_offered_on_this_inside_its_own_class() {
    let index = build_index();
    let src = "modded class PlayerBase { void Run() { this. } }";
    let offset = src.find("this.").unwrap() + "this.".len();
    let c = complete_at(&index, src, offset);
    assert!(labels(&c.items).contains(&"m_Secret"));
}

#[test]
fn a_member_list_cut_at_the_cap_reports_truncation() {
    let mut src = String::from("class Big {\n");
    for i in 0..205 {
        src.push_str(&format!("\tvoid M{i}();\n"));
    }
    src.push_str("}\nclass M { void Run(Big b) { b. } }");
    let mut index = Index::new();
    index.set_file(mission_key(), extract(&src));

    let offset = src.find("b.").unwrap() + 2;
    let c = complete_at(&index, &src, offset);
    assert!(c.inferred);
    assert_eq!(c.items.len(), 200);
    assert!(c.truncated);

    let small = "class Small { void Only(); }\nclass M { void Run(Small s) { s. } }";
    let mut index = Index::new();
    index.set_file(mission_key(), extract(small));
    let offset = small.find("s.").unwrap() + 2;
    let c = complete_at(&index, small, offset);
    assert!(c.inferred);
    assert_eq!(c.items.len(), 1);
    assert!(!c.truncated);
}

#[test]
fn a_receivers_own_members_come_before_inherited_ones() {
    let mut index = Index::new();
    index.set_file(
        mission_key(),
        extract("class Base { void Aaa(); }\nclass Child extends Base { void Zzz(); }"),
    );
    let src = "class M { void Run(Child c) { c. } }";
    let offset = src.find("c.").unwrap() + 2;
    let c = complete_at(&index, src, offset);
    assert_eq!(labels(&c.items), ["Zzz", "Aaa"]);
}

#[test]
fn equally_good_classes_are_listed_alphabetically() {
    let names = ["Qd", "Qa", "QAb", "Qc", "Qaa", "Qb", "Qf", "Qe"];
    let defs: String = names.iter().map(|n| format!("class {n} {{}}\n")).collect();
    let mut index = Index::new();
    index.set_file(mission_key(), extract(&defs));
    let src = "class M { void Run() { Q } }";
    let offset = src.find('Q').unwrap() + 1;
    let c = complete_at(&index, src, offset);
    let q_labels: Vec<&str> = labels(&c.items)
        .into_iter()
        .filter(|l| l.starts_with('Q'))
        .collect();
    assert_eq!(q_labels, ["Qa", "Qaa", "QAb", "Qb", "Qc", "Qd", "Qe", "Qf"]);
}

#[test]
fn an_offset_past_the_end_or_inside_a_character_does_not_panic() {
    let index = build_index();
    let src = "class M { void Run() { é } }";
    let at_end = complete_at(&index, src, src.len());
    let past_end = complete_at(&index, src, src.len() + 3);
    assert_eq!(labels(&past_end.items), labels(&at_end.items));
    assert_eq!(past_end.replace, at_end.replace);
    let inside_char = src.find('é').unwrap() + 1;
    let inside = complete_at(&index, src, inside_char);
    assert!(inside.items.is_empty());
}

#[test]
fn reserved_words_are_offered_as_keywords() {
    let index = build_index();
    for (prefix, keyword) in [("seal", "sealed"), ("threa", "thread"), ("aut", "auto")] {
        let src = format!("class M {{ void Run() {{ {prefix} }} }}");
        let offset = src.rfind(prefix).unwrap() + prefix.len();
        let c = complete_at(&index, &src, offset);
        assert!(
            c.items
                .iter()
                .any(|i| i.label == keyword && i.kind == CompletionKind::Keyword),
            "{keyword} offered for {prefix}: {:?}",
            labels(&c.items)
        );
    }
}
