//! One game build's Enforce declarations, merged across files in load order.

use crate::short_hash;
use modfoundry_script::outline::{
    Arm, ClassOutline, EnumOutline, FieldOutline, FileOutline, GuardOutline, MethodOutline,
    ParamOutline, TypedefOutline,
};
use modfoundry_script::{outline, Layer};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Every declaration one build's script files add up to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptModel {
    pub classes: BTreeMap<String, ClassModel>,
    pub enums: BTreeMap<String, EnumModel>,
    pub typedefs: BTreeMap<String, TypedefModel>,
    pub functions: BTreeMap<String, MethodModel>,
    pub globals: BTreeMap<String, FieldModel>,
    /// Paths whose outline has syntax errors, sorted.
    pub error_files: Vec<String>,
}

/// A file and 1-based line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub file: String,
    pub line: usize,
}

/// A class as the build loads it, vanilla declaration plus every `modded` part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassModel {
    pub name: String,
    pub base: Option<String>,
    /// The plain declaration; `None` when only `modded` parts were seen.
    pub place: Option<Place>,
    pub guards: Vec<String>,
    /// Each `modded class` part, in load order.
    pub modded_in: Vec<Place>,
    pub methods: BTreeMap<String, MethodModel>,
    pub fields: BTreeMap<String, FieldModel>,
}

/// A method or a free function with its signature parts and body hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodModel {
    pub name: String,
    pub destructor: bool,
    pub guards: Vec<String>,
    pub ret: String,
    pub modifiers: Vec<String>,
    pub params: Vec<ParamModel>,
    pub body: String,
    pub place: Place,
}

/// One formal parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamModel {
    pub name: String,
    pub ty: String,
    pub modifiers: Vec<String>,
    pub default: Option<String>,
    pub array: Vec<String>,
}

/// A class field or a global.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldModel {
    pub name: String,
    pub guards: Vec<String>,
    pub ty: String,
    pub modifiers: Vec<String>,
    pub value: Option<String>,
    pub array: Vec<String>,
    pub place: Place,
}

/// An enum and its member names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumModel {
    pub name: String,
    pub guards: Vec<String>,
    pub base: Option<String>,
    pub members: Vec<String>,
    /// One per member, `None` where it has no `= ...`.
    pub values: Vec<Option<String>>,
    pub place: Place,
}

/// A typedef and the type it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedefModel {
    pub name: String,
    pub guards: Vec<String>,
    pub target: String,
    pub place: Place,
}

/// The condition under which guarded code is compiled in.
pub fn condition(guard: &GuardOutline) -> String {
    if (guard.arm == Arm::Then) != guard.negated {
        guard.define.clone()
    } else {
        format!("!{}", guard.define)
    }
}

/// The key a member is stored under: its name, plus its conditions when it has any.
pub fn member_key(name: &str, guards: &[String]) -> String {
    if guards.is_empty() {
        name.to_owned()
    } else {
        format!("{name} [{}]", guards.join(" && "))
    }
}

/// The key a class member is stored under: its conditions beyond the class's own.
pub fn class_member_key(name: &str, guards: &[String], class_guards: &[String]) -> String {
    member_key(name, guards.strip_prefix(class_guards).unwrap_or(guards))
}

/// The key a method is stored under: overloads and the destructor each get their own.
pub fn method_key(method: &MethodModel, class_guards: &[String]) -> String {
    class_member_key(&method.head(), &method.guards, class_guards)
}

impl MethodModel {
    /// The method's name and parameter types, `~` first for a destructor.
    pub fn head(&self) -> String {
        method_head(
            &self.name,
            self.destructor,
            self.params
                .iter()
                .map(|param| (param.ty.as_str(), param.array.as_slice())),
        )
    }
}

/// A method's head: its name and parameter types, with `~` first for a destructor.
pub fn method_head<'a>(
    name: &str,
    destructor: bool,
    params: impl IntoIterator<Item = (&'a str, &'a [String])>,
) -> String {
    let params: Vec<String> = params
        .into_iter()
        .map(|(ty, array)| format!("{ty}{}", dims(array)))
        .collect();
    let tilde = if destructor { "~" } else { "" };
    format!("{tilde}{name}({})", params.join(", "))
}

pub(crate) fn dims(array: &[String]) -> String {
    array.iter().map(|size| format!("[{size}]")).collect()
}

/// A short hash of the code from the first `{` of the lines on, comments and spacing removed.
pub fn body_hash(source: &str, lines: [usize; 2]) -> String {
    let text: Vec<&str> = source
        .lines()
        .skip(lines[0].saturating_sub(1))
        .take(lines[1].saturating_sub(lines[0]) + 1)
        .collect();
    let stripped = strip_comments(&text.join("\n"));
    let body = stripped.find('{').map_or("", |start| &stripped[start..]);
    let squeezed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    short_hash(squeezed.as_bytes())
}

/// The model of every file's declarations, merged in the order the game loads them.
pub fn script_model(files: &[(String, String)]) -> ScriptModel {
    let mut ordered: Vec<&(String, String)> = files.iter().collect();
    ordered.sort_by_key(|(path, _)| (load_ordinal(path), path));

    let outlines: Vec<_> = ordered
        .into_iter()
        .map(|(path, source)| (path, source, outline(path, source)))
        .collect();
    let class_guards = plain_class_guards(&outlines);

    let mut model = ScriptModel::default();
    for (path, source, file) in &outlines {
        if !file.errors.is_empty() {
            model.error_files.push((*path).clone());
        }
        for class in &file.classes {
            let guards = class_guards.get(&class.name).map_or(&[][..], Vec::as_slice);
            merge_class(&mut model, path, source, class, guards);
        }
        for item in &file.enums {
            let model_item = enum_model(path, item);
            model
                .enums
                .insert(member_key(&item.name, &model_item.guards), model_item);
        }
        for item in &file.typedefs {
            let model_item = typedef_model(path, item);
            model
                .typedefs
                .insert(member_key(&item.name, &model_item.guards), model_item);
        }
        for item in &file.functions {
            let model_item = method_model(path, source, item);
            model
                .functions
                .insert(method_key(&model_item, &[]), model_item);
        }
        for item in &file.globals {
            let model_item = field_model(path, item);
            model
                .globals
                .insert(member_key(&item.name, &model_item.guards), model_item);
        }
    }
    model.error_files.sort();
    model.error_files.dedup();
    model
}

/// Each class's conditions as its first plain declaration in load order has them.
fn plain_class_guards(
    outlines: &[(&String, &String, FileOutline)],
) -> HashMap<String, Vec<String>> {
    let mut guards = HashMap::new();
    for (_, _, file) in outlines {
        for class in file.classes.iter().filter(|class| !class.modded) {
            guards
                .entry(class.name.clone())
                .or_insert_with(|| conditions(&class.guards));
        }
    }
    guards
}

fn merge_class(
    model: &mut ScriptModel,
    path: &str,
    source: &str,
    class: &ClassOutline,
    class_guards: &[String],
) {
    let place = Place {
        file: path.to_owned(),
        line: class.lines[0],
    };
    let entry = model
        .classes
        .entry(class.name.clone())
        .or_insert_with(|| ClassModel {
            name: class.name.clone(),
            base: None,
            place: None,
            guards: Vec::new(),
            modded_in: Vec::new(),
            methods: BTreeMap::new(),
            fields: BTreeMap::new(),
        });
    if class.modded {
        entry.modded_in.push(place);
    } else if entry.place.is_none() {
        entry.base = class.base.clone();
        entry.place = Some(place);
        entry.guards = conditions(&class.guards);
    }
    // Member guards repeat the class's own; keys drop those so every part of the class lines up.
    for item in &class.methods {
        let model_item = method_model(path, source, item);
        let key = method_key(&model_item, class_guards);
        entry.methods.insert(key, model_item);
    }
    for item in &class.fields {
        let model_item = field_model(path, item);
        let key = class_member_key(&item.name, &model_item.guards, class_guards);
        entry.fields.insert(key, model_item);
    }
}

fn method_model(path: &str, source: &str, item: &MethodOutline) -> MethodModel {
    MethodModel {
        name: item.name.clone(),
        destructor: item.destructor,
        guards: conditions(&item.guards),
        ret: item.ret.clone(),
        modifiers: item.modifiers.clone(),
        params: item.params.iter().map(param_model).collect(),
        body: body_hash(source, item.lines),
        place: Place {
            file: path.to_owned(),
            line: item.lines[0],
        },
    }
}

fn param_model(item: &ParamOutline) -> ParamModel {
    ParamModel {
        name: item.name.clone(),
        ty: item.ty.clone(),
        modifiers: item.modifiers.clone(),
        default: item.default.clone(),
        array: item.array.clone(),
    }
}

fn field_model(path: &str, item: &FieldOutline) -> FieldModel {
    FieldModel {
        name: item.name.clone(),
        guards: conditions(&item.guards),
        ty: item.ty.clone(),
        modifiers: item.modifiers.clone(),
        value: item.value.clone(),
        array: item.array.clone(),
        place: Place {
            file: path.to_owned(),
            line: item.line,
        },
    }
}

fn enum_model(path: &str, item: &EnumOutline) -> EnumModel {
    EnumModel {
        name: item.name.clone(),
        guards: conditions(&item.guards),
        base: item.base.clone(),
        members: item.members.clone(),
        values: item.values.clone(),
        place: Place {
            file: path.to_owned(),
            line: item.line,
        },
    }
}

fn typedef_model(path: &str, item: &TypedefOutline) -> TypedefModel {
    TypedefModel {
        name: item.name.clone(),
        guards: conditions(&item.guards),
        target: item.target.clone(),
        place: Place {
            file: path.to_owned(),
            line: item.line,
        },
    }
}

fn conditions(guards: &[GuardOutline]) -> Vec<String> {
    guards.iter().map(condition).collect()
}

/// A module layer's place in load order; files outside every module load last.
fn load_ordinal(path: &str) -> u8 {
    Layer::from_path(path).map_or(u8::MAX, |layer| layer as u8)
}

fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut copied = 0;
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            if byte == b'\\' {
                i += 2;
                continue;
            }
            in_string = byte != b'"' && byte != b'\n';
            i += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                out.push_str(&text[copied..i]);
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                copied = i;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.push_str(&text[copied..i]);
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                out.push(' ');
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}
