use std::collections::HashMap;

use crate::diag::{Diagnostic, Severity};
use crate::input::Module;

use super::model::{holder, ClassModel, EnumModel};

// The merge runs at the end of parsing a module, in file load order.
// It looks the name up among the module's own declarations, renames that one to
// `X#<file>#<line>` after the wrapping declaration (its file's place among the
// module's files, counted from 0, and its line), gives the plain name to the
// modded class and makes the modded class derive from the renamed original, so
// the next modded class wraps this one. A member variable the original (or a same-module ancestor) already has is
// removed from the modded class and its initialiser replaces the original's, so
// no declaration is made for it.
//
// A second declaration of a name that already has one in this module is caught
// here, before the merge, and the later one is dropped.
pub struct Merged {
    pub classes: Vec<ClassModel>,
    pub enums: Vec<EnumModel>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Merged {
    pub fn of(classes: Vec<ClassModel>, enums: Vec<EnumModel>) -> Merged {
        let mut diagnostics = Vec::new();
        let mut classes = duplicates(
            classes,
            "class",
            |class: &ClassModel| !class.modded && !class.forward,
            &mut diagnostics,
        );
        let mut enums = duplicates(enums, "enum", |e: &EnumModel| !e.modded, &mut diagnostics);
        for index in 0..classes.len() {
            if !classes[index].modded || classes[index].forward {
                continue;
            }
            let name = classes[index].name.clone();
            // The plain name is held by a plain class at first, then by the wrap
            // of the previous pass, so a second modded class wraps the first.
            let target = classes.iter().enumerate().position(|(at, candidate)| {
                at != index
                    && candidate.name == name
                    && !candidate.forward
                    && (!candidate.modded || candidate.wrapped.is_some())
            });
            let Some(target) = target else {
                let entry = &classes[index];
                diagnostics.push(unknown(&entry.file, entry.line, entry.module, &name));
                continue;
            };
            if classes[target].sealed {
                let entry = &classes[index];
                diagnostics.push(sealed(&entry.file, entry.line, entry.module, &name));
                classes[index].wrapped = Some((name.clone(), false));
                continue;
            }
            let engine = classes[target].is_engine();
            let inherited = inherited_names(&classes, target);
            let mut kept = Vec::new();
            let mut moved = Vec::new();
            for member in std::mem::take(&mut classes[index].members) {
                if inherited.contains(&member.name) {
                    moved.push(member);
                } else {
                    kept.push(member);
                }
            }
            for member in moved {
                // The member the redeclaration replaces may be the target's own
                // or one an ancestor declared in this module, and a second wrap
                // reaches it through the class the first one wrapped.
                let Some((owner, at)) = member_owner(&classes, target, &member.name) else {
                    continue;
                };
                let entry = super::model::Override {
                    name: member.name.clone(),
                    type_name: member.base_type().map(str::to_string),
                    original_type: classes[owner].members[at].base_type().map(str::to_string),
                    file: classes[owner].file.clone(),
                    line: classes[owner].members[at].line,
                    module: classes[owner].module,
                };
                classes[target].overrides.push(entry);
                let original = &mut classes[owner].members[at];
                original.has_init = member.has_init;
                original.init = member.init.clone();
            }
            classes[index].members = kept;
            let renamed = format!("{name}#{}#{}", classes[index].position, classes[index].line);
            classes[target].replaced = true;
            classes[target].name = renamed.clone();
            // The original's constructor and destructor are rewritten to the new
            // name, so they stay that class's constructor and destructor.
            for function in &mut classes[target].functions {
                // A destructor's name is stored without the `~`.
                if function.name == name {
                    function.name = renamed.clone();
                }
            }
            classes[index].base = Some(renamed);
            classes[index].wrapped = Some((name, engine));
        }
        for index in 0..enums.len() {
            if !enums[index].modded {
                continue;
            }
            let name = enums[index].name.clone();
            let target = enums.iter().enumerate().position(|(at, candidate)| {
                at != index && candidate.name == name && (!candidate.modded || candidate.wrapped.is_some())
            });
            let Some(target) = target else {
                let entry = &enums[index];
                diagnostics.push(unknown(&entry.file, entry.line, entry.module, &name));
                continue;
            };
            if enums[target].sealed {
                let entry = &enums[index];
                diagnostics.push(sealed(&entry.file, entry.line, entry.module, &name));
                enums[index].wrapped = Some(name);
                continue;
            }
            let renamed = format!("{name}#{}#{}", enums[index].position, enums[index].line);
            enums[target].replaced = true;
            enums[target].name = renamed.clone();
            enums[index].base = Some(renamed);
            enums[index].wrapped = Some(name);
        }
        Merged {
            classes,
            enums,
            diagnostics,
        }
    }
}

// The module's first declaration of a name keeps it; every later one is dropped
// with `Multiple declaration of <kind> '<name>'` and `Previously declared here`.
// A `modded` or forward declaration takes part in neither.
fn duplicates<T: Named>(
    entries: Vec<T>,
    kind: &str,
    unique: impl Fn(&T) -> bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<T> {
    let mut first: HashMap<String, usize> = HashMap::new();
    let mut kept: Vec<T> = Vec::with_capacity(entries.len());
    for entry in entries {
        if !unique(&entry) {
            kept.push(entry);
            continue;
        }
        let name = entry.name().to_string();
        match first.get(&name) {
            Some(&at) => diagnostics.push(Diagnostic {
                file: entry.file().to_string(),
                line: entry.line(),
                module: entry.module(),
                severity: Severity::Error,
                message: format!("Multiple declaration of {kind} '{name}'"),
                fixme: false,
                game_crash: false,
                reports_only: false,
                related: Some((kept[at].file().to_string(), kept[at].line())),
            }),
            None => {
                first.insert(name, kept.len());
                kept.push(entry);
            }
        }
    }
    kept
}

trait Named {
    fn name(&self) -> &str;
    fn file(&self) -> &str;
    fn line(&self) -> u32;
    fn module(&self) -> Module;
}

impl Named for ClassModel {
    fn name(&self) -> &str {
        &self.name
    }

    fn file(&self) -> &str {
        &self.file
    }

    fn line(&self) -> u32 {
        self.line
    }

    fn module(&self) -> Module {
        self.module
    }
}

impl Named for EnumModel {
    fn name(&self) -> &str {
        &self.name
    }

    fn file(&self) -> &str {
        &self.file
    }

    fn line(&self) -> u32 {
        self.line
    }

    fn module(&self) -> Module {
        self.module
    }
}

fn diagnostic(file: &str, line: u32, module: Module, message: String) -> Diagnostic {
    Diagnostic {
        file: file.to_string(),
        line,
        module,
        severity: Severity::Error,
        message,
        fixme: false,
        game_crash: false,
        reports_only: false,
        related: None,
    }
}

fn sealed(file: &str, line: u32, module: Module, name: &str) -> Diagnostic {
    diagnostic(file, line, module, format!("Sealed type '{name}' can't be modded"))
}

fn unknown(file: &str, line: u32, module: Module, name: &str) -> Diagnostic {
    diagnostic(file, line, module, format!("Unknown type '{name}'"))
}

// The class in the wrapping chain that declares this member and where in its
// member list, starting at the target: a second wrap reaches the member through
// the class the first one wrapped, and an ancestor declares it too.
fn member_owner(classes: &[ClassModel], target: usize, name: &str) -> Option<(usize, usize)> {
    let mut current = Some(target);
    let mut seen = Vec::new();
    while let Some(at) = current {
        if seen.contains(&at) {
            return None;
        }
        seen.push(at);
        if let Some(member) = classes[at].members.iter().position(|member| member.name == name) {
            return Some((at, member));
        }
        current = classes[at]
            .base_name()
            .and_then(|base| classes.iter().position(|candidate| candidate.name == base));
    }
    None
}

// The member names of a class and its same-module ancestors, nearest first.
fn inherited_names(classes: &[ClassModel], target: usize) -> Vec<String> {
    let mut names = classes[target].member_names();
    let mut seen = vec![classes[target].name.clone()];
    let mut base = classes[target].base_name().map(str::to_string);
    while let Some(name) = base {
        if seen.contains(&name) {
            break;
        }
        seen.push(name.clone());
        let Some(parent) = holder(classes, &name) else {
            break;
        };
        names.extend(parent.member_names());
        base = parent.base_name().map(str::to_string);
    }
    names
}
