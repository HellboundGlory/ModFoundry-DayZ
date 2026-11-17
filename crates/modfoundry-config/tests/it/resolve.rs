use modfoundry_config::ast::Value;
use modfoundry_config::parser::parse;
use modfoundry_config::resolve::{ConfigOrigin, Resolver, SourceConfig};

fn source(id: &str, origin: ConfigOrigin, text: &str) -> SourceConfig {
    let parsed = parse(text);
    assert!(parsed.issues.is_empty(), "{id}: {:?}", parsed.issues);
    SourceConfig {
        id: id.into(),
        display: id.into(),
        vpath: format!("{id}/config.cpp"),
        origin,
        file: parsed.file,
    }
}

fn patch(name: &str, requires: &[&str]) -> String {
    let list = requires
        .iter()
        .map(|r| format!("\"{r}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("class CfgPatches {{ class {name} {{ requiredAddons[] = {{ {list} }}; }}; }};\n")
}

fn vanilla(id: &str, name: &str, requires: &[&str], body: &str) -> SourceConfig {
    source(
        id,
        ConfigOrigin::Vanilla,
        &format!("{}{body}", patch(name, requires)),
    )
}

fn workspace(id: &str, name: &str, requires: &[&str], body: &str) -> SourceConfig {
    source(
        id,
        ConfigOrigin::Workspace,
        &format!("{}{body}", patch(name, requires)),
    )
}

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn num(text: &str) -> Value {
    Value::Number(text.into())
}

fn items(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| num(v)).collect())
}

#[test]
fn later_source_overrides_and_tags_provenance() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Base { scope = 1; tag = 5; }; class Item: Base { scope = 2; }; };",
    );
    let b = workspace(
        "workspace:b",
        "B",
        &["A"],
        "class CfgVehicles { class Item { scope = 3; }; };",
    );
    let r = Resolver::new(vec![b, a]);
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    let scope = item.properties.iter().find(|p| p.name == "scope").unwrap();
    assert_eq!(scope.value, num("3"));
    assert_eq!(scope.from_source, "workspace:b");
    assert_eq!(scope.from_class, path(&["CfgVehicles", "Item"]));
    assert!(scope.own);
    let tag = item.properties.iter().find(|p| p.name == "tag").unwrap();
    assert_eq!(tag.from_class, path(&["CfgVehicles", "Base"]));
    assert_eq!(tag.from_source, "vanilla:a");
    assert!(!tag.own);
    assert_eq!(item.base_chain.len(), 1);
    assert_eq!(item.base_chain[0].path, path(&["CfgVehicles", "Base"]));
    assert_eq!(item.base_chain[0].source, "vanilla:a");
    assert_eq!(r.load_order(), vec!["vanilla:a", "workspace:b"]);
}

#[test]
fn plus_equals_appends_to_inherited_value() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Base { list[] = {1,2}; }; class Item: Base {}; };",
    );
    let b = workspace(
        "workspace:b",
        "B",
        &["A"],
        "class CfgVehicles { class Item { list[] += {3}; }; };",
    );
    let r = Resolver::new(vec![a, b]);
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    let list = item.properties.iter().find(|p| p.name == "list").unwrap();
    assert_eq!(list.value, items(&["1", "2", "3"]));
    assert!(list.array);
    assert!(list.own);
    assert_eq!(list.from_source, "workspace:b");
}

#[test]
fn plus_equals_appends_to_own_value() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Base { list[] = {1}; }; class Item: Base { list[] = {9}; }; };",
    );
    let b = workspace(
        "workspace:b",
        "B",
        &["A"],
        "class CfgVehicles { class Item { list[] += {3}; }; };",
    );
    let r = Resolver::new(vec![a, b]);
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    let list = item.properties.iter().find(|p| p.name == "list").unwrap();
    assert_eq!(list.value, items(&["9", "3"]));
}

#[test]
fn delete_removes_properties_and_classes() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Item { scope = 2; keep = 1; class Sub {}; }; };",
    );
    let b = workspace(
        "workspace:b",
        "B",
        &["A"],
        "class CfgVehicles { class Item { delete scope; delete Sub; }; };",
    );
    let r = Resolver::new(vec![a, b]);
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    let names: Vec<_> = item.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["keep"]);
    assert!(item.children.is_empty());
}

#[test]
fn forward_declaration_then_definition() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Base; class Item: Base { own = 1; }; };",
    );
    let b = vanilla(
        "vanilla:b",
        "B",
        &["A"],
        "class CfgVehicles { class Base { from_b = 1; }; };",
    );
    let r = Resolver::new(vec![a, b]);
    assert_eq!(
        r.defined_by(&path(&["CfgVehicles", "Base"])),
        vec!["vanilla:b"]
    );
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    assert!(item.properties.iter().any(|p| p.name == "from_b" && !p.own));
    assert_eq!(item.unresolved_base, None);
}

#[test]
fn forward_only_base_resolves_but_has_no_definer() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Base; class Item: Base {}; };",
    );
    let r = Resolver::new(vec![a]);
    assert!(r.defined_by(&path(&["CfgVehicles", "Base"])).is_empty());
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    assert_eq!(item.base_chain.len(), 1);
}

#[test]
fn forward_declaration_sets_base_only_when_missing() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class P {}; class Q {}; class Item: P {}; class Later; };",
    );
    let b = vanilla(
        "vanilla:b",
        "B",
        &["A"],
        "class CfgVehicles { class Item: Q; class Later: P; };",
    );
    let r = Resolver::new(vec![a, b]);
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    assert_eq!(item.base_chain[0].path, path(&["CfgVehicles", "P"]));
    assert!(item.rebases.is_empty());
    let later = r.class(&path(&["CfgVehicles", "Later"])).unwrap();
    assert_eq!(later.base_chain[0].path, path(&["CfgVehicles", "P"]));
}

#[test]
fn rebase_is_recorded_and_later_base_wins() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class P {}; class Q {}; class Item: P {}; };",
    );
    let b = workspace(
        "workspace:b",
        "B",
        &["A"],
        "class CfgVehicles { class Item: Q {}; };",
    );
    let r = Resolver::new(vec![a, b]);
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    assert_eq!(item.base_chain[0].path, path(&["CfgVehicles", "Q"]));
    assert_eq!(item.rebases.len(), 1);
    assert_eq!(item.rebases[0].source, "workspace:b");
    assert_eq!(item.rebases[0].from, "P");
    assert_eq!(item.rebases[0].to, "Q");
}

#[test]
fn source_without_cfgpatches_loads_first_and_is_in_every_closure() {
    let core = source(
        "vanilla:bin.pbo:config.bin",
        ConfigOrigin::Workspace,
        "class CfgVehicles { class Core { scope = 0; }; };",
    );
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Item: Core {}; };",
    );
    let b = workspace("workspace:b", "B", &["A"], "");
    let r = Resolver::new(vec![b, a, core]);
    assert_eq!(
        r.load_order(),
        vec!["vanilla:bin.pbo:config.bin", "vanilla:a", "workspace:b"]
    );
    for id in ["vanilla:a", "workspace:b"] {
        assert!(r
            .required_closure(id)
            .contains("vanilla:bin.pbo:config.bin"));
    }
    let closure = r.required_closure("workspace:b");
    assert!(closure.contains("workspace:b") && closure.contains("vanilla:a"));
    assert!(r
        .class(&path(&["CfgVehicles", "Item"]))
        .unwrap()
        .unresolved_base
        .is_none());
}

#[test]
fn closure_follows_required_addons_only() {
    let a = vanilla("vanilla:a", "A", &[], "");
    let b = vanilla("vanilla:b", "B", &["a"], "");
    let c = vanilla("vanilla:c", "C", &[], "");
    let r = Resolver::new(vec![a, b, c]);
    let closure = r.required_closure("vanilla:b");
    assert!(closure.contains("vanilla:a"));
    assert!(!closure.contains("vanilla:c"));
    assert_eq!(r.owner_patch("vanilla:b"), Some("B"));
    let patches = r.patches();
    let b_patch = patches.iter().find(|p| p.name == "B").unwrap();
    assert_eq!(b_patch.source, "vanilla:b");
    assert_eq!(b_patch.required_addons, vec!["a".to_string()]);
}

#[test]
fn missing_required_addons_are_ignored() {
    let a = vanilla("vanilla:a", "A", &["Nowhere"], "");
    let r = Resolver::new(vec![a]);
    assert_eq!(r.load_order(), vec!["vanilla:a"]);
    assert!(r.cycles().is_empty());
}

#[test]
fn same_class_name_under_two_roots_stays_separate() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Thing { x = 1; }; }; class CfgWeapons { class Thing { x = 2; }; };",
    );
    let r = Resolver::new(vec![a]);
    let v = r.class(&path(&["CfgVehicles", "Thing"])).unwrap();
    let w = r.class(&path(&["cfgweapons", "THING"])).unwrap();
    assert_eq!(v.properties[0].value, num("1"));
    assert_eq!(w.properties[0].value, num("2"));
}

#[test]
fn nested_scope_wins_over_the_outer_one() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class Inner { outer = 1; }; class Outer { class Inner { nested = 1; }; class X: Inner {}; };",
    );
    let r = Resolver::new(vec![a]);
    let x = r.class(&path(&["Outer", "X"])).unwrap();
    assert_eq!(x.base_chain[0].path, path(&["Outer", "Inner"]));
    assert!(x.properties.iter().any(|p| p.name == "nested"));
    assert!(!x.properties.iter().any(|p| p.name == "outer"));
}

#[test]
fn outer_scope_is_used_when_nothing_nested_matches() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class Shared { s = 1; }; class Outer { class Deep { class X: Shared {}; }; };",
    );
    let r = Resolver::new(vec![a]);
    let x = r.class(&path(&["Outer", "Deep", "X"])).unwrap();
    assert_eq!(x.base_chain[0].path, path(&["Shared"]));
}

#[test]
fn self_named_child_resolves_to_the_inherited_one() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class A { class DamageSystem { from_a = 1; class Inner {}; }; }; \
         class B: A { class DamageSystem: DamageSystem { own = 1; }; }; };",
    );
    let r = Resolver::new(vec![a]);
    let ds = r
        .class(&path(&["CfgVehicles", "B", "DamageSystem"]))
        .unwrap();
    assert_eq!(ds.unresolved_base, None);
    assert_eq!(
        ds.base_chain[0].path,
        path(&["CfgVehicles", "A", "DamageSystem"])
    );
    assert!(ds.properties.iter().any(|p| p.name == "from_a" && !p.own));
    assert!(ds.properties.iter().any(|p| p.name == "own" && p.own));
}

#[test]
fn child_classes_are_inherited_with_counts() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class A { class DamageSystem { class Health {}; class Global {}; }; }; \
         class B: A { class Extra {}; }; };",
    );
    let r = Resolver::new(vec![a]);
    let b = r.class(&path(&["CfgVehicles", "B"])).unwrap();
    let ds = b
        .children
        .iter()
        .find(|c| c.name == "DamageSystem")
        .unwrap();
    assert!(!ds.own);
    assert_eq!(ds.from_class, path(&["CfgVehicles", "A"]));
    assert_eq!(ds.classes_below, 2);
    let extra = b.children.iter().find(|c| c.name == "Extra").unwrap();
    assert!(extra.own);
    assert_eq!(extra.from_class, path(&["CfgVehicles", "B"]));
}

#[test]
fn inherited_child_path_resolves_through_the_base() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class A { class DamageSystem { hp = 1; }; }; class B: A {}; };",
    );
    let r = Resolver::new(vec![a]);
    let ds = r
        .class(&path(&["CfgVehicles", "B", "DamageSystem"]))
        .unwrap();
    assert!(ds.properties.iter().any(|p| p.name == "hp"));
}

#[test]
fn unresolved_base_is_reported_without_panic() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Item: Nowhere { x = 1; }; };",
    );
    let r = Resolver::new(vec![a]);
    let item = r.class(&path(&["CfgVehicles", "Item"])).unwrap();
    assert_eq!(item.unresolved_base.as_deref(), Some("Nowhere"));
    assert!(item.base_chain.is_empty());
    assert_eq!(item.properties.len(), 1);
    assert!(r.class(&path(&["CfgVehicles", "Missing"])).is_none());
    assert!(r.class(&[]).is_none());
}

#[test]
fn base_chain_cycle_is_guarded() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class A: B { a = 1; }; class B: A { b = 1; }; };",
    );
    let r = Resolver::new(vec![a]);
    let class_a = r.class(&path(&["CfgVehicles", "A"])).unwrap();
    assert!(class_a.base_chain.len() <= 2);
    assert!(class_a.properties.iter().any(|p| p.name == "a"));
}

#[test]
fn required_addons_cycle_is_ordered_deterministically_and_reported() {
    let build = || {
        vec![
            workspace("workspace:w", "W", &["Y"], ""),
            vanilla("vanilla:y", "Y", &["X"], ""),
            vanilla("vanilla:x", "X", &["Y"], ""),
        ]
    };
    let r = Resolver::new(build());
    let mut reversed = build();
    reversed.reverse();
    let r2 = Resolver::new(reversed);
    assert_eq!(
        r.load_order(),
        vec!["vanilla:x", "vanilla:y", "workspace:w"]
    );
    assert_eq!(r.load_order(), r2.load_order());
    assert_eq!(
        r.cycles(),
        vec![vec!["vanilla:x".to_string(), "vanilla:y".to_string()]]
    );
}

#[test]
fn ties_break_by_origin_then_source_id() {
    let dep = source(
        "dependency:1:d",
        ConfigOrigin::Dependency(1),
        &patch("D", &[]),
    );
    let dep0 = source(
        "dependency:0:z",
        ConfigOrigin::Dependency(0),
        &patch("Z", &[]),
    );
    let ws = workspace("workspace:a", "W", &[], "");
    let van = vanilla("vanilla:q", "Q", &[], "");
    let r = Resolver::new(vec![ws, dep, dep0, van]);
    assert_eq!(
        r.load_order(),
        vec![
            "vanilla:q",
            "dependency:0:z",
            "dependency:1:d",
            "workspace:a"
        ]
    );
}

#[test]
fn with_overlay_replaces_a_workspace_source() {
    let a = vanilla(
        "vanilla:a",
        "A",
        &[],
        "class CfgVehicles { class Base { scope = 1; }; };",
    );
    let saved = workspace(
        "workspace:w",
        "W",
        &["A"],
        "class CfgVehicles { class Item: Base { scope = 2; }; };",
    );
    let r = Resolver::new(vec![a, saved]);
    let unsaved = workspace(
        "workspace:w",
        "W",
        &["A"],
        "class CfgVehicles { class Item: Base { scope = 7; }; };",
    );
    let overlaid = r.with_overlay(unsaved);
    let scope = |r: &Resolver| {
        r.class(&path(&["CfgVehicles", "Item"]))
            .unwrap()
            .properties
            .iter()
            .find(|p| p.name == "scope")
            .unwrap()
            .value
            .clone()
    };
    assert_eq!(scope(&r), num("2"));
    assert_eq!(scope(&overlaid), num("7"));
    assert_eq!(overlaid.load_order(), r.load_order());
}

#[test]
fn closure_follows_required_addons_appended_with_plus_equals() {
    let a = vanilla("vanilla:a", "A", &[], "");
    let c = vanilla("vanilla:c", "C", &[], "");
    let b = source(
        "vanilla:b",
        ConfigOrigin::Vanilla,
        "class CfgPatches { class B { requiredAddons[] = { \"a\" }; requiredAddons[] += { \"c\" }; }; };\n",
    );
    let r = Resolver::new(vec![a, b, c]);
    let closure = r.required_closure("vanilla:b");
    assert!(closure.contains("vanilla:a"));
    assert!(closure.contains("vanilla:c"));
    let patches = r.patches();
    let b_patch = patches.iter().find(|p| p.name == "B").unwrap();
    assert_eq!(b_patch.required_addons, ["a", "c"]);
}
