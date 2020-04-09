//! utils used in proc-macro tests

use crate::dylib;
use crate::list_macros;
pub use difference::Changeset as __Changeset;
use ra_proc_macro::ListMacrosTask;
use std::str::FromStr;
use test_utils::assert_eq_text;

mod fixtures {
    use cargo_metadata::{parse_messages, Message};
    use std::process::Command;

    // Use current project metadata to get the proc-macro dylib path
    pub fn dylib_path(crate_name: &str, version: &str) -> std::path::PathBuf {
        let command = Command::new("cargo")
            .args(&["check", "--message-format", "json"])
            .output()
            .unwrap()
            .stdout;

        for message in parse_messages(command.as_slice()) {
            match message.unwrap() {
                Message::CompilerArtifact(artifact) => {
                    if artifact.target.kind.contains(&"proc-macro".to_string()) {
                        let repr = format!("{} {}", crate_name, version);
                        if artifact.package_id.repr.starts_with(&repr) {
                            return artifact.filenames[0].clone();
                        }
                    }
                }
                _ => (), // Unknown message
            }
        }

        panic!("No proc-macro dylib for {} found!", crate_name);
    }
}

fn parse_string(code: &str) -> Option<crate::rustc_server::TokenStream> {
    Some(crate::rustc_server::TokenStream::from_str(code).unwrap())
}

pub fn assert_expand(
    crate_name: &str,
    macro_name: &str,
    version: &str,
    fixture: &str,
    expect: &str,
) {
    let path = fixtures::dylib_path(crate_name, version);
    let expander = dylib::Expander::new(&path).unwrap();
    let fixture = parse_string(fixture).unwrap();

    let res = expander.expand(macro_name, &fixture.subtree, None).unwrap();
    assert_eq_text!(&format!("{:?}", res), &expect.trim());
}

pub fn list(crate_name: &str, version: &str) -> Vec<String> {
    let path = fixtures::dylib_path(crate_name, version);
    let task = ListMacrosTask { lib: path };

    let res = list_macros(&task).unwrap();
    res.macros.into_iter().map(|(name, kind)| format!("{} [{:?}]", name, kind)).collect()
}
