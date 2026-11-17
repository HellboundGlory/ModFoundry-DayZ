use crate::common::key;
use modfoundry_script::{extract, FileKey, Index, Layer, MemberKind, Origin};

fn build_index() -> Index {
    let mut index = Index::new();
    index.set_file(
        key("vanilla/4_World/manbase.c", Origin::Vanilla, Layer::World),
        extract("class ManBase { void Base(); }"),
    );
    index.set_file(
        key(
            "vanilla/4_World/playerbase.c",
            Origin::Vanilla,
            Layer::World,
        ),
        extract(
            "class PlayerBase extends ManBase {\n\
             //! vanilla doc\n\
             void OnConnect() {} int m_Health; }",
        ),
    );
    index.set_file(
        key(
            "dependency/4_World/playerbase.c",
            Origin::Dependency(0),
            Layer::World,
        ),
        extract("modded class PlayerBase { override void OnConnect() { super.OnConnect(); } }"),
    );
    index.set_file(
        key(
            "workspace/4_World/playerbase.c",
            Origin::Workspace,
            Layer::World,
        ),
        extract(
            "modded class PlayerBase { override void OnConnect() { super.OnConnect(); } \
             void Mine() { OnConnect(); Base(); m_Health = 1; } }",
        ),
    );
    index
}

#[test]
fn class_chain_orders_vanilla_then_dependency_then_workspace() {
    let index = build_index();
    let chain = index.class_chain("PlayerBase");
    let ids: Vec<&str> = chain.iter().map(|l| l.key.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "vanilla/4_World/playerbase.c",
            "dependency/4_World/playerbase.c",
            "workspace/4_World/playerbase.c",
        ]
    );
}

#[test]
fn a_later_module_layer_loads_after_every_mod_in_an_earlier_one() {
    let mut index = Index::new();
    index.set_file(
        key("vanilla/3_Game/foo.c", Origin::Vanilla, Layer::Game),
        extract("class Foo { void A(); }"),
    );
    index.set_file(
        key(
            "dependency/4_World/foo.c",
            Origin::Dependency(0),
            Layer::World,
        ),
        extract("modded class Foo { void B(); }"),
    );
    index.set_file(
        key("workspace/3_Game/foo.c", Origin::Workspace, Layer::Game),
        extract("modded class Foo { void C(); }"),
    );
    let chain = index.class_chain("Foo");
    let ids: Vec<&str> = chain.iter().map(|l| l.key.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "vanilla/3_Game/foo.c",
            "workspace/3_Game/foo.c",
            "dependency/4_World/foo.c",
        ]
    );
}

#[test]
fn base_class_reads_the_non_modded_definition() {
    let index = build_index();
    assert_eq!(index.base_class("PlayerBase"), Some("ManBase"));
}

#[test]
fn find_members_walks_last_to_first_then_base() {
    let index = build_index();
    let onconnect = index.find_members("PlayerBase", "OnConnect");
    let ids: Vec<&str> = onconnect.iter().map(|d| d.key.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "workspace/4_World/playerbase.c",
            "dependency/4_World/playerbase.c",
            "vanilla/4_World/playerbase.c",
        ]
    );

    let base = index.find_members("PlayerBase", "Base");
    assert_eq!(base.len(), 1);
    assert_eq!(base[0].key.id, "vanilla/4_World/manbase.c");
}

#[test]
fn type_exists_covers_primitives_classes_and_unknown() {
    let index = build_index();
    assert!(index.type_exists("int"));
    assert!(index.type_exists("auto"));
    assert!(index.type_exists("PlayerBase"));
    assert!(index.type_exists("ManBase"));
    assert!(!index.type_exists("Nonexistent"));
}

#[test]
fn set_file_replace_never_leaves_stale_members() {
    let mut index = build_index();
    index.set_file(
        key(
            "dependency/4_World/playerbase.c",
            Origin::Dependency(0),
            Layer::World,
        ),
        extract(""),
    );
    let chain = index.class_chain("PlayerBase");
    assert_eq!(chain.len(), 2);
    let onconnect = index.find_members("PlayerBase", "OnConnect");
    assert_eq!(onconnect.len(), 2);
}

#[test]
fn set_file_and_remove_file_keep_name_indexes_consistent() {
    let mut index = Index::new();
    index.set_file(
        key("a.c", Origin::Vanilla, Layer::World),
        extract("class X {} enum E { A } typedef int Y;\nvoid F() {}\nint g;\n"),
    );
    index.set_file(
        key("b.c", Origin::Vanilla, Layer::World),
        extract("class X {}"),
    );

    assert!(index.type_exists("X"));
    assert_eq!(index.class_chain("X").len(), 2);
    assert!(index.enum_def("E").is_some());
    assert!(index.typedef_def("Y").is_some());
    assert!(index.function("F").is_some());
    assert!(index.global("g").is_some());

    index.set_file(
        key("a.c", Origin::Vanilla, Layer::World),
        extract("int unrelated;\n"),
    );

    let chain = index.class_chain("X");
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].key.id, "b.c");
    assert!(index.enum_def("E").is_none());
    assert!(index.typedef_def("Y").is_none());
    assert!(index.function("F").is_none());
    assert!(index.global("g").is_none());
    assert!(index.type_exists("X"));

    index.remove_file("b.c");
    assert!(!index.type_exists("X"));
    assert!(index.class_chain("X").is_empty());
}

#[test]
fn inheritance_cycle_terminates() {
    let mut index = Index::new();
    index.set_file(
        key("a.c", Origin::Vanilla, Layer::World),
        extract("class A extends B {}"),
    );
    index.set_file(
        key("b.c", Origin::Vanilla, Layer::World),
        extract("class B extends A {}"),
    );
    let members = index.find_members("A", "anything");
    assert!(members.is_empty());
}

#[test]
fn a_typedef_base_is_followed_to_the_class_it_names() {
    let mut index = Index::new();
    index.set_file(
        key("a.c", Origin::Vanilla, Layer::World),
        extract(
            "class Real { void Foo(); }\n\
             typedef Real Alias;\n\
             typedef Alias Alias2;\n\
             class Derived : Alias { }\n\
             class Derived2 extends Alias2 { }",
        ),
    );
    assert_eq!(index.base_class("Derived"), Some("Real"));
    assert_eq!(index.base_class("Derived2"), Some("Real"));
    assert_eq!(index.find_members("Derived2", "Foo").len(), 1);
}

#[test]
fn a_class_wins_over_a_typedef_of_the_same_name() {
    let mut index = Index::new();
    index.set_file(
        key("a.c", Origin::Vanilla, Layer::World),
        extract(
            "typedef int[] PhysicsJoint;\n\
             class PhysicsJoint { void Free(); }\n\
             class Physics6DOFJoint : PhysicsJoint { }",
        ),
    );
    assert_eq!(index.base_class("Physics6DOFJoint"), Some("PhysicsJoint"));
    assert_eq!(index.find_members("Physics6DOFJoint", "Free").len(), 1);
}

#[test]
fn typedef_loops_in_a_base_terminate() {
    let mut index = Index::new();
    index.set_file(
        key("a.c", Origin::Vanilla, Layer::World),
        extract(
            "typedef LoopB LoopA;\n\
             typedef LoopA LoopB;\n\
             class OnLoop extends LoopA { void Own(); }\n\
             class Self extends SelfAlias { void Mine(); }\n\
             typedef Self SelfAlias;",
        ),
    );
    assert!(index.base_class("OnLoop").is_some());
    assert_eq!(index.find_members("OnLoop", "Own").len(), 1);
    assert_eq!(index.find_members("Self", "Mine").len(), 1);
}

#[test]
fn all_members_keeps_the_last_loaded_definition_of_a_name() {
    let mut index = Index::new();
    for i in 0..20 {
        index.set_file(
            key(
                &format!("dependency/4_World/c{i}.c"),
                Origin::Dependency(i),
                Layer::World,
            ),
            extract(&format!("class C{i} {{ void Init(int a); }}")),
        );
    }
    index.set_file(
        key("workspace/5_Mission/w.c", Origin::Workspace, Layer::Mission),
        extract("class W { void Init(string s); }"),
    );
    let members = index.all_members();
    let init: Vec<_> = members
        .iter()
        .filter(|d| matches!(d.member, MemberKind::Method(m) if m.name == "Init"))
        .collect();
    assert_eq!(init.len(), 1);
    assert_eq!(init[0].owner, "W");
    assert_eq!(init[0].key.id, "workspace/5_Mission/w.c");
}

#[test]
fn a_file_outside_a_numbered_folder_loads_after_every_layer() {
    let mut index = Index::new();
    index.set_file(
        key("vanilla/5_Mission/foo.c", Origin::Vanilla, Layer::Mission),
        extract("class Foo {}"),
    );
    index.set_file(
        key(
            "workspace/5_Mission/foo.c",
            Origin::Workspace,
            Layer::Mission,
        ),
        extract("modded class Foo { void A(); }"),
    );
    index.set_file(
        FileKey {
            id: "dependency/foo.c".to_owned(),
            origin: Origin::Dependency(0),
            layer: None,
        },
        extract("modded class Foo { void A(); }"),
    );
    let chain = index.class_chain("Foo");
    let ids: Vec<&str> = chain.iter().map(|l| l.key.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "vanilla/5_Mission/foo.c",
            "workspace/5_Mission/foo.c",
            "dependency/foo.c",
        ]
    );
    assert_eq!(index.find_members("Foo", "A")[0].key.id, "dependency/foo.c");
}
