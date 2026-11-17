use crate::parser::{Expr, ExprKind, VarDecl};

use super::conversions::Kind;
use super::scope::{Member, Ty};
use super::BodyChecker;

impl BodyChecker<'_> {
    // A freshly built object is kept alive by a strong reference only. Locals
    // hold one by themselves; a member, a static member or a global holds one
    // when it says so.
    pub(super) fn strong_ref(&mut self, place: &Expr, value: &Expr) {
        let Some(hint) = self.fresh_object(value) else {
            return;
        };
        let Some(holder) = self.holder(place) else {
            return;
        };
        if !self.holds_object(&holder.ty) {
            return;
        }
        if !holder.has("ref") && !holder.has("autoptr") {
            self.error(format!("Variable '{}' is not strong ref (missing {hint}) !", holder.name));
        }
    }

    // A member or a global built where it is declared is held to the same
    // rule. True when the game goes on to refuse the value as well, which it
    // does for a global.
    pub(super) fn strong_ref_initialiser(&mut self, decl: &VarDecl, init: &Expr, ty: &Ty) -> bool {
        let Some(hint) = self.fresh_object(init) else {
            return false;
        };
        let strong = decl.modifiers.iter().any(|modifier| modifier == "ref" || modifier == "autoptr");
        if strong || !self.holds_object(ty) {
            return false;
        }
        self.error(format!("Variable '{}' is not strong ref (missing {hint}) !", decl.name));
        self.class.is_none()
    }

    // Only a class is kept by reference: a static array or a vector is plain
    // data, filled in place.
    fn holds_object(&self, ty: &Ty) -> bool {
        !matches!(ty, Ty::Array(_)) && matches!(self.kind(ty), Kind::Class(_) | Kind::Unknown)
    }

    // The word the game suggests when a value is an object built on the spot
    // that something must keep alive.
    fn fresh_object(&self, value: &Expr) -> Option<&'static str> {
        match &value.kind {
            ExprKind::New { type_, .. } => {
                let counted = type_.as_ref().is_none_or(|type_| self.is_counted(&type_.name));
                counted.then_some("'ref'?")
            }
            ExprKind::List { .. } => Some("autoptr?"),
            _ => None,
        }
    }

    // Whether instances of a class are reference counted. A class built on
    // `Managed` is, and so is a template. A class with a plain 1_Core or
    // 2_GameLib class among its ancestors is not (Community Framework's event
    // arguments), nor is the game object, which vanilla builds into a plain
    // global.
    fn is_counted(&self, class: &str) -> bool {
        let chain = self.world.chain(class);
        if chain.is_empty() || chain.iter().any(|info| info.name == "CGame") {
            return false;
        }
        chain.iter().any(|info| info.name == "Managed")
            || !chain.iter().any(|info| info.early && info.template.is_empty() && info.name != "Class")
    }

    // The member, global or parameter a place names, if it is one that must
    // say so to keep an object.
    fn holder(&mut self, place: &Expr) -> Option<Member> {
        match &place.kind {
            ExprKind::Leaf { text } => {
                // A local keeps what it is given. A parameter does not, unless
                // it hands the object back to the caller.
                if let Some(local) = self.locals.find(text) {
                    let kept = self.locals.declared_in_body(text) || local.has("out") || local.has("inout");
                    return (!kept).then(|| local.clone());
                }
                let member = self.class.as_deref().and_then(|class| self.world.member_of(class, text));
                let global = || self.world.module_variables.contains(text).then(|| self.world.globals.get(text)).flatten();
                member.or_else(global).cloned()
            }
            ExprKind::Member { object, name } => {
                let object_ty = self.quiet(|checker| checker.typed_in(object, None));
                let class = object_ty.base()?;
                self.world.member_of(class, name).cloned()
            }
            _ => None,
        }
    }
}
