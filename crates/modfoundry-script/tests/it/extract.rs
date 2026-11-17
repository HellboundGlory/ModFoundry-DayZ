use modfoundry_script::{extract, TypeRef};

const SAMPLE: &str = r#"typedef array<string> TStringArray;
enum EMode { A = 1, B }
modded class PlayerBase extends ManBase
{
    //! the thing
    protected ref array<ref Foo> m_Items;
    static const int MAX = 3;
    override void OnConnect(int a, out string b = "x")
    {
        super.OnConnect(a, b);
    }
    proto native int Native();
    #ifdef SERVER
    void OnlyServer() {}
    #endif
};
class Foo<Class T> { T val; }
"#;

fn class(src: &str, name: &str) -> modfoundry_script::ClassItem {
    let items = extract(src);
    items
        .classes
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no class {name} in {}", items.classes.len()))
        .clone()
}

fn method<'a>(
    c: &'a modfoundry_script::ClassItem,
    name: &str,
) -> &'a modfoundry_script::MethodItem {
    c.methods
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no method {name} in {:?}", c.methods))
}

fn field<'a>(c: &'a modfoundry_script::ClassItem, name: &str) -> &'a modfoundry_script::FieldItem {
    c.fields
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name} in {:?}", c.fields))
}

#[test]
fn extracts_top_level_declarations() {
    let items = extract(SAMPLE);
    assert_eq!(items.classes.len(), 2, "PlayerBase + Foo");
    assert_eq!(items.enums.len(), 1);
    assert_eq!(items.typedefs.len(), 1);
    assert_eq!(items.typedefs[0].name, "TStringArray");
    assert_eq!(items.typedefs[0].target.display(), "array<string>");
}

#[test]
fn class_modifiers_and_base() {
    let c = class(SAMPLE, "PlayerBase");
    assert!(c.modded);
    assert_eq!(
        c.base.as_ref().map(TypeRef::display),
        Some("ManBase".into())
    );
    assert!(c.doc.is_none());
}

#[test]
fn field_type_args_and_doc() {
    let c = class(SAMPLE, "PlayerBase");
    let f = field(&c, "m_Items");
    assert_eq!(f.ty.name, "array");
    assert_eq!(f.ty.args.len(), 1);
    assert_eq!(f.ty.args[0].name, "Foo");
    assert!(f.ty.args[0].strong);
    assert!(f.ty.strong, "the field's own type is a `ref`");
    assert_eq!(f.modifiers, ["protected"]);
    assert_eq!(f.doc.as_deref(), Some("the thing"));
}

#[test]
fn const_field_modifiers() {
    let c = class(SAMPLE, "PlayerBase");
    let f = field(&c, "MAX");
    assert_eq!(f.modifiers, ["static", "const"]);
    assert_eq!(f.ty.name, "int");
}

#[test]
fn method_signature_and_body() {
    let c = class(SAMPLE, "PlayerBase");
    let m = method(&c, "OnConnect");
    assert_eq!(
        m.signature(),
        "override void OnConnect(int a, out string b = \"x\")"
    );
    assert!(m.body.is_some());
    assert_eq!(m.params.len(), 2);
    assert_eq!(m.params[1].modifiers, ["out"]);
    assert_eq!(m.params[1].default.as_deref(), Some("\"x\""));
}

#[test]
fn native_method_has_no_body() {
    let c = class(SAMPLE, "PlayerBase");
    let m = method(&c, "Native");
    assert_eq!(m.modifiers, ["proto", "native"]);
    assert!(m.body.is_none());
    assert_eq!(m.ret.name, "int");
}

#[test]
fn ifdef_guarded_member_is_indexed() {
    let c = class(SAMPLE, "PlayerBase");
    assert_eq!(method(&c, "OnlyServer").name, "OnlyServer");
}

#[test]
fn ifdef_split_class_header_keeps_the_first_arm() {
    let src =
        "#ifdef A\nclass C extends Base1\n#else\nclass C extends Base2\n#endif\n{\n    int x;\n}\n";
    let c = class(src, "C");
    assert_eq!(c.base.as_ref().map(TypeRef::display), Some("Base1".into()));
    assert_eq!(c.fields.len(), 1);
    assert_eq!(field(&c, "x").ty.name, "int");
}

#[test]
fn enum_members() {
    let items = extract(SAMPLE);
    let e = &items.enums[0];
    assert_eq!(e.name, "EMode");
    assert_eq!(
        e.members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );
}

#[test]
fn generic_class_type_params() {
    let c = class(SAMPLE, "Foo");
    assert_eq!(c.type_params, ["T"]);
    let f = field(&c, "val");
    assert_eq!(f.ty.name, "T");
    assert!(!f.ty.strong);
}

#[test]
fn every_name_span_slices_its_name() {
    let items = extract(SAMPLE);
    let check = |name: &str, span: modfoundry_core::Span| {
        assert_eq!(&SAMPLE[span.start..span.end], name);
    };
    for c in &items.classes {
        check(&c.name, c.name_span);
        for f in &c.fields {
            check(&f.name, f.name_span);
        }
        for m in &c.methods {
            check(&m.name, m.name_span);
            for p in &m.params {
                check(&p.name, p.name_span);
            }
        }
    }
    for e in &items.enums {
        check(&e.name, e.name_span);
        for m in &e.members {
            check(&m.name, m.name_span);
        }
    }
    for t in &items.typedefs {
        check(&t.name, t.name_span);
    }
}

#[test]
fn multi_declarator_field_yields_one_item_each() {
    let items = extract("class C { int a, b; }");
    let c = &items.classes[0];
    assert_eq!(c.fields.len(), 2);
    assert_eq!(c.fields[0].name, "a");
    assert_eq!(c.fields[1].name, "b");
    assert!(c.fields.iter().all(|f| f.ty.name == "int"));
}

#[test]
fn top_level_function_and_globals() {
    let items = extract("int g;\nvoid FreeFn(int a) { int local; }\n");
    assert_eq!(items.functions.len(), 1);
    assert_eq!(items.functions[0].name, "FreeFn");
    assert_eq!(items.globals.len(), 1);
    assert_eq!(items.globals[0].name, "g");
}

#[test]
fn broken_source_keeps_the_class_and_its_method() {
    let broken = SAMPLE.replace("    #endif\n};\n", "    #endif\n") + "f.\n";
    assert_ne!(broken, SAMPLE);
    let c = class(&broken, "PlayerBase");
    assert!(c.modded);
    assert_eq!(method(&c, "OnConnect").name, "OnConnect");
}

#[test]
fn empty_and_garbage_do_not_panic() {
    assert!(extract("").classes.is_empty());
    let _ = extract("}}}{{{ class");
}

#[test]
fn plain_comment_is_not_a_doc() {
    let items = extract("class C {\n    // nope\n    int x;\n}\n");
    assert!(items.classes[0].fields[0].doc.is_none());
}

#[test]
fn blank_line_breaks_doc_attachment() {
    let items = extract("class C {\n    //! far\n\n    int x;\n}\n");
    assert!(items.classes[0].fields[0].doc.is_none());
}

#[test]
fn stacked_doc_lines_join() {
    let items = extract("class C {\n    //! one\n    //! two\n    int x;\n}\n");
    assert_eq!(items.classes[0].fields[0].doc.as_deref(), Some("one\ntwo"));
}

#[test]
fn doc_on_class_and_enum() {
    let items = extract("//! class doc\nclass A { }\n/*! block doc */\nenum B { X }\n");
    assert_eq!(items.classes[0].doc.as_deref(), Some("class doc"));
    assert_eq!(items.enums[0].doc.as_deref(), Some("block doc"));
}

#[test]
fn doc_block_strips_markers() {
    let items = extract("class C {\n    /**\n     * alpha\n     * beta\n     */\n    int x;\n}\n");
    assert_eq!(
        items.classes[0].fields[0].doc.as_deref(),
        Some("alpha\nbeta")
    );
}

#[test]
fn a_ref_with_no_modifier_before_it_is_still_a_strong_type() {
    let src = "class A\n{\n    ref W t;\n    void F(ref W a, out ref W b) {}\n}\n";
    let c = class(src, "A");
    assert!(field(&c, "t").ty.strong);
    let f = method(&c, "F");
    assert!(f.params.iter().all(|p| p.ty.strong), "{:?}", f.params);
}
