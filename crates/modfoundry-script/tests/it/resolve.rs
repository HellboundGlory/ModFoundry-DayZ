use crate::common::{dependency_key, key, mission_key, workspace_key};
use modfoundry_script::{extract, resolve_at, FileKey, Index, Layer, Origin, SymbolKind};

const VANILLA_SRC: &str = "class ManBase { void Base(); }\n\
class PlayerBase extends ManBase {\n\
//! vanilla doc\n\
void OnConnect() {} int m_Health; }";

const DEPENDENCY_SRC: &str =
    "modded class PlayerBase { override void OnConnect() { super.OnConnect(); } }";

const WORKSPACE_SRC: &str = "modded class PlayerBase { \
override void OnConnect() { super.OnConnect(); } \
void Mine() { OnConnect(); Base(); m_Health = 1; } }";

const MISSION_SRC: &str = "class M { void Run(PlayerBase p) { PlayerBase q; p.OnConnect(); \
q.Mine(); int n; foreach (auto x : list) {} } }";

fn vanilla_key() -> FileKey {
    key(
        "vanilla/4_World/playerbase.c",
        Origin::Vanilla,
        Layer::World,
    )
}

fn build_index() -> Index {
    let mut index = Index::new();
    index.set_file(vanilla_key(), extract(VANILLA_SRC));
    index.set_file(dependency_key(), extract(DEPENDENCY_SRC));
    index.set_file(workspace_key(), extract(WORKSPACE_SRC));
    index.set_file(mission_key(), extract(MISSION_SRC));
    index
}

fn offset_of(src: &str, needle: &str) -> usize {
    src.find(needle).expect("needle not found in source")
}

#[test]
fn type_position_resolves_the_class_chain() {
    let index = build_index();
    let offset = offset_of(MISSION_SRC, "PlayerBase p");
    let res = resolve_at(&index, &mission_key(), MISSION_SRC, offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Class);
    assert_eq!(res.targets.len(), 3);
    assert_eq!(res.targets[0].file, "vanilla/4_World/playerbase.c");
    assert_eq!(res.targets[1].file, "dependency/4_World/playerbase.c");
    assert_eq!(res.targets[2].file, "workspace/4_World/playerbase.c");
    assert!(res.symbol.signature.contains("extends ManBase"));
    assert!(res.symbol.signature.contains("2 modded link"));
}

#[test]
fn method_call_through_a_typed_parameter_resolves_most_derived_first() {
    let index = build_index();
    let offset = offset_of(MISSION_SRC, "OnConnect()");
    let res = resolve_at(&index, &mission_key(), MISSION_SRC, offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Method);
    assert_eq!(res.targets.len(), 3);
    assert_eq!(res.targets[0].file, "workspace/4_World/playerbase.c");
    assert_eq!(res.targets[1].file, "dependency/4_World/playerbase.c");
    assert_eq!(res.targets[2].file, "vanilla/4_World/playerbase.c");
    assert_eq!(res.symbol.doc.as_deref(), Some("vanilla doc"));
}

#[test]
fn local_receiver_resolves_to_its_declaration() {
    let index = build_index();
    let decl_offset = offset_of(MISSION_SRC, "PlayerBase q");
    let decl_name_offset = decl_offset + "PlayerBase ".len();
    let use_offset = offset_of(MISSION_SRC, "q.Mine()");
    let res = resolve_at(&index, &mission_key(), MISSION_SRC, use_offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Local);
    assert_eq!(res.targets.len(), 1);
    assert_eq!(res.targets[0].file, mission_key().id);
    assert_eq!(res.targets[0].span.start, decl_name_offset);
}

#[test]
fn parameter_receiver_resolves_as_a_param() {
    let index = build_index();
    let offset = offset_of(MISSION_SRC, "p.OnConnect()");
    let res = resolve_at(&index, &mission_key(), MISSION_SRC, offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Param);
}

#[test]
fn unqualified_call_falls_back_to_the_base_class() {
    let index = build_index();
    let offset = offset_of(WORKSPACE_SRC, "Base()");
    let res = resolve_at(&index, &workspace_key(), WORKSPACE_SRC, offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Method);
    assert_eq!(res.targets.len(), 1);
    assert!(
        res.targets[0].file.contains("manbase")
            || res.symbol.container.as_deref() == Some("ManBase")
    );
}

#[test]
fn unqualified_field_use_resolves_to_the_vanilla_field() {
    let index = build_index();
    let offset = offset_of(WORKSPACE_SRC, "m_Health = 1");
    let res = resolve_at(&index, &workspace_key(), WORKSPACE_SRC, offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Field);
    assert_eq!(res.targets.len(), 1);
    assert_eq!(res.targets[0].file, "vanilla/4_World/playerbase.c");
}

#[test]
fn super_call_in_a_modded_link_skips_its_own_link() {
    let index = build_index();
    let offset = offset_of(WORKSPACE_SRC, "super.OnConnect()") + "super.".len();
    let res = resolve_at(&index, &workspace_key(), WORKSPACE_SRC, offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Method);
    assert_eq!(res.targets.len(), 2);
    assert_eq!(res.targets[0].file, "dependency/4_World/playerbase.c");
    assert_eq!(res.targets[1].file, "vanilla/4_World/playerbase.c");
}

#[test]
fn re_set_file_updates_the_chain_used_by_resolve() {
    let mut index = build_index();
    index.set_file(dependency_key(), extract(""));
    let offset = offset_of(MISSION_SRC, "PlayerBase p");
    let res = resolve_at(&index, &mission_key(), MISSION_SRC, offset).expect("resolves");
    assert_eq!(res.targets.len(), 2);
}

#[test]
fn offset_in_a_string_literal_returns_none() {
    let index = build_index();
    let src = "class X { void M() { string s = \"hello\"; } }";
    let offset = offset_of(src, "hello");
    assert!(resolve_at(&index, &mission_key(), src, offset).is_none());
}

#[test]
fn offset_in_a_comment_returns_none() {
    let index = build_index();
    let src = "// a comment about PlayerBase\nclass X {}";
    let offset = offset_of(src, "comment");
    assert!(resolve_at(&index, &mission_key(), src, offset).is_none());
}

#[test]
fn a_chained_call_receiver_resolves_through_its_return_type() {
    let mut index = build_index();
    index.set_file(
        key("vanilla/1_Core/game.c", Origin::Vanilla, Layer::Core),
        extract("class Game { PlayerBase GetPlayer(); }\nGame GetGame();"),
    );
    let src = "class Z { void Run() { GetGame().GetPlayer(); } }";
    let offset = offset_of(src, "GetPlayer");
    let res = resolve_at(&index, &mission_key(), src, offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Method);
    assert_eq!(res.symbol.container.as_deref(), Some("Game"));
}

#[test]
fn offset_on_the_class_keyword_returns_none() {
    let index = build_index();
    let src = "class X {}";
    let offset = offset_of(src, "class");
    assert!(resolve_at(&index, &mission_key(), src, offset).is_none());
}

#[test]
fn super_falls_back_to_the_base_when_the_file_is_not_in_the_chain() {
    let mut index = build_index();
    let src = "class Kid extends PlayerBase { void f() { super.OnConnect(); } }";
    index.set_file(
        key("workspace/4_World/kid.c", Origin::Workspace, Layer::World),
        extract(src),
    );
    let moved = key(
        "workspace/4_World/kid_moved.c",
        Origin::Workspace,
        Layer::World,
    );
    let offset = offset_of(src, "super.OnConnect()") + "super.".len();
    let res = resolve_at(&index, &moved, src, offset).expect("resolves");
    assert_eq!(res.symbol.kind, SymbolKind::Method);
    assert_eq!(res.symbol.container.as_deref(), Some("PlayerBase"));
}

#[test]
fn hover_on_a_class_without_a_base_names_no_base() {
    let index = build_index();
    let src = "class N { void Run(ManBase b) {} }";
    let offset = offset_of(src, "ManBase b");
    let res = resolve_at(&index, &mission_key(), src, offset).expect("resolves");
    assert_eq!(res.symbol.signature, "class ManBase");
}

#[test]
fn super_in_a_modded_class_outside_the_chain_stays_on_the_class() {
    let index = build_index();
    let src = "modded class PlayerBase { void OnConnect() { super.OnConnect(); } }";
    let moved = key(
        "workspace/4_World/new_link.c",
        Origin::Workspace,
        Layer::World,
    );
    let offset = offset_of(src, "super.OnConnect()") + "super.".len();
    let res = resolve_at(&index, &moved, src, offset).expect("resolves");
    assert_eq!(res.symbol.container.as_deref(), Some("PlayerBase"));
}
