//! Compile-fail tests for `#[adk_rs::tool]` misuse diagnostics.
//!
//! Regenerate the expected output after intentional changes with
//! `TRYBUILD=overwrite cargo test --test macro_trybuild --features macros`.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
