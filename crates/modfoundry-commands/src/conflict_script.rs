//! What several mods' scripts declare or override in common.

use std::collections::BTreeMap;

use modfoundry_core::diagnostic::{Diagnostic, Severity};
use modfoundry_gamediff::hits::ModInput;
use modfoundry_gamediff::hits_script::head_of_outline;
use modfoundry_gamediff::script::condition;
use modfoundry_script::outline::{FileOutline, GuardOutline, MethodOutline};

use crate::conflict_data::{Finding, FindingPlace};

const SHARED_DECLARATION: &str = "MF-M101";
const SHARED_MODDED_CLASS: &str = "MF-M102";
const SHARED_OVERRIDE: &str = "MF-M103";
const OVERRIDE_SKIPS_SUPER: &str = "MF-M104";

/// Every script collision between `mods`, which are loaded in `order`.
pub(crate) fn script_conflicts(mods: &[ModInput], order: &[usize]) -> Vec<Finding> {
    let outlines: Vec<Vec<FileOutline>> = mods.iter().map(ModInput::outlines).collect();
    let rank = ranks(order);
    let mut findings = shared_declarations(mods, &outlines, &rank);
    findings.extend(shared_modded_classes(mods, &outlines, &rank));
    let (overriden, cuts) = overrides(mods, &outlines, &rank);
    findings.extend(overriden);
    findings.extend(cuts);
    findings
}

/// Where one mod's side of a finding is.
#[derive(Clone, Copy)]
struct Site<'a> {
    mod_index: usize,
    path: &'a str,
    line: usize,
}

/// One mod override of one method head; each of its arms is an override.
#[derive(Clone, Copy)]
struct Override<'a> {
    mod_index: usize,
    path: &'a str,
    method: &'a MethodOutline,
}

impl Override<'_> {
    fn site(&self) -> Site<'_> {
        Site {
            mod_index: self.mod_index,
            path: self.path,
            line: self.method.lines[0],
        }
    }
}

/// Each mod's position in the load order.
fn ranks(order: &[usize]) -> Vec<usize> {
    let mut rank = vec![0; order.len()];
    for (position, &mod_index) in order.iter().enumerate() {
        rank[mod_index] = position;
    }
    rank
}

/// A class or enum declared by two or more mods under the same conditions.
fn shared_declarations(
    mods: &[ModInput],
    outlines: &[Vec<FileOutline>],
    rank: &[usize],
) -> Vec<Finding> {
    let mut groups: BTreeMap<(String, Vec<String>), Vec<Site>> = BTreeMap::new();
    for (mod_index, files) in outlines.iter().enumerate() {
        for file in files {
            for class in &file.classes {
                if !class.modded {
                    groups
                        .entry((class.name.clone(), conditions(&class.guards)))
                        .or_default()
                        .push(Site {
                            mod_index,
                            path: &file.path,
                            line: class.lines[0],
                        });
                }
            }
            for item in &file.enums {
                groups
                    .entry((item.name.clone(), conditions(&item.guards)))
                    .or_default()
                    .push(Site {
                        mod_index,
                        path: &file.path,
                        line: item.line,
                    });
            }
        }
    }
    let mut findings = Vec::new();
    for ((name, _), mut sites) in groups {
        if mods_in(&sites).len() < 2 {
            continue;
        }
        sort_sites(&mut sites, rank);
        let declared_by = names(&sites, mods).join(", ");
        findings.push(finding(
            SHARED_DECLARATION,
            Severity::Error,
            format!("{name} is declared by {declared_by}; only one mod can declare it"),
            mods,
            &sites,
        ));
    }
    findings
}

/// A class two or more mods mod.
fn shared_modded_classes(
    mods: &[ModInput],
    outlines: &[Vec<FileOutline>],
    rank: &[usize],
) -> Vec<Finding> {
    let mut groups: BTreeMap<&str, Vec<Site>> = BTreeMap::new();
    for (mod_index, files) in outlines.iter().enumerate() {
        for file in files {
            for class in file.classes.iter().filter(|class| class.modded) {
                groups.entry(&class.name).or_default().push(Site {
                    mod_index,
                    path: &file.path,
                    line: class.lines[0],
                });
            }
        }
    }
    let mut findings = Vec::new();
    for (name, mut sites) in groups {
        if mods_in(&sites).len() < 2 {
            continue;
        }
        sort_sites(&mut sites, rank);
        let modded_by = names(&sites, mods).join(", ");
        findings.push(finding(
            SHARED_MODDED_CLASS,
            Severity::Hint,
            format!("class {name} is modded by {modded_by}"),
            mods,
            &sites,
        ));
    }
    findings
}

/// Overrides by two or more mods, then the ones that cut off a mod loaded before them.
fn overrides(
    mods: &[ModInput],
    outlines: &[Vec<FileOutline>],
    rank: &[usize],
) -> (Vec<Finding>, Vec<Finding>) {
    let mut overriden = Vec::new();
    let mut cuts = Vec::new();
    for (class, heads) in modded_heads(outlines) {
        for (head, mut overrides) in heads {
            sort_overrides(&mut overrides, rank);
            let sites: Vec<Site> = overrides.iter().map(|item| item.site()).collect();
            let loaded = mods_in(&sites);
            if loaded.len() < 2 {
                continue;
            }
            let reachers = loaded_mods(mods, &loaded).join(", ");
            overriden.push(finding(
                SHARED_OVERRIDE,
                Severity::Warning,
                format!("{class}.{head} is overridden by {reachers}"),
                mods,
                &sites,
            ));
            cuts.extend(cut_offs(class, &head, mods, &overrides, &loaded));
        }
    }
    (overriden, cuts)
}

/// Each modded class, with its mods' overrides keyed by method head.
fn modded_heads<'a>(
    outlines: &'a [Vec<FileOutline>],
) -> BTreeMap<&'a str, BTreeMap<String, Vec<Override<'a>>>> {
    let mut classes: BTreeMap<&str, BTreeMap<String, Vec<Override>>> = BTreeMap::new();
    for (mod_index, files) in outlines.iter().enumerate() {
        for file in files {
            for class in file.classes.iter().filter(|class| class.modded) {
                let heads = classes.entry(&class.name).or_default();
                for method in &class.methods {
                    if is_construction(class, method) {
                        continue;
                    }
                    heads
                        .entry(head_of_outline(method))
                        .or_default()
                        .push(Override {
                            mod_index,
                            path: &file.path,
                            method,
                        });
                }
            }
        }
    }
    classes
}

/// Only a method marked `override` replaces an earlier mod's definition.
fn is_override(method: &MethodOutline) -> bool {
    method
        .modifiers
        .iter()
        .any(|modifier| modifier == "override")
}

/// A constructor or destructor has no `super.<name>()` to call.
fn is_construction(
    class: &modfoundry_script::outline::ClassOutline,
    method: &MethodOutline,
) -> bool {
    method.destructor || method.name == class.name
}

/// Every later mod that overrides `head` without calling `super`, named with the mods it cuts off.
fn cut_offs(
    class: &str,
    head: &str,
    mods: &[ModInput],
    overrides: &[Override],
    loaded: &[usize],
) -> Vec<Finding> {
    let mut cuts = Vec::new();
    for split in 1..loaded.len() {
        let (before, later) = loaded.split_at(split);
        let current = later[0];
        let mut skipping: Vec<&Override> = overrides
            .iter()
            .filter(|item| {
                item.mod_index == current && is_override(item.method) && !calls_super(item.method)
            })
            .collect();
        if skipping.is_empty() {
            continue;
        }
        skipping.sort_by_key(|item| (item.path, item.method.lines[0]));
        let cut_off = before.iter().flat_map(|index| {
            overrides
                .iter()
                .filter(move |item| item.mod_index == *index)
        });
        let sites: Vec<Site> = skipping
            .iter()
            .map(|item| item.site())
            .chain(cut_off.map(Override::site))
            .collect();
        let name = &skipping[0].method.name;
        let involved = loaded_mods(mods, &loaded[..=split]);
        let cut_off_names = loaded_mods(mods, before).join(", ");
        cuts.push(Finding {
            diagnostic: Diagnostic::new(
                OVERRIDE_SKIPS_SUPER,
                Severity::Error,
                format!(
                    "{} overrides {class}.{head} without calling super.{name}, which cuts off {cut_off_names}",
                    mods[current].name
                ),
            ),
            mods: involved,
            places: places(mods, &sites),
        });
    }
    cuts
}

fn calls_super(method: &MethodOutline) -> bool {
    method
        .calls
        .iter()
        .any(|call| call.receiver.as_deref() == Some("super") && call.name == method.name)
}

fn conditions(guards: &[GuardOutline]) -> Vec<String> {
    guards.iter().map(condition).collect()
}

/// The mods among `sites`, in the order they first appear.
fn mods_in(sites: &[Site]) -> Vec<usize> {
    let mut seen = Vec::new();
    for site in sites {
        if !seen.contains(&site.mod_index) {
            seen.push(site.mod_index);
        }
    }
    seen
}

fn sort_sites(sites: &mut [Site], rank: &[usize]) {
    sites.sort_by_key(|site| (rank[site.mod_index], site.path, site.line));
}

fn sort_overrides(overrides: &mut [Override], rank: &[usize]) {
    overrides.sort_by_key(|item| (rank[item.mod_index], item.path, item.method.lines[0]));
}

fn names(sites: &[Site], mods: &[ModInput]) -> Vec<String> {
    loaded_mods(mods, &mods_in(sites))
}

fn loaded_mods(mods: &[ModInput], indexes: &[usize]) -> Vec<String> {
    indexes
        .iter()
        .map(|index| mods[*index].name.clone())
        .collect()
}

fn finding(
    code: &str,
    severity: Severity,
    message: String,
    mods: &[ModInput],
    sites: &[Site],
) -> Finding {
    Finding {
        diagnostic: Diagnostic::new(code, severity, message),
        mods: names(sites, mods),
        places: places(mods, sites),
    }
}

fn places(mods: &[ModInput], sites: &[Site]) -> Vec<FindingPlace> {
    sites
        .iter()
        .map(|site| FindingPlace {
            mod_name: mods[site.mod_index].name.clone(),
            path: site.path.to_owned(),
            line: Some(site.line),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_core::diagnostic::Severity;
    use modfoundry_gamediff::hits::ModInput;

    const WITH_SUPER: &str =
        "modded class C\n{\n\toverride void F(int a)\n\t{\n\t\tsuper.F(a);\n\t}\n}\n";
    const WITHOUT_SUPER: &str = "modded class C\n{\n\toverride void F(int a)\n\t{\n\t}\n}\n";

    fn input(name: &str, files: &[(&str, &str)]) -> ModInput {
        ModInput {
            name: name.to_owned(),
            scripts: files
                .iter()
                .map(|(path, source)| ((*path).to_owned(), (*source).to_owned()))
                .collect(),
            configs: Vec::new(),
        }
    }

    fn findings_of(sources: &[(&str, &str)]) -> Vec<Finding> {
        let mods: Vec<ModInput> = sources
            .iter()
            .map(|(name, source)| input(name, &[("scripts/a.c", source)]))
            .collect();
        let order: Vec<usize> = (0..mods.len()).collect();
        script_conflicts(&mods, &order)
    }

    fn places(finding: &Finding) -> Vec<(&str, &str, Option<usize>)> {
        finding
            .places
            .iter()
            .map(|place| (place.mod_name.as_str(), place.path.as_str(), place.line))
            .collect()
    }

    fn of_code<'a>(found: &'a [Finding], code: &str) -> Vec<&'a Finding> {
        found
            .iter()
            .filter(|finding| finding.diagnostic.code == code)
            .collect()
    }

    #[test]
    fn two_mods_declaring_one_name_give_one_error() {
        let mods = [
            input("A", &[("a.c", "class Shared\n{\n}\n")]),
            input("B", &[("b.c", "\nclass Shared\n{\n}\n")]),
            input("C", &[("c.c", "enum Shared\n{\n\tX\n}\n")]),
        ];

        let found = script_conflicts(&mods, &[0, 1, 2]);

        assert_eq!(found.len(), 1, "{found:?}");
        let finding = &found[0];
        assert_eq!(finding.diagnostic.code, "MF-M101");
        assert_eq!(finding.diagnostic.severity, Severity::Error);
        assert_eq!(
            finding.diagnostic.message,
            "Shared is declared by A, B, C; only one mod can declare it"
        );
        assert_eq!(finding.mods, ["A", "B", "C"]);
        assert_eq!(
            places(finding),
            [
                ("A", "a.c", Some(1)),
                ("B", "b.c", Some(2)),
                ("C", "c.c", Some(1)),
            ]
        );
    }

    #[test]
    fn a_name_declared_twice_in_one_mod_is_not_a_collision() {
        let mods = [input(
            "A",
            &[("a.c", "class Shared {}\n"), ("b.c", "class Shared {}\n")],
        )];

        assert!(script_conflicts(&mods, &[0]).is_empty());
    }

    #[test]
    fn declarations_that_do_not_collide_give_nothing() {
        let cases = [
            ("class Shared {}\n", "class Other {}\n"),
            ("class Shared {}\n", "modded class Shared {}\n"),
            (
                "#ifdef SERVER\nclass Shared {}\n#endif\n",
                "#ifndef SERVER\nclass Shared {}\n#endif\n",
            ),
        ];

        for (a, b) in cases {
            assert!(findings_of(&[("A", a), ("B", b)]).is_empty(), "{a} / {b}");
        }
    }

    #[test]
    fn a_class_modded_by_two_mods_gives_one_hint() {
        let found = findings_of(&[
            ("A", "modded class C\n{\n}\n"),
            ("B", "modded class C\n{\n}\n"),
        ]);

        assert_eq!(found.len(), 1, "{found:?}");
        let finding = &found[0];
        assert_eq!(finding.diagnostic.code, "MF-M102");
        assert_eq!(finding.diagnostic.severity, Severity::Hint);
        assert_eq!(finding.diagnostic.message, "class C is modded by A, B");
        assert_eq!(finding.mods, ["A", "B"]);
        assert_eq!(
            places(finding),
            [("A", "scripts/a.c", Some(1)), ("B", "scripts/a.c", Some(1))]
        );
    }

    #[test]
    fn a_class_modded_by_one_mod_gives_no_hint() {
        let found = findings_of(&[
            ("A", "modded class C\n{\n}\n"),
            ("B", "class Other\n{\n}\n"),
        ]);

        assert!(of_code(&found, "MF-M102").is_empty(), "{found:?}");
    }

    #[test]
    fn two_modded_parts_in_one_mod_count_as_one_mod() {
        let twice = "modded class C\n{\n}\nmodded class C\n{\n}\n";
        let found = findings_of(&[("A", twice), ("B", "class Other\n{\n}\n")]);

        assert!(of_code(&found, "MF-M102").is_empty(), "{found:?}");
    }

    #[test]
    fn an_override_by_two_mods_gives_one_warning_with_both_places() {
        let found = findings_of(&[("A", WITH_SUPER), ("B", WITH_SUPER)]);

        let warnings = of_code(&found, "MF-M103");
        assert_eq!(warnings.len(), 1, "{found:?}");
        assert_eq!(warnings[0].diagnostic.severity, Severity::Warning);
        assert_eq!(
            warnings[0].diagnostic.message,
            "C.F(int) is overridden by A, B"
        );
        assert_eq!(warnings[0].mods, ["A", "B"]);
        assert_eq!(
            places(warnings[0]),
            [("A", "scripts/a.c", Some(3)), ("B", "scripts/a.c", Some(3))]
        );
    }

    #[test]
    fn overrides_that_do_not_share_a_head_give_nothing() {
        let found = findings_of(&[
            ("A", "modded class C\n{\n\toverride void F(int a) {}\n}\n"),
            (
                "B",
                "modded class C\n{\n\toverride void F(string a) {}\n}\n",
            ),
        ]);

        assert!(of_code(&found, "MF-M103").is_empty(), "{found:?}");
    }

    #[test]
    fn two_arms_of_one_ifdef_in_one_mod_count_as_one_mod() {
        let both_arms = "modded class C\n{\n#ifdef SERVER\n\toverride void F(int a) { super.F(a); }\n#else\n\toverride void F(int a) { super.F(a); }\n#endif\n}\n";
        let found = findings_of(&[("A", both_arms), ("B", WITH_SUPER)]);

        let warnings = of_code(&found, "MF-M103");
        assert_eq!(warnings.len(), 1, "{found:?}");
        assert_eq!(warnings[0].mods, ["A", "B"]);
        assert_eq!(places(warnings[0]).len(), 3);
    }

    #[test]
    fn constructors_and_destructors_are_left_out() {
        let found = findings_of(&[
            ("A", "modded class C\n{\n\tvoid C() {}\n\tvoid ~C() {}\n}\n"),
            ("B", "modded class C\n{\n\tvoid C() {}\n\tvoid ~C() {}\n}\n"),
        ]);

        assert!(of_code(&found, "MF-M103").is_empty(), "{found:?}");
        assert!(of_code(&found, "MF-M104").is_empty(), "{found:?}");
    }

    #[test]
    fn the_order_given_decides_which_mod_comes_first() {
        let mods = vec![
            input("A", &[("scripts/a.c", WITH_SUPER)]),
            input("B", &[("scripts/a.c", WITH_SUPER)]),
        ];

        let found = script_conflicts(&mods, &[1, 0]);

        let warning = of_code(&found, "MF-M103")[0];
        assert_eq!(warning.mods, ["B", "A"]);
        assert_eq!(
            places(warning),
            [("B", "scripts/a.c", Some(3)), ("A", "scripts/a.c", Some(3))]
        );
    }

    fn one_cut(found: &[Finding]) -> &Finding {
        let cuts = of_code(found, "MF-M104");
        assert_eq!(cuts.len(), 1, "{found:?}");
        cuts[0]
    }

    #[test]
    fn an_override_without_super_cuts_off_the_mod_loaded_before_it() {
        let found = findings_of(&[("A", WITH_SUPER), ("B", WITHOUT_SUPER)]);

        let cut = one_cut(&found);
        assert_eq!(cut.diagnostic.severity, Severity::Error);
        assert_eq!(
            cut.diagnostic.message,
            "B overrides C.F(int) without calling super.F, which cuts off A"
        );
        assert_eq!(cut.mods, ["A", "B"]);
        assert_eq!(
            places(cut),
            [("B", "scripts/a.c", Some(3)), ("A", "scripts/a.c", Some(3))]
        );
    }

    #[test]
    fn a_super_call_anywhere_in_the_override_is_enough() {
        let found = findings_of(&[("A", WITH_SUPER), ("B", WITH_SUPER)]);

        assert!(of_code(&found, "MF-M104").is_empty(), "{found:?}");
    }

    #[test]
    fn a_method_both_mods_add_without_override_cuts_nothing_off() {
        let added = "modded class C\n{\n\tvoid Helper(int a)\n\t{\n\t}\n}\n";
        let found = findings_of(&[("A", added), ("B", added)]);

        assert!(of_code(&found, "MF-M104").is_empty(), "{found:?}");
    }

    #[test]
    fn the_first_loaded_mod_never_gets_a_cut_off() {
        let found = findings_of(&[("A", WITHOUT_SUPER), ("B", WITH_SUPER)]);

        assert!(of_code(&found, "MF-M104").is_empty(), "{found:?}");
    }

    #[test]
    fn a_third_mod_that_skips_super_names_both_mods_before_it() {
        let found = findings_of(&[("A", WITH_SUPER), ("B", WITH_SUPER), ("C", WITHOUT_SUPER)]);

        let cut = one_cut(&found);
        assert_eq!(
            cut.diagnostic.message,
            "C overrides C.F(int) without calling super.F, which cuts off A, B"
        );
        assert_eq!(cut.mods, ["A", "B", "C"]);
    }

    #[test]
    fn a_middle_mod_that_skips_only_cuts_off_the_mods_before_it() {
        let found = findings_of(&[("A", WITH_SUPER), ("B", WITHOUT_SUPER), ("C", WITH_SUPER)]);

        let cut = one_cut(&found);
        assert_eq!(
            cut.diagnostic.message,
            "B overrides C.F(int) without calling super.F, which cuts off A"
        );
        assert_eq!(cut.mods, ["A", "B"]);
    }

    #[test]
    fn an_arm_without_a_super_call_counts_as_skipping() {
        let guarded = "modded class C\n{\n#ifdef SERVER\n\toverride void F(int a) { super.F(a); }\n#else\n\toverride void F(int a) { }\n#endif\n}\n";
        let found = findings_of(&[("A", WITH_SUPER), ("B", guarded)]);

        let cut = one_cut(&found);
        assert_eq!(
            cut.diagnostic.message,
            "B overrides C.F(int) without calling super.F, which cuts off A"
        );
        assert_eq!(
            places(cut),
            [("B", "scripts/a.c", Some(6)), ("A", "scripts/a.c", Some(3))]
        );
    }

    #[test]
    fn a_super_call_to_another_method_does_not_count() {
        let other = "modded class C\n{\n\toverride void F(int a) { super.G(a); }\n}\n";
        let found = findings_of(&[("A", WITH_SUPER), ("B", other)]);

        one_cut(&found);
    }

    #[test]
    fn swapping_the_order_swaps_who_is_cut_off() {
        let mods = vec![
            input("A", &[("scripts/a.c", WITHOUT_SUPER)]),
            input("B", &[("scripts/a.c", WITHOUT_SUPER)]),
        ];

        let first = script_conflicts(&mods, &[0, 1]);
        assert_eq!(
            one_cut(&first).diagnostic.message,
            "B overrides C.F(int) without calling super.F, which cuts off A"
        );

        let swapped = script_conflicts(&mods, &[1, 0]);
        assert_eq!(
            one_cut(&swapped).diagnostic.message,
            "A overrides C.F(int) without calling super.F, which cuts off B"
        );
    }

    #[test]
    fn each_code_keeps_its_severity() {
        let found = findings_of(&[
            (
                "A",
                "class Shared {}\nmodded class C\n{\n\toverride void F(int a) { super.F(a); }\n}\n",
            ),
            (
                "B",
                "enum Shared { X }\nmodded class C\n{\n\toverride void F(int a) { }\n}\n",
            ),
        ]);

        let pairs: Vec<(&str, Severity)> = found
            .iter()
            .map(|f| (f.diagnostic.code.as_str(), f.diagnostic.severity))
            .collect();
        assert_eq!(
            pairs,
            [
                ("MF-M101", Severity::Error),
                ("MF-M102", Severity::Hint),
                ("MF-M103", Severity::Warning),
                ("MF-M104", Severity::Error),
            ],
            "{found:?}"
        );
    }
}
