use crate::index::{dimension_lines, Dim};
use crate::parser::VarDecl;

use super::scope::base_name;
use super::BodyChecker;

impl BodyChecker<'_> {
    // The rules a local declaration is held to before its type and its
    // initialiser are read. False when the game gives up on the declaration.
    pub(super) fn declaration_rules(&mut self, decl: &VarDecl) -> bool {
        let has = |word: &str| decl.modifiers.iter().any(|modifier| modifier == word);
        if has("out") || has("inout") || has("notnull") {
            self.error("inout/out/notnull can be used only on function parameters".into());
            return false;
        }
        let type_name = decl.type_.as_ref().map(|type_| base_name(&type_.name)).unwrap_or_default();
        // `Type name(args)` builds the class it names, and `void` is none.
        if decl.ctor_args.is_some() && type_name == "void" {
            self.error("Can't find class void".into());
            return false;
        }
        if type_name == "auto" {
            if has("static") || has("const") {
                self.error("static/const variables can't be auto".into());
                return false;
            }
            // `auto` has nothing to take its type from.
            if decl.init.is_none() {
                self.error("Syntax error".into());
                self.error("Unknown type 'auto'".into());
                self.error("Error in parameters".into());
                return false;
            }
        }
        // A type the game cannot resolve ends the declaration before its name
        // is looked at.
        let resolves = decl.type_.is_none()
            || super::is_primitive(type_name)
            || self.world.types.is_known(type_name)
            || self.is_template_param(type_name);
        if resolves {
            self.name_rules(&decl.name);
        }
        self.dimension_rules(decl, type_name);
        true
    }

    // A function's locals share one namespace with its parameters, the globals
    // and the class's static members; an instance member only draws a warning,
    // and an enum value none at all.
    fn name_rules(&mut self, name: &str) {
        if self.world.types.blocks_variable_name(name) {
            self.error(format!("Variable name '{name}' already used as type name"));
            return;
        }
        let member = self.class.as_deref().and_then(|class| self.world.member_of(class, name));
        let shared = member.is_some_and(|member| member.has("static") || member.has("const"));
        if self.locals.find(name).is_some() || self.world.module_variables.contains(name) || shared {
            self.error(format!("Multiple declaration of variable '{name}'"));
        } else if member.is_some() {
            self.warning(format!("Possible variable name conflict '{name}'"));
        }
    }

    fn dimension_rules(&mut self, decl: &VarDecl, type_name: &str) {
        let dims: Vec<Dim> = decl.dims.iter().map(Dim::of).collect();
        // A local `const` sizes an array too, to a value this pass does not
        // compute.
        let local_constant = dims.iter().any(|dim| {
            matches!(dim, Dim::Name(name) if self.locals.find(name).is_some_and(|local| local.has("const")))
        });
        if local_constant {
            return;
        }
        for line in dimension_lines(&self.world.constants, &self.world.types, &decl.name, &dims, type_name) {
            self.soft_or_error(line.soft, line.message);
        }
    }
}
