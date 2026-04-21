//! Additional filter tests that exercise the public surface from outside
//! the crate (the in-module tests exercise it from inside). Keeps us
//! honest about the `pub` API.

use hoangsa_proxy::filters;

#[test]
fn public_pipeline_chains() {
    let raw = "a\na\nb\nc\nc\nc\nd\n";
    let ls = filters::lines(raw);
    let grouped = filters::collapse_repeats(&ls);
    let out = filters::join(&grouped);
    assert_eq!(out, "a (x2)\nb\nc (x3)\nd\n");
}

#[test]
fn summary_reports_zero_for_no_input() {
    let s = filters::summary(0, 0);
    assert!(s.contains("0% saved"));
}
