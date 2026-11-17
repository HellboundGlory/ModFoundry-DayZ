//! Shared fixtures and helpers for the `edit_*` test files (`#[path]`-included).
#![allow(dead_code)]

use modfoundry_config::ast::Value;
use modfoundry_config::edit::*;

pub fn to_new(value: &Value) -> Option<NewValue> {
    Some(match value {
        Value::Str(v) => NewValue::Str(v.clone()),
        Value::Number(v) => NewValue::Number(v.clone()),
        Value::Ident(v) => NewValue::Ident(v.clone()),
        Value::Array(items) => NewValue::Array(items.iter().map(to_new).collect::<Option<_>>()?),
        Value::Float(_) | Value::Other(_) => return None,
    })
}

pub fn edit(text: &str, op: &EditOp) -> String {
    let edits = plan(text, op).unwrap_or_else(|e| panic!("{e}: {op:?}"));
    check_postcondition(text, op, &edits).unwrap_or_else(|m| panic!("{m}: {op:?}"));
    apply(text, &edits)
}

pub fn s(v: &str) -> NewValue {
    NewValue::Str(v.into())
}

pub fn n(v: &str) -> NewValue {
    NewValue::Number(v.into())
}

pub fn id(v: &str) -> NewValue {
    NewValue::Ident(v.into())
}

pub fn arr(items: Vec<NewValue>) -> NewValue {
    NewValue::Array(items)
}

pub fn path(names: &[&str]) -> ConfigPath {
    ConfigPath::of(names)
}

pub fn prop(class: &[&str], name: &str) -> Target {
    Target::Property {
        class: path(class),
        name: name.into(),
    }
}

pub fn class(names: &[&str]) -> Target {
    Target::Class(path(names))
}

pub fn set(class_path: &[&str], name: &str, value: NewValue) -> EditOp {
    EditOp::SetValue {
        property: prop(class_path, name),
        value,
    }
}

pub fn add(class_path: &[&str], name: &str, value: NewValue, position: Position) -> EditOp {
    EditOp::AddProperty {
        class: path(class_path),
        name: name.into(),
        value,
        position,
    }
}

pub fn add_class(
    parent: &[&str],
    name: &str,
    base: Option<&str>,
    forward: bool,
    position: Position,
) -> EditOp {
    EditOp::AddClass {
        parent: path(parent),
        name: name.into(),
        base: base.map(str::to_owned),
        forward,
        position,
    }
}

pub fn remove(target: Target) -> EditOp {
    EditOp::Remove { target }
}

pub fn rename(target: Target, new_name: &str) -> EditOp {
    EditOp::Rename {
        target,
        new_name: new_name.into(),
    }
}

pub fn set_base(class_path: &[&str], base: Option<&str>) -> EditOp {
    EditOp::SetBase {
        class: path(class_path),
        base: base.map(str::to_owned),
    }
}

pub fn insert(class_path: &[&str], name: &str, index: usize, value: NewValue) -> EditOp {
    EditOp::ArrayInsert {
        property: prop(class_path, name),
        index,
        value,
    }
}

pub fn remove_item(class_path: &[&str], name: &str, index: usize) -> EditOp {
    EditOp::ArrayRemove {
        property: prop(class_path, name),
        index,
    }
}

pub fn set_item(class_path: &[&str], name: &str, index: usize, value: NewValue) -> EditOp {
    EditOp::ArraySet {
        property: prop(class_path, name),
        index,
        value,
    }
}

pub fn move_item(class_path: &[&str], name: &str, from: usize, to: usize) -> EditOp {
    EditOp::ArrayMove {
        property: prop(class_path, name),
        from,
        to,
    }
}

pub fn crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

/// Re-indents a 4-space canonical text and optionally switches to CRLF.
pub fn restyle(text: &str, indent: &str, use_crlf: bool) -> String {
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let body = line.trim_start_matches(' ');
            let depth = (line.len() - body.len()) / 4;
            format!("{}{}", indent.repeat(depth), body)
        })
        .collect();
    let joined = lines.join("\n");
    if use_crlf {
        crlf(&joined)
    } else {
        joined
    }
}

pub const CANON: &str = "class CfgPatches
{
    class MyMod
    {
        units[] = {};
        requiredVersion = 0.1;
        requiredAddons[] = { \"DZ_Data\" };
    };
};
class CfgVehicles
{
    class Base;
    class Item: Base
    {
        // display
        displayName = \"Item\";
        scope = 2;
        itemSize[] = { 1, 2 };
        class Sub
        {
            x = 1;
        };
    };
};
";

pub const ITEM: [&str; 2] = ["CfgVehicles", "Item"];
pub const MYMOD: [&str; 2] = ["CfgPatches", "MyMod"];

pub const KNR: &str = "class CfgVehicles {
    class Base;
    class Item: Base {
        scope = 2;
    };
};
";

pub const ONE_LINE: &str = "class A { x = 1; y[] = { 1, 2 }; };
class B { };
class C {};
";

pub const VANILLA_LF: &str = "class CfgPatches
{
\tclass MyMod
\t{
\t\tunits[]={};
\t\trequiredVersion=0.1;
\t\trequiredAddons[]=
\t\t{
\t\t\t\"DZ_Data\",
\t\t\t\"DZ_Gear\"
\t\t};
\t};
};
class CfgVehicles
{
\tclass Base;
\tclass Item: Base
\t{
\t\tscope=2;
\t\tdisplayName=\"Item\";
\t\titemSize[]={2,2};
\t};
};
";

pub const COMMENTS: &str = "class A
{
    #ifdef X
    a = 1; // one
    #endif
    /* block */
    b = 2;
    c[] = {
        \"x\", // first
        \"y\"
    };
};
";

pub const ARRAYS: &str = "class A
{
    trail[] = { \"a\", \"b\", };
    tight[] = {1,2};
    plus[] += { 1, 2 };
    nested[] = { { 1, 2 }, { 3 } };
    lines[] = {
        \"a\",
        \"b\",
    };
    none[] = {};
};
";

pub fn scaffold_config() -> String {
    modfoundry_config::mod_skeleton(&modfoundry_config::ScaffoldOptions::new("MyMod"))
        .expect("skeleton")
        .into_iter()
        .find(|f| f.path == "config.cpp")
        .expect("config.cpp")
        .contents
}
