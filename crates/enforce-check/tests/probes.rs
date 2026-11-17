use std::path::{Path, PathBuf};

use enforce_check::probes::{
    CaseResult, ExpectedLine, Observed, Report, Totals, Verdict, compare_lists, load_probe, load_probes, load_tree,
    score,
};
use enforce_check::{Diagnostic, Layer, Module, Severity, Target};

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(path)
}

#[test]
fn probe_folder_gives_its_sources_and_expected_result() {
    let case = load_probe(&fixture("probes/probes/decl-001"));

    assert_eq!(case.id, "decl-001");
    assert_eq!(case.cfg_class, "Probe_decl_001");
    assert_eq!(case.files.len(), 1);
    assert_eq!(case.files[0].path, "Probe_decl_001/Scripts/3_Game/b.c");
    assert_eq!(case.files[0].layer, Layer::Game);
    assert_eq!(case.files[0].mod_index, 1);
    assert!(case.files[0].text.contains("class ProbeDup"));

    let observed = &case.observed;
    assert_eq!(observed.module, Some(Module::Game));
    assert_eq!(observed.severity, Some(Severity::Error));
    assert_eq!(observed.verdict, Some(Verdict::Ok));
    let line = observed.first_line.as_ref().expect("first line");
    assert_eq!(line.file.as_deref(), Some("Scripts/3_Game/b.c"));
    assert_eq!(line.line, Some(1));
    assert!(!line.fixme);
    assert_eq!(line.message, "Multiple declaration of class 'ProbeDup'");
}

#[test]
fn loose_tree_lists_scripts_with_their_layer() {
    let files = load_tree(&fixture("probes/vanilla"), Layer::from_folder_name);

    let listed: Vec<(&str, Layer)> = files
        .iter()
        .map(|file| (file.path.as_str(), file.layer))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("Scripts/1_Core/base.c", Layer::Core),
            ("Scripts/3_Game/nested/helper.c", Layer::Game),
        ]
    );
    assert!(files.iter().all(|file| file.mod_index == 0));
}

#[test]
fn diag_result_gives_the_expected_line_and_failed_module() {
    let case = load_probe(&fixture("probes/probes/syn-001"));

    let diag = case.diag.as_ref().expect("results-diag file");
    assert_eq!(diag.module, Some(Module::World));
    assert_eq!(diag.severity, Some(Severity::Error));
    assert_eq!(diag.verdict, None);
    let line = diag.first_line.as_ref().expect("first line");
    assert_eq!(line.file.as_deref(), Some("Probe_syn_001/Scripts/4_World/a.c"));
    assert_eq!(line.line, Some(3));
    assert_eq!(line.message, "Missing function scope");

    let retail = case.expected(Target::RetailServer);
    assert_eq!(retail.severity, Some(Severity::Warning));
    assert!(retail.first_line.as_ref().expect("first line").fixme);
    assert_eq!(case.expected(Target::DiagServer).module, Some(Module::World));
}

#[test]
fn diag_result_without_an_error_line_is_not_scored() {
    let case = load_probe(&fixture("probes/probes/decl-001"));

    assert!(case.diag.is_none());
    assert!(!case.expected(Target::DiagServer).recorded);
    assert!(case.expected(Target::RetailServer).recorded);
}

#[test]
fn probes_without_an_observed_row_are_not_scored() {
    let cases = load_probes(&fixture("probes"));
    let ids: Vec<&str> = cases.iter().map(|case| case.id.as_str()).collect();
    assert_eq!(ids, vec!["decl-001", "body-P01", "syn-001", "ord-001"]);

    let ord = cases.iter().find(|case| case.id == "ord-001").unwrap();
    assert!(!ord.observed.recorded);
    assert_eq!(ord.observed.expected_text(), "no verdict");
}

fn diagnostic(file: &str, line: u32, message: &str) -> Diagnostic {
    Diagnostic {
        file: file.to_string(),
        line,
        module: Module::Game,
        severity: Severity::Error,
        message: message.to_string(),
        fixme: false,
        game_crash: false,
        reports_only: false,
        related: None,
    }
}

fn expected(file: &str, line: u32, message: &str) -> Observed {
    Observed {
        module: Some(Module::Game),
        severity: Some(Severity::Error),
        first_line: Some(ExpectedLine {
            file: Some(file.to_string()),
            line: Some(line),
            fixme: false,
            message: message.to_string(),
        }),
        verdict: Some(Verdict::Ok),
        recorded: true,
    }
}

#[test]
fn comparison_scores_what_the_game_printed() {
    let duplicate = "Multiple declaration of class 'ProbeDup'";
    let crash_expected = Observed {
        first_line: None,
        verdict: Some(Verdict::Crash),
        recorded: true,
        ..Observed::default()
    };
    let compiles = Observed {
        first_line: None,
        verdict: Some(Verdict::Ok),
        recorded: true,
        ..Observed::default()
    };
    let mut crash = diagnostic("Scripts/3_Game/b.c", 1, duplicate);
    crash.game_crash = true;
    let no_verdict = Observed {
        first_line: Some(ExpectedLine {
            file: None,
            line: None,
            fixme: false,
            message: duplicate.to_string(),
        }),
        ..Observed::default()
    };

    let cases: Vec<(&str, Observed, Option<Diagnostic>, bool)> = vec![
        (
            "same line, message and module",
            expected("Scripts/3_Game/b.c", 1, duplicate),
            Some(diagnostic("Probe_decl_001/Scripts/3_Game/B.C", 1, duplicate)),
            true,
        ),
        (
            "differ by line",
            expected("Scripts/3_Game/b.c", 1, duplicate),
            Some(diagnostic("Scripts/3_Game/b.c", 2, duplicate)),
            false,
        ),
        (
            "differ by message",
            expected("Scripts/3_Game/b.c", 1, duplicate),
            Some(diagnostic("Scripts/3_Game/b.c", 1, "Syntax error")),
            false,
        ),
        (
            "differ by file",
            expected("Scripts/3_Game/b.c", 1, duplicate),
            Some(diagnostic("Scripts/3_Game/a.c", 1, duplicate)),
            false,
        ),
        (
            "expected an error, got nothing",
            expected("Scripts/3_Game/b.c", 1, duplicate),
            None,
            false,
        ),
        ("expected a crash, got a crash", crash_expected.clone(), Some(crash), true),
        ("expected a crash, got an error", crash_expected.clone(), Some(diagnostic("Scripts/3_Game/b.c", 1, duplicate)), false),
        ("expected a crash, got nothing", crash_expected, None, false),
        ("expected no error, got nothing", compiles.clone(), None, true),
        (
            "expected no error, got one",
            compiles,
            Some(diagnostic("Scripts/3_Game/b.c", 1, duplicate)),
            false,
        ),
        ("probe without a verdict", no_verdict, None, false),
    ];

    for (name, observed, got, agree) in cases {
        assert_eq!(observed.agrees(got.as_ref()), agree, "{name}");
    }
}

#[test]
fn report_totals_count_agree_and_skip_probes_without_a_verdict() {
    let cases = vec![
        CaseResult {
            id: "decl-001".to_string(),
            agree: true,
            expected: expected("Scripts/3_Game/b.c", 1, "Multiple declaration of class 'ProbeDup'"),
            got: None,
        },
        CaseResult {
            id: "decl-002".to_string(),
            agree: false,
            expected: expected("Scripts/3_Game/b.c", 1, "Unknown type 'ProbeNoSuchBase'"),
            got: None,
        },
        CaseResult {
            id: "body-P01".to_string(),
            agree: true,
            expected: expected("Scripts/3_Game/probe.c", 5, "Can't find variable 'b'"),
            got: None,
        },
        CaseResult {
            id: "ord-001".to_string(),
            agree: false,
            expected: Observed::default(),
            got: None,
        },
    ];

    let report = Report::build(Target::RetailServer, &cases);
    assert_eq!(report.totals, Totals { agree: 2, total: 3 });
    assert_eq!(
        report.by_prefix,
        vec![
            ("decl".to_string(), Totals { agree: 1, total: 2 }),
            ("body".to_string(), Totals { agree: 1, total: 1 }),
            ("syn".to_string(), Totals::default()),
            ("ord".to_string(), Totals::default()),
        ]
    );
    assert_eq!(report.rows[3].agree, None);
    assert_eq!(report.rows[3].expected, "no verdict");

    let table = report.to_table();
    assert!(table.contains("decl-001"));
    assert!(table.contains("differ"));
    assert!(table.contains("agree 2/3"));
    assert!(table.contains("decl 1/2"));
    assert!(table.contains("body 1/1"));
}

#[test]
fn filter_matches_the_start_of_the_id() {
    let root = fixture("probes");
    let vanilla = load_tree(&root.join("vanilla"), Layer::from_folder_name);
    for (filter, expected_ids) in [
        ("decl", vec!["decl-001"]),
        ("decl-00", vec!["decl-001"]),
        ("body-P", vec!["body-P01"]),
        ("nothing", vec![]),
    ] {
        let report = score(&root, &vanilla, Target::RetailServer, Some(filter));
        let ids: Vec<&str> = report.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, expected_ids, "{filter}");
    }
}

#[test]
fn table_has_a_got_column_and_no_verdict_rows_show_a_dash() {
    let root = fixture("probes");
    let report = score(&root, &[], Target::RetailServer, None);
    let table = report.to_table();
    assert!(table.lines().next().unwrap().ends_with("got"), "{table}");
    let ord = report.rows.iter().find(|row| row.id == "ord-001").unwrap();
    assert_eq!(ord.got, "-");
    let decl = report.rows.iter().find(|row| row.id == "decl-001").unwrap();
    assert_eq!(decl.got, "nothing");
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

#[test]
#[ignore]
fn every_probe_loads_and_the_scored_ones_run() {
    if std::env::var("ENFORCE_CHECK_SLOW").as_deref() != Ok("1") {
        return;
    }
    let Some(root) = env_path("ENFORCE_CHECK_PROBES") else {
        return;
    };
    let Some(vanilla_dir) = env_path("ENFORCE_CHECK_VANILLA") else {
        return;
    };

    let started = std::time::Instant::now();
    let vanilla = load_tree(&vanilla_dir, Layer::from_folder_name);
    let cases = load_probes(&root);
    let loaded = cases.len();
    let scored = cases.iter().filter(|case| case.observed.recorded).count();
    let unscored: Vec<&str> = cases
        .iter()
        .filter(|case| !case.observed.recorded)
        .map(|case| case.id.as_str())
        .collect();
    let report = Report::build(
        Target::RetailServer,
        &cases
            .iter()
            .map(|case| enforce_check::probes::run_case(case, &vanilla, Target::RetailServer))
            .collect::<Vec<_>>(),
    );

    println!(
        "loaded {loaded} probes from {} vanilla files, scored {scored}, unscored {unscored:?}",
        vanilla.len()
    );
    println!("{}", report.to_table());
    println!("run took {:?}", started.elapsed());

    assert_eq!(loaded, 815);
    assert_eq!(scored, 797);
    assert_eq!(report.totals.total, 797);
    // tpl-36 and tpl-37 ran on the Diag server only.
    let mut without_a_rule_row: Vec<&str> = unscored
        .iter()
        .copied()
        .filter(|id| !id.starts_with("ord-") && !id.ends_with("-nt"))
        .collect();
    without_a_rule_row.sort_unstable();
    assert_eq!(without_a_rule_row, ["smoke-undefined-function", "tpl-36", "tpl-37"]);
    assert_eq!(unscored.len(), 18);
}

#[test]
fn whole_list_comparison_reads_every_line_that_points_into_the_probe() {
    let root = fixture("probes");
    let vanilla = load_tree(&root.join("vanilla"), Layer::from_folder_name);

    let results = compare_lists(&root, &vanilla, Target::RetailServer, None).results;

    let listed: Vec<(&str, bool)> = results.iter().map(|result| (result.id.as_str(), result.agree)).collect();
    assert_eq!(listed, vec![("body-P01", true), ("syn-001", false)]);
    let syn = &results[1];
    assert_eq!(
        syn.expected,
        vec![
            "W @\"Probe_syn_001/Scripts/4_World/a.c,3\": FIX-ME: Missing function scope",
            "W @\"Probe_syn_001/Scripts/4_World/a.c,4\": Made up line the checker does not print",
        ]
    );
    assert!(syn.got.is_empty(), "{:?}", syn.got);
    assert_eq!(
        results[0].got,
        vec![
            "E @\"Probe_body_P01/Scripts/3_Game/probe.c,5\": Undefined function 'PrA.Print'",
            "Can't compile \"Game\" script module!"
        ]
    );
}

struct ListRoot(PathBuf);

impl ListRoot {
    // A probes folder with one probe `list-01`: its script, the game's result file and its observed row.
    fn new(name: &str, script: &str, result: &str, observed: &str) -> ListRoot {
        let root = std::env::temp_dir().join(format!("enforce-check-lists-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let probe = root.join("probes/list-01/Scripts/3_Game");
        std::fs::create_dir_all(&probe).unwrap();
        std::fs::create_dir_all(root.join("results")).unwrap();
        std::fs::write(probe.join("probe.c"), script).unwrap();
        std::fs::write(root.join("results/list-01.txt"), result).unwrap();
        std::fs::write(root.join("observed.tsv"), format!("id\tmodule_failed\tsev\tfirst_line\texpect\tverdict\n{observed}")).unwrap();
        ListRoot(root)
    }

    fn agree(&self) -> bool {
        let lists = compare_lists(&self.0, &[], Target::RetailServer, None);
        lists.results[0].agree
    }
}

impl Drop for ListRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const CLEAN: &str = "class ListA {}\n";
const BROKEN: &str = "class ListA { void M() { Nope(); } }\n";
const NOPE_LINE: &str = "SCRIPT    (E): @\"Probe_list_01/Scripts/3_Game/probe.c,1\": Undefined function 'ListA.Nope'\n";
const GAME_FAILED: &str = "SCRIPT    (E): Can't compile \"Game\" script module!\n";

#[test]
fn whole_list_comparison_notices_a_module_the_game_failed_and_we_did_not() {
    let root = ListRoot::new("failed", CLEAN, GAME_FAILED, "list-01\tGame\tE\t\t?\tOK");
    assert!(!root.agree());
}

#[test]
fn whole_list_comparison_notices_a_module_we_failed_and_the_game_did_not() {
    let root = ListRoot::new("passed", BROKEN, NOPE_LINE, "list-01\t-\tE\t\t?\tOK");
    assert!(!root.agree());
}

#[test]
fn whole_list_comparison_notices_a_crash_we_do_not_report() {
    let root = ListRoot::new("crash", CLEAN, "SCRIPT       : Module: GameLib; loaded 1x files\n", "list-01\tCRASH\t-\t\t?\tCRASH");
    assert!(!root.agree());
}

#[test]
fn whole_list_comparison_keeps_the_case_of_names_in_messages() {
    let result = "SCRIPT    (E): @\"Probe_list_01/Scripts/3_Game/probe.c,1\": Undefined function 'LISTA.Nope'\nSCRIPT    (E): Can't compile \"Game\" script module!\n";
    let root = ListRoot::new("case", BROKEN, result, "list-01\tGame\tE\t\t?\tOK");
    assert!(!root.agree());
}

#[test]
fn whole_list_comparison_folds_the_case_of_the_path() {
    let result = "SCRIPT    (E): @\"probe_list_01/scripts/3_game/probe.c,1\": Undefined function 'ListA.Nope'\nSCRIPT    (E): Can't compile \"Game\" script module!\n";
    let root = ListRoot::new("path", BROKEN, result, "list-01\tGame\tE\t\t?\tOK");
    assert!(root.agree());
}

#[test]
fn whole_list_comparison_names_the_probes_with_no_result_file() {
    let root = ListRoot::new("missing", CLEAN, "", "list-01\t-\t-\t\t?\tOK");
    std::fs::remove_file(root.0.join("results/list-01.txt")).unwrap();
    let lists = compare_lists(&root.0, &[], Target::RetailServer, None);
    assert!(lists.results.is_empty());
    assert_eq!(lists.without_result, vec!["list-01"]);
}

#[test]
fn whole_list_comparison_does_not_call_a_crash_where_the_game_was_not_asked() {
    let script = "#define X 1\nclass ListA { int y = X; }\n";
    let root = ListRoot::new("unasked", script, "SCRIPT       : Module: GameLib; loaded 1x files\n", "");
    assert!(root.agree());
}

#[test]
fn whole_list_comparison_reads_the_diag_results_for_a_diag_target() {
    let root = ListRoot::new("diag", CLEAN, "", "list-01\t-\t-\t\t?\tOK");
    std::fs::create_dir_all(root.0.join("results-diag")).unwrap();
    std::fs::rename(root.0.join("results/list-01.txt"), root.0.join("results-diag/list-01.txt")).unwrap();

    let diag = compare_lists(&root.0, &[], Target::DiagServer, None);
    let retail = compare_lists(&root.0, &[], Target::RetailServer, None);

    assert_eq!((diag.results.len(), diag.without_result.len()), (1, 0));
    assert_eq!((retail.results.len(), retail.without_result.len()), (0, 1));
}

// The open-string message names its file, which the game prints in lower case.
#[test]
fn whole_list_comparison_folds_the_path_inside_the_open_string_message() {
    let at = "@\"Probe_list_01/Scripts/3_Game/probe.c,3\": ";
    let result = format!(
        "SCRIPT    (E): {at}CParser: quoted string not closed on line 3, module probe_list_01/scripts/3_game/probe.c\n\
         SCRIPT    (W): {at}Missing ';' at the end of line\nSCRIPT    (W): {at}Broken expression (missing ';'?)\n{GAME_FAILED}"
    );
    let root = ListRoot::new("open-string", "void F()\n{\n\tstring s = \"ab;\n}\n", &result, "list-01\tGame\tE\t\t?\tOK");
    assert!(root.agree());
}

#[test]
fn the_scoreboard_folds_the_path_inside_the_open_string_message() {
    let first = "@\"Scripts/3_Game/probe.c,3\": CParser: quoted string not closed on line 3, module probe_list_01/scripts/3_game/probe.c";
    let root = ListRoot::new("open-string-score", "void F()\n{\n\tstring s = \"ab;\n}\n", GAME_FAILED, &format!("list-01\tGame\tE\t{first}\t?\tOK"));
    let report = enforce_check::probes::score(&root.0, &[], Target::RetailServer, None);
    assert_eq!((report.totals.agree, report.totals.total), (1, 1));
}
