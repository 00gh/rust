mod support;

use ra_lsp_server::req::{Runnables, RunnablesParams};

use crate::support::project;

const LOG: &'static str = "";

#[test]
fn test_runnables_no_project() {
    let server = project(
        r"
//- lib.rs
#[test]
fn foo() {
}
",
    );
    server.request::<Runnables>(
        RunnablesParams {
            text_document: server.doc_id("lib.rs"),
            position: None,
        },
        r#"[
          {
            "args": [ "test", "--", "foo", "--nocapture" ],
            "bin": "cargo",
            "env": { "RUST_BACKTRACE": "short" },
            "label": "test foo",
            "range": {
              "end": { "character": 1, "line": 2 },
              "start": { "character": 0, "line": 0 }
            }
          },
          {
            "args": [
              "check",
              "--all"
            ],
            "bin": "cargo",
            "env": {},
            "label": "cargo check --all",
            "range": {
              "end": {
                "character": 0,
                "line": 0
              },
              "start": {
                "character": 0,
                "line": 0
              }
            }
          }
        ]"#,
    );
}

#[test]
fn test_runnables_project() {
    let server = project(
        r#"
//- Cargo.toml
[package]
name = "foo"
version = "0.0.0"

//- src/lib.rs
pub fn foo() {}

//- tests/spam.rs
#[test]
fn test_eggs() {}
"#,
    );
    server.wait_for_feedback("workspace loaded");
    server.request::<Runnables>(
        RunnablesParams {
            text_document: server.doc_id("tests/spam.rs"),
            position: None,
        },
        r#"[
          {
            "args": [ "test", "--package", "foo", "--test", "spam", "--", "test_eggs", "--nocapture" ],
            "bin": "cargo",
            "env": { "RUST_BACKTRACE": "short" },
            "label": "test test_eggs",
            "range": {
              "end": { "character": 17, "line": 1 },
              "start": { "character": 0, "line": 0 }
            }
          },
          {
            "args": [
              "check",
              "--package",
              "foo",
              "--test",
              "spam"
            ],
            "bin": "cargo",
            "env": {},
            "label": "cargo check -p foo",
            "range": {
              "end": {
                "character": 0,
                "line": 0
              },
              "start": {
                "character": 0,
                "line": 0
              }
            }
          }
        ]"#
    );
}
