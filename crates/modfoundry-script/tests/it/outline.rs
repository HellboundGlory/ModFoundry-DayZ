use modfoundry_script::outline;
use serde_json::{json, Value};

fn to_json(path: &str, source: &str) -> Value {
    serde_json::to_value(outline(path, source)).unwrap()
}

#[test]
fn a_modded_class_lists_its_field_and_method() {
    let source = r#"modded class A : B<int>
{
    protected int m_Value;
    override protected void F(int x, string s = "a")
    {
    }
}
"#;
    let value = to_json("Scripts/4_World/a.c", source);
    assert_eq!(value["layer"], "4_World");
    assert_eq!(
        value["classes"][0],
        json!({
            "name": "A",
            "modded": true,
            "base": "B<int>",
            "type_params": [],
            "guards": [],
            "lines": [1, 7],
            "attributes": [],
            "decl_line": 1,
            "fields": [{
                "name": "m_Value",
                "type": "int",
                "modifiers": ["protected"],
                "guards": [],
                "line": 3,
                "attributes": [],
                "value": null,
                "array": []
            }],
            "methods": [{
                "name": "F",
                "return": "void",
                "modifiers": ["override", "protected"],
                "params": [
                    {"name": "x", "type": "int", "modifiers": [], "default": null, "array": []},
                    {"name": "s", "type": "string", "modifiers": [], "default": "\"a\"", "array": []}
                ],
                "guards": [],
                "lines": [4, 6],
                "attributes": [],
                "decl_line": 4,
                "destructor": false,
                "return_notnull": false,
                "calls": [],
                "strings": [],
                "returns": [],
                "locals": [],
                "types": []
            }]
        })
    );
}

#[test]
fn methods_in_both_arms_of_an_ifdef_are_listed_with_their_guard() {
    let source = "class Hub\n{\n    #ifdef SERVER\n    void OnServer() {}\n    #else\n    void OnClient() {}\n    #endif\n}\n";
    let value = to_json("Scripts/3_Game/hub.c", source);
    assert_eq!(
        value["classes"][0],
        json!({
            "name": "Hub",
            "modded": false,
            "base": null,
            "type_params": [],
            "guards": [],
            "lines": [1, 8],
            "attributes": [], "decl_line": 1,
            "fields": [],
            "methods": [
                {
                    "name": "OnServer",
                    "return": "void",
                    "modifiers": [],
                    "params": [],
                    "guards": [{"define": "SERVER", "negated": false, "arm": "then"}],
                    "lines": [4, 4],
                    "attributes": [], "decl_line": 4, "destructor": false, "return_notnull": false,
                    "calls": [],
                    "strings": [],
                    "returns": [],
                    "locals": [],
                    "types": []
                },
                {
                    "name": "OnClient",
                    "return": "void",
                    "modifiers": [],
                    "params": [],
                    "guards": [{"define": "SERVER", "negated": false, "arm": "else"}],
                    "lines": [6, 6],
                    "attributes": [], "decl_line": 6, "destructor": false, "return_notnull": false,
                    "calls": [],
                    "strings": [],
                    "returns": [],
                    "locals": [],
                    "types": []
                }
            ]
        })
    );
}

#[test]
fn a_class_in_the_else_arm_of_a_top_level_ifndef_is_listed_with_its_guard() {
    let source = "#ifndef X\nclass Shown {}\n#else\nclass Hidden {}\n#endif\n";
    let value = to_json("Scripts/3_Game/gate.c", source);
    assert_eq!(
        value["classes"],
        json!([
            {
                "name": "Shown",
                "modded": false,
                "base": null,
                "type_params": [],
                "guards": [{"define": "X", "negated": true, "arm": "then"}],
                "lines": [2, 2],
                "attributes": [], "decl_line": 2,
                "fields": [],
                "methods": []
            },
            {
                "name": "Hidden",
                "modded": false,
                "base": null,
                "type_params": [],
                "guards": [{"define": "X", "negated": true, "arm": "else"}],
                "lines": [4, 4],
                "attributes": [], "decl_line": 4,
                "fields": [],
                "methods": []
            }
        ])
    );
}

#[test]
fn an_else_nested_in_an_ifdef_arm_is_inactive_and_not_listed() {
    let source = "class Hub\n{\n#ifdef DEDICATED\n    #ifdef SERVER\n    void Keep() {}\n    #else\n    void Drop() {}\n    #endif\n#endif\n}\n";
    let value = to_json("Scripts/3_Game/hub.c", source);
    assert_eq!(value["inactive"], json!([[7, 7]]));
    assert_eq!(
        value["classes"][0]["methods"],
        json!([{
            "name": "Keep",
            "return": "void",
            "modifiers": [],
            "params": [],
            "guards": [
                {"define": "DEDICATED", "negated": false, "arm": "then"},
                {"define": "SERVER", "negated": false, "arm": "then"}
            ],
            "lines": [5, 5],
            "attributes": [], "decl_line": 5, "destructor": false, "return_notnull": false,
            "calls": [],
            "strings": [],
            "returns": [],
            "locals": [],
            "types": []
        }])
    );
}

#[test]
fn free_functions_globals_enums_and_typedefs_are_listed() {
    let source = "typedef array<string> TStrings;\nenum EMode { A = 1, B }\nint g_Count = 3;\nvoid Helper(int x) {}\n";
    let value = to_json("Scripts/3_Game/misc.c", source);
    assert_eq!(
        value["typedefs"],
        json!([{"name": "TStrings", "target": "array<string>", "guards": [], "line": 1}])
    );
    assert_eq!(
        value["enums"],
        json!([{"name": "EMode", "base": null, "members": ["A", "B"], "values": ["1", null], "guards": [], "line": 2}])
    );
    assert_eq!(
        value["globals"],
        json!([{"name": "g_Count", "type": "int", "modifiers": [], "guards": [], "line": 3, "attributes": [], "value": "3", "array": []}])
    );
    assert_eq!(
        value["functions"],
        json!([{
            "name": "Helper",
            "return": "void",
            "modifiers": [],
            "params": [{"name": "x", "type": "int", "modifiers": [], "default": null, "array": []}],
            "guards": [],
            "lines": [4, 4],
            "attributes": [], "decl_line": 4, "destructor": false, "return_notnull": false,
            "calls": [],
            "strings": [],
            "returns": [],
            "locals": [],
            "types": []
        }])
    );
}

#[test]
fn preprocessor_directive_lines_are_never_listed_as_inactive() {
    let source = "class Hub\n{\n#ifdef A\n    #ifdef D\n    void Keep() {}\n    #else\n    #ifdef B\n    void Drop() {}\n    #endif\n    #endif\n#endif\n}\n";
    let value = to_json("Scripts/3_Game/hub.c", source);
    assert_eq!(value["inactive"], json!([[8, 8]]));
}

#[test]
fn members_both_views_share_are_listed_once() {
    let header = "#ifdef X\nclass A : B\n#else\nclass A : C\n#endif\n{\n    int f;\n}\n";
    let classes = &to_json("a.c", header)["classes"];
    let fields: Vec<usize> = (0..2)
        .map(|i| classes[i]["fields"].as_array().unwrap().len())
        .collect();
    assert_eq!(classes.as_array().unwrap().len(), 2);
    assert_eq!(fields, [1, 0]);

    let members = "enum E {\n    A,\n#ifdef X\n    B,\n#else\n    C,\n#endif\n}\n";
    assert_eq!(
        to_json("a.c", members)["enums"][0]["members"],
        json!(["A", "B", "C"])
    );
}

#[test]
fn enum_members_list_their_values() {
    let value = to_json("a.c", "enum E { A, B = 2, C = 1 << 3 }\n");
    assert_eq!(value["enums"][0]["members"], json!(["A", "B", "C"]));
    assert_eq!(value["enums"][0]["values"], json!([null, "2", "1 << 3"]));

    let members = "enum E {\n    A,\n#ifdef X\n    B = 1,\n#else\n    C = 2,\n#endif\n}\n";
    assert_eq!(
        to_json("a.c", members)["enums"][0]["values"],
        json!([null, "1", "2"])
    );
}

#[test]
fn a_method_body_lists_its_calls_and_strings_in_source_order() {
    let source = r#"class A
{
    void F()
    {
        Init();
        m_Rpc.Send(Cast(obj));
        GetGame()
            .GetRPC().Send(a,
             b);
        super.F();
        Print("one");
        Print("two \"q\"");
    }
}
"#;
    let value = to_json("Scripts/3_Game/a.c", source);
    let method = &value["classes"][0]["methods"][0];
    assert_eq!(
        method["calls"],
        json!([
            {"receiver": null, "name": "Init", "args": 0, "arg_texts": [], "line": 5},
            {"receiver": "m_Rpc", "name": "Send", "args": 1, "arg_texts": ["Cast(obj)"], "line": 6},
            {"receiver": null, "name": "Cast", "args": 1, "arg_texts": ["obj"], "line": 6},
            {"receiver": null, "name": "GetGame", "args": 0, "arg_texts": [], "line": 7},
            {"receiver": "GetGame()", "name": "GetRPC", "args": 0, "arg_texts": [], "line": 8},
            {"receiver": "GetGame() .GetRPC()", "name": "Send", "args": 2, "arg_texts": ["a", "b"], "line": 8},
            {"receiver": "super", "name": "F", "args": 0, "arg_texts": [], "line": 10},
            {"receiver": null, "name": "Print", "args": 1, "arg_texts": ["\"one\""], "line": 11},
            {"receiver": null, "name": "Print", "args": 1, "arg_texts": ["\"two \\\"q\\\"\""], "line": 12}
        ])
    );
    assert_eq!(
        method["strings"],
        json!([
            {"text": "one", "line": 11},
            {"text": "two \\\"q\\\"", "line": 12}
        ])
    );
}

#[test]
fn calls_in_else_arms_are_listed_and_calls_in_arms_neither_view_keeps_are_not() {
    let source = "class Hub\n{\n    void F()\n    {\n#ifdef DEDICATED\n    #ifdef SERVER\n        Keep();\n    #else\n        Drop();\n    #endif\n#else\n        Other(\"o\");\n#endif\n        Last();\n    }\n#ifdef SERVER\n#else\n    void G() { Client(); }\n#endif\n}\n";
    let value = to_json("Scripts/3_Game/hub.c", source);
    let methods = &value["classes"][0]["methods"];
    assert_eq!(
        methods[0]["calls"],
        json!([
            {"receiver": null, "name": "Keep", "args": 0, "arg_texts": [], "line": 7},
            {"receiver": null, "name": "Other", "args": 1, "arg_texts": ["\"o\""], "line": 12},
            {"receiver": null, "name": "Last", "args": 0, "arg_texts": [], "line": 14}
        ])
    );
    assert_eq!(methods[0]["strings"], json!([{"text": "o", "line": 12}]));
    assert_eq!(methods[1]["calls"][0]["name"], "Client");
}

#[test]
fn attributes_and_declaration_lines_are_listed() {
    let source = "[RegisterProfileSetting(\"a\", 1)]\nclass A\n{\n    [NonSerialized()]\n    int m_Count;\n    [Obsolete()]\n    void F() {}\n}\n";
    let class = &to_json("Scripts/3_Game/a.c", source)["classes"][0];
    assert_eq!(
        class["attributes"],
        json!(["RegisterProfileSetting(\"a\", 1)"])
    );
    assert_eq!(class["decl_line"], 2);
    assert_eq!(class["lines"], json!([1, 8]));
    assert_eq!(class["fields"][0]["attributes"], json!(["NonSerialized()"]));
    let method = &class["methods"][0];
    assert_eq!(method["attributes"], json!(["Obsolete()"]));
    assert_eq!(method["decl_line"], 7);
    assert_eq!(method["lines"], json!([6, 7]));
}

#[test]
fn destructors_and_notnull_return_types_are_marked() {
    let source = "class Foo\n{\n    void ~Foo() {}\n    void F() {}\n    notnull Foo Get() {}\n}\n";
    let methods = &to_json("Scripts/3_Game/foo.c", source)["classes"][0]["methods"];
    assert_eq!(methods[0]["destructor"], true);
    assert_eq!(methods[1]["destructor"], false);
    assert_eq!(methods[2]["destructor"], false);
    assert_eq!(methods[1]["return_notnull"], false);
    assert_eq!(methods[2]["return_notnull"], true);
    assert_eq!(methods[2]["modifiers"], json!(["notnull"]));
}

#[test]
fn field_values_and_array_sizes_are_listed() {
    let source = "class Cfg\n{\n    const string MOD_ID = \"SampleMod\";\n    int a = 1, b[4], c;\n    void F(int x[3], float m[]) {}\n}\n";
    let class = &to_json("Scripts/3_Game/cfg.c", source)["classes"][0];
    let fields = &class["fields"];
    assert_eq!(fields[0]["value"], "\"SampleMod\"");
    assert_eq!(fields[0]["array"], json!([]));
    assert_eq!(fields[1]["value"], "1");
    assert_eq!(fields[1]["array"], json!([]));
    assert_eq!(fields[2]["value"], Value::Null);
    assert_eq!(fields[2]["array"], json!(["4"]));
    assert_eq!(fields[3]["value"], Value::Null);
    assert_eq!(fields[3]["array"], json!([]));
    let params = &class["methods"][0]["params"];
    assert_eq!(params[0]["array"], json!(["3"]));
    assert_eq!(params[1]["array"], json!([""]));
}

#[test]
fn attributes_on_the_line_above_a_field_stay_with_it() {
    let source = "class A\n{\n    int x;\n    [NonSerialized()]\n    int y;\n    int g[2][3];\n    [Obsolete()]\n    int z;\n}\n";
    let fields = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["fields"];
    let attributes: Vec<&Value> = fields
        .as_array()
        .unwrap()
        .iter()
        .map(|field| &field["attributes"])
        .collect();
    assert_eq!(
        attributes,
        vec![
            &json!([]),
            &json!(["NonSerialized()"]),
            &json!([]),
            &json!(["Obsolete()"])
        ]
    );
}

#[test]
fn comments_between_attributes_and_declarations_are_ignored() {
    let source = "[Tag(\"a\" /* first */, 2)]\n// note\nclass A\n{\n    [Hint() /* x */]\n    // about the field\n    int m_Count;\n    [Other()]\n    //! about the method\n    void F() {}\n}\n";
    let class = &to_json("Scripts/3_Game/a.c", source)["classes"][0];
    assert_eq!(class["attributes"], json!(["Tag(\"a\" , 2)"]));
    assert_eq!(class["decl_line"], 3);
    assert_eq!(class["fields"][0]["attributes"], json!(["Hint()"]));
    let method = &class["methods"][0];
    assert_eq!(method["attributes"], json!(["Other()"]));
    assert_eq!(method["decl_line"], 10);
}

#[test]
fn comments_inside_array_brackets_are_not_sizes() {
    let source = "class A\n{\n    int a[/* open */];\n    int b[4 /* four */];\n}\n";
    let fields = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["fields"];
    assert_eq!(fields[0]["array"], json!([""]));
    assert_eq!(fields[1]["array"], json!(["4"]));
}

#[test]
fn a_size_skips_comments_and_strings_around_its_brackets() {
    let cases = [
        ("int a[2 /* ] */];", json!(["2"])),
        ("int b /* x */ [3];", json!(["3"])),
        ("string s[SIZE(\"]\")];", json!(["SIZE(\"]\")"])),
        ("int h[2]\n    [Attr] int z;", json!(["2"])),
    ];
    for (field, array) in cases {
        let source = format!("class A\n{{\n    {field}\n}}\n");
        let fields = &to_json("Scripts/3_Game/a.c", &source)["classes"][0]["fields"];
        assert_eq!(fields[0]["array"], array, "{field}");
    }
}

#[test]
fn a_field_value_leaves_out_comments_inside_it() {
    for (init, value) in [
        ("{ 1, // one\n        2 }", "{ 1, 2 }"),
        ("1 + /* two */ 2", "1 + 2"),
        ("{ 1, 2 } /* after */", "{ 1, 2 }"),
    ] {
        let source = format!("class A\n{{\n    int a[] = {init};\n}}\n");
        let fields = &to_json("Scripts/3_Game/a.c", &source)["classes"][0]["fields"];
        assert_eq!(fields[0]["value"], value, "{init}");
    }
}

#[test]
fn both_sizes_of_a_2d_field_belong_to_it() {
    let source = "class A\n{\n    int g[2][3];\n    int h;\n}\n";
    let fields = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["fields"];
    assert_eq!(fields[0]["name"], "g");
    assert_eq!(fields[0]["array"], json!(["2", "3"]));
    assert_eq!(fields[0]["attributes"], json!([]));
    assert_eq!(fields[1]["name"], "h");
    assert_eq!(fields[1]["array"], json!([]));
    assert_eq!(fields[1]["attributes"], json!([]));
}

#[test]
fn a_size_after_a_space_stays_with_its_field() {
    let source = "class A\n{\n    int h[2] [3];\n    int z;\n}\n";
    let fields = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["fields"];
    assert_eq!(fields[0]["name"], "h");
    assert_eq!(fields[0]["array"], json!(["2", "3"]));
    assert_eq!(fields[1]["name"], "z");
    assert_eq!(fields[1]["array"], json!([]));
    assert_eq!(fields[1]["attributes"], json!([]));

    let source = "class A\n{\n    int h[2];\n    [Attr] int z;\n}\n";
    let fields = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["fields"];
    assert_eq!(fields[1]["name"], "z");
    assert_eq!(fields[1]["attributes"], json!(["Attr"]));
}

#[test]
fn a_bracket_on_the_next_line_is_an_attribute_not_a_second_size() {
    let source = "class A\n{\n    int h[2]\n    [Attr()] int z;\n}\n";
    let fields = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["fields"];
    assert_eq!(fields[0]["name"], "h");
    assert_eq!(fields[0]["array"], json!(["2"]));
    assert_eq!(fields[1]["name"], "z");
    assert_eq!(fields[1]["attributes"], json!(["Attr()"]));
}

#[test]
fn both_sizes_of_a_2d_global_belong_to_it() {
    let value = to_json("Scripts/3_Game/a.c", "int g[2][3];\nint h;\n");
    let globals = &value["globals"];
    assert_eq!(globals[0]["name"], "g");
    assert_eq!(globals[0]["array"], json!(["2", "3"]));
    assert_eq!(globals[0]["attributes"], json!([]));
}

#[test]
fn a_2d_array_keeps_both_sizes_and_its_value() {
    let cases = [
        (
            "class A\n{\n    int g[2][3] = 1;\n}\n",
            "/classes/0/fields",
            json!([{ "name": "g", "array": ["2", "3"] }]),
        ),
        (
            "class A\n{\n    int a[2][3], b;\n}\n",
            "/classes/0/fields",
            json!([{ "name": "a", "array": ["2", "3"] }, { "name": "b", "array": [] }]),
        ),
        (
            "class A\n{\n    int last[2][3];\n}\n",
            "/classes/0/fields",
            json!([{ "name": "last", "array": ["2", "3"] }]),
        ),
        (
            "class A\n{\n    void M()\n    {\n        int c[2][3];\n        int d;\n    }\n}\n",
            "/classes/0/methods/0/locals",
            json!([{ "name": "c", "array": ["2", "3"] }, { "name": "d", "array": [] }]),
        ),
        (
            "int k[2][3];\n",
            "/globals",
            json!([{ "name": "k", "array": ["2", "3"] }]),
        ),
    ];
    for (source, pointer, expected) in cases {
        let value = to_json("Scripts/3_Game/a.c", source);
        let got: Vec<Value> = value
            .pointer(pointer)
            .and_then(Value::as_array)
            .expect(pointer)
            .iter()
            .map(|item| json!({ "name": item["name"], "array": item["array"] }))
            .collect();
        assert_eq!(Value::Array(got), expected, "{source}");
    }

    let value = to_json(
        "Scripts/3_Game/a.c",
        "class A\n{\n    int g[2][3] = 1;\n}\n",
    );
    assert_eq!(value["classes"][0]["fields"][0]["value"], "1");
}

#[test]
fn a_clean_file_lists_no_errors() {
    let value = to_json("Scripts/3_Game/a.c", "class A\n{\n    void F() {}\n}\n");
    assert_eq!(value["errors"], json!([]));
}

#[test]
fn a_class_missing_its_closing_brace_is_one_error() {
    let source = "class A\n{\n    void F() {}\n";
    let value = to_json("Scripts/3_Game/a.c", source);
    let errors = value["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let line = errors[0]["line"].as_u64().unwrap();
    let column = errors[0]["column"].as_u64().unwrap();
    assert!((1..=4).contains(&line), "line {line}");
    assert!(column >= 1, "column {column}");
    assert!(!errors[0]["message"].as_str().unwrap().is_empty());
}

#[test]
fn an_error_in_an_arm_only_the_other_view_reads_is_listed() {
    let source = "#ifdef SERVER\nclass A {}\n#else\n}\n#endif\n";
    let value = to_json("Scripts/3_Game/a.c", source);
    let errors = value["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["line"], 4);
    assert_eq!(errors[0]["column"], 1);
}

#[test]
fn a_missing_token_only_the_other_view_causes_is_listed_at_its_own_line() {
    let cases = [
        (
            "class A\n{\n#ifdef SERVER\n    int x;\n#else\n    void F() {\n#endif\n}\n",
            6,
            15,
        ),
        (
            "class A\n{\n    int y;\n#ifdef SERVER\n    int x;\n#else\n    void F() {\n#endif\n}\n",
            7,
            15,
        ),
    ];
    for (source, line, column) in cases {
        let value = to_json("Scripts/3_Game/a.c", source);
        let errors = value["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0]["line"], line, "{errors:?}");
        assert_eq!(errors[0]["column"], column, "{errors:?}");
        assert_eq!(errors[0]["message"], "missing `}`");
    }
}

#[test]
fn a_missing_token_a_doxygen_arm_causes_is_not_listed() {
    for source in [
        "class A\n{\n#ifdef DOXYGEN\n    void F() {\n#endif\n}\n",
        "class A\n{\n#ifndef DOXYGEN\n    int x;\n#else\n    void F() {\n#endif\n}\n",
    ] {
        let value = to_json("Scripts/3_Game/a.c", source);
        assert_eq!(value["errors"], json!([]), "{source}");
    }
}

#[test]
fn an_error_in_an_arm_neither_view_reads_is_not_listed() {
    let source = "class Hub\n{\n#ifdef DEDICATED\n    #ifdef SERVER\n    void Keep() {}\n    #else\n    }\n    #endif\n#endif\n}\n";
    let value = to_json("Scripts/3_Game/hub.c", source);
    assert_eq!(value["errors"], json!([]));
}

#[test]
fn errors_are_listed_in_source_order() {
    let source = "#ifdef SERVER\nclass A {}\n#else\n}\n#endif\n}\n";
    let value = to_json("Scripts/3_Game/a.c", source);
    let lines: Vec<&Value> = value["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["line"])
        .collect();
    assert_eq!(lines, [&json!(4), &json!(6)]);
}

#[test]
fn spaces_inside_string_literals_are_kept() {
    let source =
        "[Tag(\"a  b\")]\nclass A\n{\n    const string S = \"x   y\" +\n        \"z\";\n}\n";
    let class = &to_json("Scripts/3_Game/a.c", source)["classes"][0];
    assert_eq!(class["attributes"], json!(["Tag(\"a  b\")"]));
    assert_eq!(class["fields"][0]["value"], "\"x   y\" + \"z\"");
}

#[test]
fn a_call_receiver_keeps_spaces_inside_strings() {
    let source =
        "class A\n{\n    void F()\n    {\n        Get(\"a  b\")\n            .Run();\n    }\n}\n";
    let calls = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["methods"][0]["calls"];
    assert_eq!(calls[1]["receiver"], "Get(\"a  b\")");
}

#[test]
fn parenthesised_values_keep_their_parentheses() {
    let source = "class A\n{\n    [(Bar)] static const int FLAGS = (A | B);\n    int b[(N + 1)];\n    void F(int x[(3)], int y = (7)) {}\n}\n";
    let class = &to_json("Scripts/3_Game/a.c", source)["classes"][0];
    let fields = &class["fields"];
    assert_eq!(fields[0]["attributes"], json!(["(Bar)"]));
    assert_eq!(fields[0]["value"], "(A | B)");
    assert_eq!(fields[1]["array"], json!(["(N + 1)"]));
    let params = &class["methods"][0]["params"];
    assert_eq!(params[0]["array"], json!(["(3)"]));
    assert_eq!(params[1]["default"], "(7)");
}

#[test]
fn errors_in_documentation_blocks_are_not_listed() {
    let source = "#ifdef DOXYGEN\n/** @name WidgetType\n */\n///@{\n\n\t//!Single-line text\n\tTextWidgetTypeID,\n#endif\nclass C\n{\n}\n";
    let value = to_json("Scripts/1_Core/a.c", source);
    assert_eq!(value["errors"], json!([]));
}

#[test]
fn call_arguments_and_returns_leave_out_comments_inside_them() {
    let source = "class A\n{\n    int F()\n    {\n        Foo(a /* one */ + b);\n        return a /* two */ + b;\n    }\n}\n";
    let method = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["methods"][0];
    assert_eq!(method["calls"][0]["arg_texts"], json!(["a + b"]));
    assert_eq!(method["returns"][0]["text"], "a + b");
}

#[test]
fn call_arguments_keep_their_text_across_lines() {
    let source = "class A\n{\n    void F()\n    {\n        Print(\"a\", x + 1);\n        Foo(v: 2,\n            a +\n            b);\n    }\n}\n";
    let calls = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["methods"][0]["calls"];
    assert_eq!(calls[0]["arg_texts"], json!(["\"a\"", "x + 1"]));
    assert_eq!(calls[1]["arg_texts"], json!(["v: 2", "a + b"]));
}

#[test]
fn return_expressions_are_listed_in_source_order() {
    let source = "class A\n{\n    int F()\n    {\n        return 3;\n        return;\n        return GetGame().GetTime();\n        return (a + b);\n        return\n    }\n}\n";
    let method = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["methods"][0];
    assert_eq!(
        method["returns"],
        json!([
            {"text": "3", "line": 5},
            {"text": null, "line": 6},
            {"text": "GetGame().GetTime()", "line": 7},
            {"text": "(a + b)", "line": 8},
            {"text": null, "line": 9}
        ])
    );
}

#[test]
fn locals_and_type_names_in_a_body_are_listed() {
    let source = "class A\n{\n    void F()\n    {\n        int a = 1, b;\n        auto c = new Foo();\n        ref array<ref Bar> d;\n        Baz e = Baz.Cast(f);\n        Print((Qux) g, new Foo());\n    }\n}\n";
    let method = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["methods"][0];
    assert_eq!(
        method["locals"],
        json!([
            {"name": "a", "type": "int", "line": 5, "array": []},
            {"name": "b", "type": "int", "line": 5, "array": []},
            {"name": "c", "type": "auto", "line": 6, "array": []},
            {"name": "d", "type": "ref array<ref Bar>", "line": 7, "array": []},
            {"name": "e", "type": "Baz", "line": 8, "array": []}
        ])
    );
    assert_eq!(
        method["types"],
        json!(["Bar", "Baz", "Foo", "Qux", "array"])
    );
}

#[test]
fn an_arm_only_the_other_view_reads_contributes_returns_locals_and_types() {
    let source = "class A\n{\n    int F()\n    {\n#ifdef SERVER\n        return 1;\n#else\n        Bar b;\n        return 2;\n#endif\n    }\n}\n";
    let method = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["methods"][0];
    assert_eq!(
        method["returns"],
        json!([{"text": "1", "line": 6}, {"text": "2", "line": 9}])
    );
    assert_eq!(
        method["locals"],
        json!([{"name": "b", "type": "Bar", "line": 8, "array": []}])
    );
    assert_eq!(method["types"], json!(["Bar"]));
}

#[test]
fn a_local_carries_its_array_sizes() {
    let source = "class A\n{\n    void F()\n    {\n        int a[3];\n        int b;\n        int c[];\n        foreach (int d : x) {}\n    }\n}\n";
    let method = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["methods"][0];
    let sizes: Vec<&Value> = method["locals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|local| &local["array"])
        .collect();
    assert_eq!(
        sizes,
        vec![&json!(["3"]), &json!([]), &json!([""]), &json!([])]
    );
}

#[test]
fn foreach_variables_are_locals_and_their_types_are_listed() {
    let source = "class A\n{\n    void F(array<Baz> x)\n    {\n        foreach (int i, Baz v : x)\n            Print(v);\n        foreach (auto w : x) {}\n    }\n}\n";
    let method = &to_json("Scripts/3_Game/a.c", source)["classes"][0]["methods"][0];
    assert_eq!(
        method["locals"],
        json!([
            {"name": "i", "type": "int", "line": 5, "array": []},
            {"name": "v", "type": "Baz", "line": 5, "array": []},
            {"name": "w", "type": "auto", "line": 7, "array": []}
        ])
    );
    assert_eq!(method["types"], json!(["Baz"]));
}

#[test]
fn comments_between_sibling_nodes_stay_out_of_outline_texts() {
    let source = "class A\n{\n    [Tag(1 /* c */ + 2)]\n    int t;\n    int F(int d = ( /* z */ 2 ))\n    {\n        return ( /* a */ 1 );\n    }\n}\n";
    let class = &to_json("Scripts/3_Game/a.c", source)["classes"][0];
    assert_eq!(class["fields"][0]["attributes"], json!(["Tag(1 + 2)"]));
    let method = &class["methods"][0];
    assert_eq!(method["params"][0]["default"], "( 2 )");
    assert_eq!(method["returns"][0]["text"], "( 1 )");
}
