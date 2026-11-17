//! Fixtures and helpers shared by the CE test files.

use modfoundry_ce::edit::{apply, plan, EditError, TextEdit, XmlOp};
use modfoundry_ce::ops::{plan_all, CeOp};
use modfoundry_ce::xml::parse;
use modfoundry_ce::Span;

pub const SAMPLE_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<types>
    <type name="FieldCanteen">
        <nominal>12</nominal>
        <lifetime>14400</lifetime>
        <restock>1800</restock>
        <min>6</min>
        <quantmin>0</quantmin>
        <quantmax>40</quantmax>
        <cost>100</cost>
        <flags count_in_cargo="0" count_in_hoarder="0" count_in_map="1" count_in_player="0" crafted="1" deloot="0"/>
        <category name="tools"/>
        <usage name="Village"/>
        <usage name="Farm"/>
        <value name="Tier1"/>
        <value user="Tier12"/>
    </type>
    <type name="FilterCartridge">
        <nominal>20</nominal>
        <lifetime>14400</lifetime>
        <restock>0</restock>
        <min>24</min>
        <quantmin>-1</quantmin>
        <quantmax>-1</quantmax>
        <cost>100</cost>
        <flags count_in_cargo="1" count_in_hoarder="1" count_in_map="1" count_in_player="0" crafted="0" deloot="0"/>
        <category name="tools"/>
        <usage name="Military"/>
        <value name="Tier2"/>
        <value name="Tier3"/>
    </type>
    <type name="FieldCanteen_Olive">
        <nominal>6</nominal>
        <lifetime>14400</lifetime>
        <restock>1800</restock>
        <min>3</min>
        <quantmin>0</quantmin>
        <quantmax>40</quantmax>
        <cost>100</cost>
        <flags count_in_cargo="0" count_in_hoarder="0" count_in_map="0" count_in_player="0" crafted="0" deloot="0"/>
        <category name="tools"/>
        <usage name="Military"/>
        <value name="Tier2"/>
        <value name="Tier4"/>
    </type>
</types>
"#;

pub fn ops_run(text: &str, file_name: &str, op: &CeOp) -> String {
    plan_all(text, Some(file_name), std::slice::from_ref(op)).expect("plan")
}

pub fn ops_err(text: &str, file_name: &str, op: &CeOp) -> EditError {
    plan_all(text, Some(file_name), std::slice::from_ref(op)).expect_err("expected an error")
}

pub fn crlf_tabs(lf: &str) -> String {
    lf.replace('\n', "\r\n").replace("    ", "\t")
}

/// Asserts the plan is sorted, non-overlapping, on char boundaries, and that
/// every byte outside the edits survives unchanged — derived from the edits.
pub fn check_edits(text: &str, edits: &[TextEdit]) {
    let mut prev_end = 0;
    for edit in edits {
        assert!(
            edit.offset >= prev_end,
            "edits overlap or are unsorted: {edits:?}"
        );
        assert!(text.is_char_boundary(edit.offset), "offset boundary");
        assert!(
            text.is_char_boundary(edit.offset + edit.len),
            "edit end boundary"
        );
        prev_end = edit.offset + edit.len;
    }

    let out = apply(text, edits);
    let mut ti = 0;
    let mut oi = 0;
    for edit in edits {
        let kept = &text[ti..edit.offset];
        assert_eq!(
            kept,
            &out[oi..oi + kept.len()],
            "bytes outside an edit changed"
        );
        oi += kept.len();
        assert_eq!(&out[oi..oi + edit.text.len()], edit.text.as_str());
        oi += edit.text.len();
        ti = edit.offset + edit.len;
    }
    assert_eq!(&text[ti..], &out[oi..], "bytes outside an edit changed");
}

/// Applies an op and checks the result is clean and lossless.
pub fn edit_run(text: &str, op: &XmlOp) -> String {
    let edits = plan(text, op).expect("op should plan");
    check_edits(text, &edits);
    let out = apply(text, &edits);
    let doc = parse(&out);
    assert!(doc.issues.is_empty(), "issues after plan: {:?}", doc.issues);
    assert_eq!(doc.rebuild(&out), out, "result is not lossless");
    out
}

pub fn assert_op(text: &str, op: &XmlOp, want: &str) {
    assert_eq!(edit_run(text, op), want);
}

pub fn slice(text: &str, span: Span) -> &str {
    &text[span.start..span.end]
}

pub fn slice_opt(text: &str, span: Option<Span>) -> &str {
    slice(text, span.expect("finding has a span"))
}
