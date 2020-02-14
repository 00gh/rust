//! FIXME: write short doc here

pub mod not_bash;
pub mod install;
pub mod pre_commit;

pub mod codegen;
mod ast_src;

use anyhow::Context;
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    codegen::Mode,
    not_bash::{pushd, run},
};

pub use anyhow::Result;

const TOOLCHAIN: &str = "stable";

pub fn project_root() -> PathBuf {
    Path::new(
        &env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_owned()),
    )
    .ancestors()
    .nth(1)
    .unwrap()
    .to_path_buf()
}

pub fn run_rustfmt(mode: Mode) -> Result<()> {
    ensure_rustfmt()?;

    if mode == Mode::Verify {
        run!("rustup run {} -- cargo fmt -- --check", TOOLCHAIN)?;
    } else {
        run!("rustup run {} -- cargo fmt", TOOLCHAIN)?;
    }
    Ok(())
}

fn reformat(text: impl std::fmt::Display) -> Result<String> {
    ensure_rustfmt()?;
    let mut rustfmt = Command::new("rustup")
        .args(&["run", TOOLCHAIN, "--", "rustfmt", "--config-path"])
        .arg(project_root().join("rustfmt.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    write!(rustfmt.stdin.take().unwrap(), "{}", text)?;
    let output = rustfmt.wait_with_output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let preamble = "Generated file, do not edit by hand, see `xtask/src/codegen`";
    Ok(format!("//! {}\n\n{}", preamble, stdout))
}

fn ensure_rustfmt() -> Result<()> {
    match Command::new("rustup")
        .args(&["run", TOOLCHAIN, "--", "cargo", "fmt", "--version"])
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => return Ok(()),
        _ => (),
    };
    run!("rustup toolchain install {}", TOOLCHAIN)?;
    run!("rustup component add rustfmt --toolchain {}", TOOLCHAIN)?;
    Ok(())
}

pub fn run_clippy() -> Result<()> {
    match Command::new("rustup")
        .args(&["run", TOOLCHAIN, "--", "cargo", "clippy", "--version"])
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => (),
        _ => install_clippy().context("install clippy")?,
    };

    let allowed_lints = [
        "clippy::collapsible_if",
        "clippy::map_clone", // FIXME: remove when Iterator::copied stabilizes (1.36.0)
        "clippy::needless_pass_by_value",
        "clippy::nonminimal_bool",
        "clippy::redundant_pattern_matching",
    ];
    run!(
        "rustup run {} -- cargo clippy --all-features --all-targets -- -A {}",
        TOOLCHAIN,
        allowed_lints.join(" -A ")
    )?;
    Ok(())
}

fn install_clippy() -> Result<()> {
    run!("rustup toolchain install {}", TOOLCHAIN)?;
    run!("rustup component add clippy --toolchain {}", TOOLCHAIN)?;
    Ok(())
}

pub fn run_fuzzer() -> Result<()> {
    let _d = pushd("./crates/ra_syntax");
    match run!("cargo fuzz --help") {
        Ok(_) => (),
        _ => {
            run!("cargo install cargo-fuzz")?;
        }
    };

    run!("rustup run nightly -- cargo fuzz run parser")?;
    Ok(())
}

/// Cleans the `./target` dir after the build such that only
/// dependencies are cached on CI.
pub fn run_pre_cache() -> Result<()> {
    let slow_tests_cookie = Path::new("./target/.slow_tests_cookie");
    if !slow_tests_cookie.exists() {
        panic!("slow tests were skipped on CI!")
    }
    rm_rf(slow_tests_cookie)?;

    for entry in Path::new("./target/debug").read_dir()? {
        let entry = entry?;
        if entry.file_type().map(|it| it.is_file()).ok() == Some(true) {
            // Can't delete yourself on windows :-(
            if !entry.path().ends_with("xtask.exe") {
                rm_rf(&entry.path())?
            }
        }
    }

    fs::remove_file("./target/.rustc_info.json")?;
    let to_delete = ["ra_", "heavy_test"];
    for &dir in ["./target/debug/deps", "target/debug/.fingerprint"].iter() {
        for entry in Path::new(dir).read_dir()? {
            let entry = entry?;
            if to_delete.iter().any(|&it| entry.path().display().to_string().contains(it)) {
                rm_rf(&entry.path())?
            }
        }
    }

    Ok(())
}

fn rm_rf(path: &Path) -> Result<()> {
    if path.is_file() { fs::remove_file(path) } else { fs::remove_dir_all(path) }
        .with_context(|| format!("failed to remove {:?}", path))
}

pub fn run_release(dry_run: bool) -> Result<()> {
    if !dry_run {
        run!("git switch release")?;
        run!("git fetch upstream")?;
        run!("git reset --hard upstream/master")?;
        run!("git push")?;
    }

    let changelog_dir = project_root().join("../rust-analyzer.github.io/thisweek/_posts");

    let today = run!("date --iso")?;
    let commit = run!("git rev-parse HEAD")?;
    let changelog_n = fs::read_dir(changelog_dir.as_path())?.count();

    let contents = format!(
        "\
= Changelog #{}
:sectanchors:
:page-layout: post

Commit: commit:{}[] +
Release: release:{}[]

== New Features

* pr:[] .

== Fixes

== Internal Improvements
",
        changelog_n, commit, today
    );

    let path = changelog_dir.join(format!("{}-changelog-{}.adoc", today, changelog_n));
    fs::write(&path, &contents)?;

    Ok(())
}
