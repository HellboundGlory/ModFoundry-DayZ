use crate::parser::Expr;

use super::{literal_int, BodyChecker};

impl BodyChecker<'_> {
    // The game folds a division of two literals while compiling and has no
    // guard for a zero divisor.
    pub(super) fn divides_by_zero(&mut self, op: &str, left: &Expr, right: &Expr) -> bool {
        // Two whole numbers: a float on the left makes it a float division.
        let crashes = op == "/" && literal_int(left).is_some() && literal_int(right) == Some(0);
        if crashes {
            self.crash("Division by zero in a constant expression".into());
        }
        crashes
    }

    // The game has no label to jump to: `name:` does not parse, so a `goto`
    // that reaches the compiler always takes it down.
    pub(super) fn goto(&mut self, target: Option<&str>) {
        if let Some(target) = target {
            self.crash(format!("GOTO '{target}' without label"));
        }
    }

    // A chain of `else if`, or of `if`s that are each the body of the one
    // before, is compiled one inside the other, and the game runs out of
    // stack on a long one: 125 `else if` branches compile, 150 do not.
    pub(super) fn long_ladder(&mut self, links: usize) {
        if links == LADDER_CRASH {
            self.crash(format!("{LADDER_CRASH} or more 'if' statements, one inside the other with no block between"));
        }
    }

    // The game has room for 256 of a function's variables at once. The one
    // after is still compiled, with the rest of the module, and the game dies
    // or hangs later; the one after that takes it down where it stands. Not
    // measured: the variables of a `foreach` take no room.
    pub(super) fn hold_variable(&mut self, line: u32) {
        self.variables += 1;
        if self.variables > VARIABLES + 1 {
            self.statement_line = line;
            self.crash(too_many_variables());
        } else if self.variables > VARIABLES && self.one_too_many.is_none() {
            self.one_too_many = Some((self.file.clone(), self.report_line.unwrap_or(line)));
        }
    }

    pub(super) fn die_of_one_variable_too_many(&mut self) {
        if let Some((file, line)) = self.one_too_many.take() {
            self.file = file;
            self.report_line = Some(line);
            self.crash(too_many_variables());
        }
    }
}

const VARIABLES: usize = 256;

fn too_many_variables() -> String {
    format!("A function of more than {VARIABLES} variables")
}
const LADDER_CRASH: usize = 150;
