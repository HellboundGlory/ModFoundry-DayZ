use super::BodyChecker;

impl BodyChecker<'_> {
    // A name passed where a `func` is wanted that no method answers to. Every
    // `func` parameter the scripts declare is untyped, so the rows that compare
    // a method with a prototype have nothing to compare against.
    pub(super) fn unknown_callback(&mut self, name: &str) {
        self.soft(format!("Can't make callback from unknown method '{name}'"));
        self.soft("Failed to create callback".into());
        // As warnings the two rows let the name be read as a variable, which
        // it is not either.
        if !self.strict {
            self.error(format!("Can't find variable '{name}'"));
        }
    }

    // A method of the class being read, named where a `func` is wanted. False
    // when the game could not build the callback.
    pub(super) fn method_callback(&mut self, name: &str) -> bool {
        let Some(class) = self.class.clone() else {
            return true;
        };
        let methods = self.world.methods_of(&class, name);
        if methods.is_empty() {
            return true;
        }
        if self.static_method && !methods.iter().any(|method| method.is_static()) {
            // A static method has no instance to call it on.
            let reason = format!("Can't make callback from non-static function '{name}' without instance.");
            self.soft(reason.clone());
            if !self.strict {
                self.error(reason);
            }
            self.soft("Failed to create callback".into());
            if !self.strict {
                self.error(format!("Can't find variable '{name}'"));
            }
            return false;
        }
        if methods.len() > 1 {
            self.soft(format!("Callbacks do not properly support overloaded methods '{name}'"));
        }
        true
    }
}
