//! Semantic lints MF-E3001 / MF-W3002.

use crate::common::key;
use modfoundry_core::text::line_col;
use modfoundry_enforce::{first_arms, parse_tree, syntax_diagnostics, Tree};
use modfoundry_pbo::read_pbo_file;
use modfoundry_script::{extract, semantic_diagnostics, FileKey, Index, Layer, Origin};
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

fn index_with_base() -> Index {
    let mut index = Index::new();
    index.set_file(
        key(
            "vanilla/4_World/playerbase.c",
            Origin::Vanilla,
            Layer::World,
        ),
        extract("class PlayerBase {}"),
    );
    index.set_file(
        key("vanilla/1_Core/array.c", Origin::Vanilla, Layer::Core),
        extract("class array<Class T> {}"),
    );
    index
}

fn codes(diagnostics: &[modfoundry_core::Diagnostic]) -> Vec<&str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

fn lint(index: &Index, file: &FileKey, source: &str) -> Vec<modfoundry_core::Diagnostic> {
    semantic_diagnostics(index, file, &parse_tree(source), source)
}

#[test]
fn a_lower_module_cannot_see_a_higher_class() {
    let index = index_with_base();
    let file = key("ws/3_Game/a.c", Origin::Workspace, Layer::Game);
    let source = "class A { PlayerBase p; }";
    let diagnostics = lint(&index, &file, source);
    assert_eq!(codes(&diagnostics), vec!["MF-E3001"], "{diagnostics:?}");
    let d = &diagnostics[0];
    assert_eq!(
        d.message,
        "class `PlayerBase` is defined in 4_World; 3_Game code cannot see it"
    );
    let span = d.span.expect("span");
    assert_eq!(&source[span.start..span.end], "PlayerBase");
}

#[test]
fn the_same_file_in_the_defining_layer_is_clean() {
    let index = index_with_base();
    let file = key("ws/4_World/a.c", Origin::Workspace, Layer::World);
    let diagnostics = lint(&index, &file, "class A { PlayerBase p; }");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn an_unresolvable_type_is_an_unknown_type_warning() {
    let index = index_with_base();
    let file = key("ws/3_Game/a.c", Origin::Workspace, Layer::Game);
    let source = "class A { Foo x; }";
    let diagnostics = lint(&index, &file, source);
    assert_eq!(codes(&diagnostics), vec!["MF-W3002"], "{diagnostics:?}");
    assert_eq!(diagnostics[0].message, "unknown type `Foo`");
    assert_eq!(diagnostics[0].severity, modfoundry_core::Severity::Warning);
    let span = diagnostics[0].span.expect("span");
    assert_eq!(&source[span.start..span.end], "Foo");
}

#[test]
fn an_endif_inside_a_syntax_error_still_ends_the_conditional_block() {
    let index = index_with_base();
    let file = key("ws/4_World/a.c", Origin::Workspace, Layer::World);
    let source = "#ifdef SERVER\nclass A ) {\n#endif\nclass B {}\nclass C { Foo x; }";
    let diagnostics = lint(&index, &file, source);
    assert_eq!(codes(&diagnostics), vec!["MF-W3002"], "{diagnostics:?}");
    let span = diagnostics[0].span.expect("span");
    assert_eq!(&source[span.start..span.end], "Foo");
}

#[test]
fn a_typedef_resolves_the_name() {
    let mut index = index_with_base();
    index.set_file(
        key("ws/3_Game/t.c", Origin::Workspace, Layer::Game),
        extract("typedef array<int> Foo;"),
    );
    let file = key("ws/3_Game/a.c", Origin::Workspace, Layer::Game);
    let diagnostics = lint(&index, &file, "class A { Foo x; }");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn a_generic_parameter_name_is_not_a_type_reference() {
    let index = index_with_base();
    let file = key("ws/3_Game/a.c", Origin::Workspace, Layer::Game);
    let diagnostics = lint(&index, &file, "class G<Class T> { T x; }");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn generic_arguments_are_checked() {
    let index = index_with_base();
    let file = key("ws/3_Game/a.c", Origin::Workspace, Layer::Game);
    let source = "class A { ref array<ref Missing> m; }";
    let diagnostics = lint(&index, &file, source);
    assert_eq!(codes(&diagnostics), vec!["MF-W3002"], "{diagnostics:?}");
    let span = diagnostics[0].span.expect("span");
    assert_eq!(&source[span.start..span.end], "Missing");
}

#[test]
fn new_cast_and_static_receiver_are_references() {
    let index = index_with_base();
    let file = key("ws/3_Game/a.c", Origin::Workspace, Layer::Game);
    let source = "class A { void Run() { PlayerBase p = new PlayerBase; auto c = (PlayerBase) p; PlayerBase.Static(); } }";
    let diagnostics = lint(&index, &file, source);
    assert_eq!(codes(&diagnostics), vec!["MF-E3001"; 4], "{diagnostics:?}");
}

#[test]
fn a_non_workspace_file_is_never_linted() {
    let index = index_with_base();
    let file = key("dep/3_Game/a.c", Origin::Dependency(0), Layer::Game);
    let diagnostics = lint(&index, &file, "class A { Foo x; }");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

struct CorpusFile {
    key: FileKey,
    text: String,
}

fn c_sources(path: &Path) -> Vec<(String, String)> {
    let (header, payloads) = read_pbo_file(path).expect("read pbo");
    header
        .entries
        .iter()
        .zip(&payloads)
        .filter_map(|(entry, data)| {
            let name = entry.name_str().replace('\\', "/");
            name.to_ascii_lowercase()
                .ends_with(".c")
                .then(|| (name, String::from_utf8_lossy(data).into_owned()))
        })
        .collect()
}

fn pbo_id(path: &Path, name: &str) -> String {
    format!("pbo:{}!{name}", path.to_string_lossy())
}

fn load_pbo(path: &Path, origin: Origin, index: &mut Index, files: &mut Vec<CorpusFile>) {
    for (name, text) in c_sources(path) {
        let key = FileKey {
            id: pbo_id(path, &name),
            origin,
            layer: Layer::from_path(&name),
        };
        index.set_file(key.clone(), extract(&text));
        files.push(CorpusFile { key, text });
    }
}

// Only anonymous keyword tokens: the named ones (`int`, `string`, `this`, `true`...) stay out because vanilla
// declares classes such as `class int` and `class string`.
fn grammar_keywords(tree: &Tree) -> HashSet<&'static str> {
    let language = tree.language();
    (0..language.node_kind_count() as u16)
        .filter_map(|id| {
            let kind = language.node_kind_for_id(id)?;
            (!language.node_kind_is_named(id)
                && language.node_kind_is_visible(id)
                && is_identifier_word(kind))
            .then_some(kind)
        })
        .collect()
}

fn is_identifier_word(kind: &str) -> bool {
    let mut chars = kind.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn keyword_identifiers(tree: &Tree, text: &str) -> Vec<(usize, usize, String)> {
    let keywords = grammar_keywords(tree);
    let mut out = Vec::new();
    collect_keyword_identifiers(tree.root_node(), text, &keywords, &mut out);
    out
}

fn collect_keyword_identifiers(
    node: tree_sitter::Node,
    text: &str,
    keywords: &HashSet<&'static str>,
    out: &mut Vec<(usize, usize, String)>,
) {
    if node.is_error() {
        return;
    }
    if node.kind() == "identifier" {
        let word = &text[node.start_byte()..node.end_byte()];
        if keywords.contains(word) {
            let (line, col) = line_col(text, node.start_byte());
            out.push((line, col, word.to_owned()));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_keyword_identifiers(child, text, keywords, out);
    }
}

#[test]
fn a_keyword_used_as_a_name_is_found() {
    let src = "class A\n{\n\tvoid F(int event)\n\t{\n\t\tint delete = 1;\n\t}\n}\n";
    assert_eq!(
        keyword_identifiers(&parse_tree(src), src),
        vec![(3, 13, "event".to_owned()), (5, 7, "delete".to_owned())]
    );
}

#[test]
fn real_keywords_comments_and_strings_are_not_hits() {
    let src = "class A\n{\n\tvoid F(int e)\n\t{\n\t\tint d = 1;\n\t\tstring s = \"else\"; // delete\n\t\tif (d) { F(1); } else { F(2); }\n\t}\n}\n";
    assert_eq!(keyword_identifiers(&parse_tree(src), src), Vec::new());
}

fn syntax_findings(id: &str, text: &str, errors: &mut Vec<String>) {
    let selected = first_arms(text);
    let tree = parse_tree(&selected);
    let findings = syntax_diagnostics(&tree, &selected);
    // Accepted, once each: BI's `ref ExplosiveEffectData>>` typo, and BI's `else` whose only
    // statement sits in the `#else` arm, so the first arm leaves it empty.
    let mut allowed: Vec<&str> = [
        ("3_Game/Global/AmmoTypes.c", "`>`"),
        ("4_World/Classes/ConstructionBase.c", "`else`"),
    ]
    .into_iter()
    .filter(|(file, _)| id.ends_with(file))
    .map(|(_, near)| near)
    .collect();
    for d in findings {
        if let Some(i) = allowed.iter().position(|near| d.message.contains(near)) {
            allowed.remove(i);
            continue;
        }
        let offset = d.span.map_or(0, |span| span.start);
        let (line, col) = line_col(&selected, offset);
        errors.push(format!("{id}:{line}:{col} {}", d.message));
    }
    for (line, col, word) in keyword_identifiers(&tree, &selected) {
        errors.push(format!(
            "{id}:{line}:{col} keyword `{word}` parsed as an identifier"
        ));
    }
}

fn syntax_check_pbo(path: &Path, errors: &mut Vec<String>) {
    for (name, text) in c_sources(path) {
        syntax_findings(&pbo_id(path, &name), &text, errors);
    }
}

#[test]
fn corpus_gate_has_no_false_positives() {
    let Ok(install) = env::var("MF_DAYZ_INSTALL") else {
        eprintln!("skipped: MF_DAYZ_INSTALL not set");
        return;
    };
    let mut index = Index::new();
    let mut files = Vec::new();
    load_pbo(
        &PathBuf::from(&install).join("dta/scripts.pbo"),
        Origin::Dependency(0),
        &mut index,
        &mut files,
    );
    let workshop = env::var("MF_WORKSHOP_PATH").ok().map(PathBuf::from);
    if let Some(workshop) = &workshop {
        let cf = workshop
            .join("1559212036")
            .join("addons")
            .join("scripts.pbo");
        if cf.is_file() {
            load_pbo(&cf, Origin::Dependency(1), &mut index, &mut files);
        } else {
            eprintln!("skipped CF: {} not found", cf.display());
        }
    }
    assert!(!files.is_empty(), "no corpus files loaded");

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for f in &files {
        syntax_findings(&f.key.id, &f.text, &mut errors);
        let mut key = f.key.clone();
        key.origin = Origin::Workspace;
        for d in lint(&index, &key, &f.text) {
            let entry = format!("{}:{}", key.id, d.message);
            match d.code.as_str() {
                "MF-E3001" | "MF-E3003" | "MF-E3004" => errors.push(entry),
                "MF-W3002" => warnings.push(entry),
                _ => {}
            }
        }
    }
    if let Some(workshop) = &workshop {
        let expansion = workshop
            .join("2291785308")
            .join("addons")
            .join("core_scripts.pbo");
        if expansion.is_file() {
            syntax_check_pbo(&expansion, &mut errors);
        } else {
            eprintln!("skipped Expansion Core: {} not found", expansion.display());
        }
    }
    if !warnings.is_empty() {
        println!("MF-W3002 findings: {}", warnings.len());
        for w in warnings.iter().take(30) {
            println!("{w}");
        }
    }
    println!(
        "keyword-as-identifier hits: {}",
        errors
            .iter()
            .filter(|e| e.contains("parsed as an identifier"))
            .count()
    );
    assert!(
        errors.is_empty(),
        "syntax and lint false positives: {errors:?}"
    );
    assert_eq!(warnings.len(), 0, "MF-W3002 false positives: {warnings:?}");
}

fn type_index() -> Index {
    let mut index = Index::new();
    index.set_file(
        key("vanilla/1_Core/set.c", Origin::Vanilla, Layer::Core),
        extract("class set {}"),
    );
    index.set_file(
        key("vanilla/5_Mission/later.c", Origin::Vanilla, Layer::Mission),
        extract("class Later {}"),
    );
    index
}

type Case<'a> = (&'a str, Origin, Option<Layer>, &'a str, Option<&'a str>);

#[test]
fn a_variable_named_like_a_visible_type_is_an_error() {
    let index = type_index();
    let cases: &[Case] = &[
        (
            "parameter",
            Origin::Workspace,
            Some(Layer::Mission),
            "void f(int set) {}",
            Some("set"),
        ),
        (
            "local",
            Origin::Workspace,
            Some(Layer::Mission),
            "void f() { int set; }",
            Some("set"),
        ),
        (
            "field",
            Origin::Workspace,
            Some(Layer::Mission),
            "class A { int set; }",
            Some("set"),
        ),
        (
            "foreach",
            Origin::Workspace,
            Some(Layer::Mission),
            "void f() { foreach (int set : items) {} }",
            Some("set"),
        ),
        (
            "second foreach variable",
            Origin::Workspace,
            Some(Layer::Mission),
            "void f() { foreach (string k, int set : items) {} }",
            Some("set"),
        ),
        (
            "foreach over a class type",
            Origin::Workspace,
            Some(Layer::Mission),
            "void f() { foreach (set s : items) {} }",
            None,
        ),
        (
            "different case",
            Origin::Workspace,
            Some(Layer::Mission),
            "class A { int Set; }",
            None,
        ),
        (
            "type defined later",
            Origin::Workspace,
            Some(Layer::Game),
            "class A { int Later; }",
            None,
        ),
        (
            "ifdef",
            Origin::Workspace,
            Some(Layer::Mission),
            "#ifdef SERVER\nclass A { int set; }\n#endif\n",
            None,
        ),
        (
            "misread else",
            Origin::Workspace,
            Some(Layer::Mission),
            "class A {\n    void f() {\n        if (a) { b(); }\n        // c\n        if (k()) set(0);\n        else set(1);\n    }\n}\n",
            None,
        ),
        (
            "method",
            Origin::Workspace,
            Some(Layer::Mission),
            "class A { void set() {} }",
            None,
        ),
        (
            "not a workspace file",
            Origin::Dependency(0),
            Some(Layer::Mission),
            "void f(int set) {}",
            None,
        ),
        (
            "no layer",
            Origin::Workspace,
            None,
            "void f(int set) {}",
            None,
        ),
    ];
    for &(label, origin, layer, source, expected) in cases {
        let file = FileKey {
            id: format!("ws/{label}.c"),
            origin,
            layer,
        };
        let diagnostics = lint(&index, &file, source);
        let Some(name) = expected else {
            assert!(diagnostics.is_empty(), "{label}: {diagnostics:?}");
            continue;
        };
        assert_eq!(
            codes(&diagnostics),
            ["MF-E3003"],
            "{label}: {diagnostics:?}"
        );
        let d = &diagnostics[0];
        assert_eq!(
            d.message,
            format!("variable name '{name}' already used as type name"),
            "{label}"
        );
        assert_eq!(d.severity, modfoundry_core::Severity::Error, "{label}");
        let span = d.span.expect("span");
        assert_eq!(&source[span.start..span.end], name, "{label}");
    }
}

fn call_index() -> Index {
    let mut index = Index::new();
    index.set_file(
        key("vanilla/4_World/building.c", Origin::Vanilla, Layer::World),
        extract("class Building {} class set<Class T> {}"),
    );
    index.set_file(
        key("vanilla/4_World/shop.c", Origin::Vanilla, Layer::World),
        extract(
            "class ShopBase { string Building() { return \"\"; } string ShopBase() { return \"\"; } }\
             class Shop extends ShopBase {}",
        ),
    );
    index
}

#[test]
fn a_bare_call_to_a_method_named_like_a_class_is_an_error() {
    let index = call_index();
    let cases: &[Case] = &[
        (
            "own method",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { string Building() { return \"\"; } void F() { string s = Building(); } }",
            Some("Building"),
        ),
        (
            "inherited method",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Shop extends ShopBase { void F() { string s = Building(); } }",
            Some("Building"),
        ),
        (
            "inherited method, child not indexed yet",
            Origin::Workspace,
            Some(Layer::Mission),
            "class NewShop extends ShopBase { void F() { string s = Building(); } }",
            Some("Building"),
        ),
        (
            "generic class",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { void set(int x) {} void F() { set(1); } }",
            None,
        ),
        (
            "this receiver",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { string Building() { return \"\"; } void F() { string s = this.Building(); } }",
            None,
        ),
        (
            "super receiver",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { string Building() { return \"\"; } void F() { string s = super.Building(); } }",
            None,
        ),
        (
            "static receiver",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { string Building() { return \"\"; } void F() { ShopBase.Building(); } }",
            None,
        ),
        (
            "new",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { string Building() { return \"\"; } void F() { auto b = new Building(); } }",
            None,
        ),
        (
            "declared but never called bare",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { string Building() { return \"\"; } }",
            None,
        ),
        (
            "no such method",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { void F() { Building(); } }",
            None,
        ),
        (
            "method name is no type",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Cart { string Paint() { return \"\"; } void F() { Paint(); } }",
            None,
        ),
        (
            "constructor",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Building { string Building() { return \"\"; } void F() { string s = Building(); } }",
            None,
        ),
        (
            "parent constructor",
            Origin::Workspace,
            Some(Layer::Mission),
            "class Shop extends ShopBase { void F() { ShopBase(); } }",
            None,
        ),
        (
            "modded class parent constructor",
            Origin::Workspace,
            Some(Layer::Mission),
            "modded class Shop { void F() { ShopBase(); } }",
            None,
        ),
        (
            "outside a class",
            Origin::Workspace,
            Some(Layer::Mission),
            "void F() { Building(); }",
            None,
        ),
    ];
    for &(label, origin, layer, source, expected) in cases {
        let file = FileKey {
            id: format!("ws/{label}.c"),
            origin,
            layer,
        };
        let diagnostics = lint(&index, &file, source);
        let Some(name) = expected else {
            assert!(diagnostics.is_empty(), "{label}: {diagnostics:?}");
            continue;
        };
        assert_eq!(
            codes(&diagnostics),
            ["MF-E3004"],
            "{label}: {diagnostics:?}"
        );
        let d = &diagnostics[0];
        assert_eq!(
            d.message,
            format!(
                "call '{name}()' runs the class {name}'s constructor, not the method {name}(); rename the method"
            ),
            "{label}"
        );
        assert_eq!(d.severity, modfoundry_core::Severity::Error, "{label}");
        let span = d.span.expect("span");
        assert_eq!(&source[span.start..span.end], name, "{label}");
    }
}

fn error_holds_identifier(node: tree_sitter::Node, text: &str, word: &str, inside: bool) -> bool {
    let inside = inside || node.is_error();
    if inside && node.kind() == "identifier" && &text[node.start_byte()..node.end_byte()] == word {
        return true;
    }
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|child| error_holds_identifier(child, text, word, inside));
    found
}

#[test]
fn a_keyword_name_inside_a_syntax_error_is_left_to_the_syntax_check() {
    // The first `delete` lands in an ERROR node, the second is the variable's name.
    let src = "class A\n{\n\tvoid F()\n\t{\n\t\tint delete delete = 1;\n\t}\n}\n";
    let tree = parse_tree(src);
    assert!(
        error_holds_identifier(tree.root_node(), src, "delete", false),
        "{}",
        tree.root_node().to_sexp()
    );
    assert_eq!(
        keyword_identifiers(&tree, src),
        vec![(5, 14, "delete".to_owned())]
    );
}
