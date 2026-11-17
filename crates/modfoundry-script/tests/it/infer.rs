//! Receiver type inference: textual scan-back at a `.`, declared
//! types, generic substitution and typedef expansion over the index.

use crate::common::{dependency_key, key, mission_key, vanilla_core_key, workspace_key};
use modfoundry_script::{extract, infer_receiver, Index, Layer, Origin};

const VANILLA_SRC: &str = "\
class array<Class T> { proto T Get(int i); proto int Count(); }
class map<Class K, Class V> { proto V Get(K k); }
typedef array<string> TStringArray;
class Game { PlayerBase GetPlayer(); }
Game GetGame();
enum EMode { A, B }
class ManBase { void Base(); }
class PlayerBase extends ManBase { void OnConnect() {} int m_Health; }
";

const DEPENDENCY_SRC: &str = "modded class PlayerBase { override void OnConnect() {} }";

const WORKSPACE_SRC: &str = "modded class PlayerBase { \
override void OnConnect() {} \
void Mine() {} \
ref array<ref PlayerBase> m_Items; \
}";

fn build_index() -> Index {
    let mut index = Index::new();
    index.set_file(vanilla_core_key(), extract(VANILLA_SRC));
    index.set_file(dependency_key(), extract(DEPENDENCY_SRC));
    index.set_file(workspace_key(), extract(WORKSPACE_SRC));
    index
}

fn offset_of(src: &str, needle: &str) -> usize {
    src.find(needle).expect("needle not found") + needle.len() - 1
}

#[test]
fn param_receiver_resolves_to_its_declared_type() {
    let index = build_index();
    let src = "class M { void Run(PlayerBase p) { p.Foo(); } }";
    let dot = offset_of(src, "p.");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "PlayerBase");
    assert!(!r.is_static);
}

#[test]
fn a_global_function_call_receiver_resolves_to_its_return_type() {
    let index = build_index();
    let src = "class M { void Run() { GetGame().Foo(); } }";
    let dot = offset_of(src, "GetGame().");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "Game");
}

#[test]
fn a_chained_call_receiver_resolves_through_the_first_calls_return_type() {
    let index = build_index();
    let src = "class M { void Run() { GetGame().GetPlayer().Foo(); } }";
    let dot = offset_of(src, "GetGame().GetPlayer().");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "PlayerBase");
}

#[test]
fn a_generic_field_indexed_through_its_get_method_substitutes_the_type_argument() {
    let index = build_index();
    let src =
        "modded class PlayerBase { ref array<ref PlayerBase> m_Items; void Run() { m_Items.Get(0).Foo(); } }";
    let dot = offset_of(src, "m_Items.Get(0).");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "PlayerBase");
}

#[test]
fn a_method_inherited_from_a_generic_base_substitutes_the_base_type_argument() {
    let cases = [
        (
            "class FooList extends array<PlayerBase> {}",
            "FooList",
            "array<PlayerBase>",
        ),
        (
            "class Mid<Class U> extends array<U> {} class Leaf extends Mid<PlayerBase> {}",
            "Leaf",
            "Mid<PlayerBase>",
        ),
    ];
    for (decl, list, base) in cases {
        let mut index = build_index();
        index.set_file(
            key("workspace/4_World/lists.c", Origin::Workspace, Layer::World),
            extract(decl),
        );
        for src in [
            format!("class M {{ void Run({list} l) {{ l.Get(0).Foo(); }} }}"),
            format!("class Sub extends {base} {{ void Run() {{ super.Get(0).Foo(); }} }}"),
        ] {
            index.set_file(mission_key(), extract(&src));
            let dot = offset_of(&src, "Get(0).");
            let r = infer_receiver(&index, &src, dot).expect("resolves");
            assert_eq!(r.ty.name, "PlayerBase", "{decl} / {src}");
        }
    }
}

#[test]
fn a_typedef_alias_expands_before_generic_substitution() {
    let index = build_index();
    let src = "class M { void Run(TStringArray names) { names.Get(0).Foo(); } }";
    let dot = offset_of(src, "names.Get(0).");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "string");
}

#[test]
fn a_map_get_call_substitutes_the_value_type_argument() {
    let index = build_index();
    let src = "class M { void Run(map<string, PlayerBase> lookup) { lookup.Get(\"a\").Foo(); } }";
    let dot = offset_of(src, "lookup.Get(\"a\").");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "PlayerBase");
}

#[test]
fn indexing_an_array_field_substitutes_its_type_argument() {
    let index = build_index();
    let src = "class M { void Run(array<PlayerBase> list) { list[0].Foo(); } }";
    let dot = offset_of(src, "list[0].");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "PlayerBase");
}

#[test]
fn a_class_name_receiver_is_static() {
    let index = build_index();
    let src = "class M { void Run() { EMode.A; } }";
    let dot = offset_of(src, "EMode.");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "EMode");
    assert!(r.is_static);
}

#[test]
fn an_auto_typed_local_cannot_be_inferred() {
    let index = build_index();
    let src = "class M { void Run() { auto a = GetGame(); a.Foo(); } }";
    let dot = src.rfind("a.").expect("a. present") + 1;
    assert!(infer_receiver(&index, src, dot).is_none());
}

#[test]
fn a_bracket_inside_a_string_argument_does_not_break_the_receiver_scan() {
    let index = build_index();
    for call in ["GetGame(\")\").", "GetGame(\"(\")."] {
        let src = format!("class M {{ void Run() {{ {call}Foo(); }} }}");
        let dot = offset_of(&src, call);
        let r = infer_receiver(&index, &src, dot).expect("resolves");
        assert_eq!(r.ty.name, "Game", "{call}");
    }
}

#[test]
fn comments_inside_the_receiver_are_skipped() {
    let index = build_index();
    for call in [
        "GetGame( // say \"hi\n).GetPlayer().",
        "GetGame(/* \" */).GetPlayer().",
        "GetGame(/* ) */).GetPlayer().",
        "GetGame( // )\n).GetPlayer().",
        "GetGame(\"a\" /* \" */).GetPlayer().",
        "GetGame().GetPlayer() /* \" */ .",
        "GetGame()\n// the player\n.GetPlayer().",
    ] {
        let src = format!("class M {{ void Run() {{ {call}Foo(); }} }}");
        let dot = offset_of(&src, call);
        let r = infer_receiver(&index, &src, dot).unwrap_or_else(|| panic!("{call}: no receiver"));
        assert_eq!(r.ty.name, "PlayerBase", "{call}");
    }
}

#[test]
fn a_name_in_a_comment_before_the_dot_is_not_the_receiver() {
    let index = build_index();
    for call in ["p // q\n.", "p /* q */ ."] {
        let src = format!("class M {{ void Run(Game q, PlayerBase p) {{ {call}Foo(); }} }}");
        let dot = offset_of(&src, call);
        let r = infer_receiver(&index, &src, dot).unwrap_or_else(|| panic!("{call}: no receiver"));
        assert_eq!(r.ty.name, "PlayerBase", "{call}");
    }
}

#[test]
fn a_dot_inside_a_comment_or_string_has_no_receiver() {
    let index = build_index();
    for text in ["// p.", "/* p.", "string s = \"p."] {
        let src = format!("class M {{ void Run(PlayerBase p) {{ {text}\n}} }}");
        let dot = src.find(text).unwrap() + text.len() - 1;
        assert!(infer_receiver(&index, &src, dot).is_none(), "{text}");
    }
}

#[test]
fn a_generic_typedef_base_gives_its_members_their_type_arguments() {
    let mut index = build_index();
    index.set_file(
        key("workspace/4_World/bag.c", Origin::Workspace, Layer::World),
        extract("class StringBag extends TStringArray { }"),
    );
    let src = "class M { void Run(StringBag bag) { bag.Get(0).Foo(); } }";
    let dot = offset_of(src, "Get(0).");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "string");
}

#[test]
fn a_receiver_typed_as_a_class_keeps_it_over_a_typedef_of_the_same_name() {
    let mut index = Index::new();
    index.set_file(
        key("a.c", Origin::Vanilla, Layer::World),
        extract("typedef int[] PhysicsJoint;\nclass PhysicsJoint { void Free(); }"),
    );
    let src = "class M { void Run(PhysicsJoint j) { j.Free(); } }";
    let dot = offset_of(src, "j.");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "PhysicsJoint");
}

#[test]
fn this_resolves_to_the_enclosing_class() {
    let index = build_index();
    let src = "modded class PlayerBase { void Run() { this.Foo(); } }";
    let dot = offset_of(src, "this.");
    let r = infer_receiver(&index, src, dot).expect("resolves");
    assert_eq!(r.ty.name, "PlayerBase");
    assert!(!r.is_static);
}

#[test]
fn super_is_the_class_itself_in_a_modded_link_and_the_base_in_a_plain_class() {
    let index = build_index();
    let cases = [
        (
            "modded class PlayerBase { void Run() { super.Foo(); } }",
            "PlayerBase",
        ),
        (
            "class PlayerBase extends ManBase { void Run() { super.Foo(); } }",
            "ManBase",
        ),
    ];
    for (src, expected) in cases {
        let dot = offset_of(src, "super.");
        let r = infer_receiver(&index, src, dot).expect("resolves");
        assert_eq!(r.ty.name, expected, "{src}");
    }
}

#[test]
fn a_proto_base_method_resolves_through_a_chained_global_call() {
    let src = "class CGame { proto Man GetPlayer(); } \
class DayZGame extends CGame {} \
DayZGame GetGame() { return null; } \
class Man { void Foo(); }";
    let mut index = Index::new();
    index.set_file(vanilla_core_key(), extract(src));

    let call = "modded class CGame { void Run() { GetGame().GetPlayer().Foo(); } }";
    let dot = offset_of(call, "GetGame().GetPlayer().");
    let r = infer_receiver(&index, call, dot).expect("resolves");
    assert_eq!(r.ty.name, "Man");
}

#[test]
fn a_function_declared_twice_resolves_to_the_last_loaded_declaration() {
    let mut index = Index::new();
    let mut early = String::new();
    let mut late = String::new();
    for i in 0..16 {
        early.push_str(&format!("Early Fn{i}();\n"));
        late.push_str(&format!("Late Fn{i}();\n"));
    }
    index.set_file(
        key("vanilla/2_GameLib/g.c", Origin::Vanilla, Layer::GameLib),
        extract(&early),
    );
    index.set_file(
        key("vanilla/3_Game/g.c", Origin::Vanilla, Layer::Game),
        extract(&late),
    );

    for i in 0..16 {
        let name = format!("Fn{i}");
        let ret = index.function(&name).map(|(_, m)| m.ret.name.clone());
        assert_eq!(ret.as_deref(), Some("Late"), "{name} -> {ret:?}");
    }
}
