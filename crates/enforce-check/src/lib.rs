pub mod bodies;
pub mod diag;
pub mod index;
pub mod input;
pub mod loader;
pub mod parser;
pub mod preproc;
pub mod probes;
pub mod target;

pub use diag::{CheckResult, Diagnostic, Severity};
pub use input::{CheckInput, Layer, ModInfo, Module, SourceFile};
pub use target::{RuleKind, Target, adjust, defines, layer_defines};

// Stack for a check that nests past what the caller's stack holds. The stages
// walk the syntax tree by recursion, and the game reads scripts nested
// thousands deep; only the pages a check touches are ever backed by memory.
const DEEP_STACK_BYTES: usize = 1 << 30;

pub fn check(input: &CheckInput) -> CheckResult {
    // Nearly every script fits the caller's stack and is read once, there.
    let (result, over_limit) = check_layers(input, parser::Limits::CALLER_STACK);
    if !over_limit {
        return result;
    }
    let deep = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(DEEP_STACK_BYTES)
            .spawn_scoped(scope, || check_layers(input, parser::Limits::DEEP_STACK).0)
            .ok()
            .map(|thread| thread.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
    });
    // No room for the deep stack: report what this one could read.
    deep.unwrap_or(result)
}

// Also says whether a file nested past `limits`.
fn check_layers(input: &CheckInput, limits: parser::Limits) -> (CheckResult, bool) {
    let mut diagnostics = Vec::new();
    let mut failed_modules = Vec::new();
    let mut over_limit = false;
    let mut known = index::Known::default();
    let mut bodies_world = bodies::World::default();
    bodies_world.input_names = input.input_names.iter().cloned().collect();
    let mut instances = bodies::Instances::default();
    let base = defines(input.target, &input.mods, &input.extra_defines);
    let strict = adjust(input.target, RuleKind::Soft).0 == Severity::Error;
    for layer in Layer::ALL {
        let files: Vec<&SourceFile> = input.files.iter().filter(|file| file.layer == layer).collect();
        if files.is_empty() {
            continue;
        }
        // `#define`s carry over from one file of a module to the next.
        let mut names = base.clone();
        names.extend(
            layer_defines(input.target, layer)
                .iter()
                .map(|name| (*name).to_string()),
        );
        let mut parsed: Vec<(&SourceFile, parser::ParseOutput)> = Vec::with_capacity(files.len());
        let mut parse_diags = Vec::new();
        let mut errors = 0;
        let mut failed_silently = false;
        for file in files {
            let lexed = preproc::lex(file, &names);
            names = lexed.defines;
            let output = parser::parse_in_module(file, &lexed.tokens, errors, strict, limits);
            over_limit |= output.over_limit;
            errors = output.errors;
            let stopped = output.stopped;
            failed_silently |= output.failed_silently;
            parse_diags.extend(lexed.diags);
            parse_diags.extend(output.diags.iter().cloned());
            parsed.push((file, output));
            if stopped {
                break;
            }
        }
        // A parse error is a lexer or parser error: both count in the same
        // place, so they stop the module together. Only an error that survives
        // the target adjustment stops it.
        if record(&mut diagnostics, &mut failed_modules, input.target, parse_diags) {
            break;
        }
        // The game can give a module up after a warning alone.
        if failed_silently {
            failed_modules.push(Module::from(layer));
            break;
        }
        let files: Vec<(&SourceFile, &parser::FileAst)> = parsed
            .iter()
            .map(|(file, output)| (*file, &output.ast))
            .collect();
        let built = index::build(&files, &known, strict);
        bodies_world.add(&built);
        bodies_world.add_templates(&files);
        if record(&mut diagnostics, &mut failed_modules, input.target, built.diagnostics) {
            break;
        }
        known = built.known;
        // Bodies run only for a module that has no error so far.
        let body_diags = bodies::check(&files, &bodies_world, &mut instances, strict);
        if record(&mut diagnostics, &mut failed_modules, input.target, body_diags) {
            break;
        }
    }
    let result = CheckResult {
        diagnostics,
        failed_modules,
    };
    (result, over_limit)
}

// The target settles the severity of every rule, so each diagnostic keeps the
// kind it was raised with until here. Returns whether the module failed.
fn record(
    diagnostics: &mut Vec<Diagnostic>,
    failed_modules: &mut Vec<Module>,
    target: Target,
    raised: Vec<Diagnostic>,
) -> bool {
    let mut failed = false;
    for mut diagnostic in raised {
        let kind = match (diagnostic.severity, diagnostic.fixme) {
            (_, true) => RuleKind::Soft,
            (Severity::Error, false) => RuleKind::Error,
            (Severity::Warning, false) => RuleKind::Warning,
        };
        (diagnostic.severity, diagnostic.fixme) = adjust(target, kind);
        if diagnostic.severity == Severity::Error && !diagnostic.reports_only {
            failed = true;
            if !failed_modules.contains(&diagnostic.module) {
                failed_modules.push(diagnostic.module);
            }
        }
        diagnostics.push(diagnostic);
    }
    failed
}
