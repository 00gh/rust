extern crate clap;
#[macro_use]
extern crate failure;
extern crate ron;
extern crate tera;
extern crate tools;
extern crate walkdir;
#[macro_use]
extern crate commandspec;
extern crate heck;

use clap::{App, Arg, SubCommand};
use heck::{CamelCase, ShoutySnakeCase, SnakeCase};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use tools::{collect_tests, Test};

type Result<T> = ::std::result::Result<T, failure::Error>;

const GRAMMAR_DIR: &str = "./crates/libsyntax2/src/grammar";
const INLINE_TESTS_DIR: &str = "./crates/libsyntax2/tests/data/parser/inline";
const GRAMMAR: &str = "./crates/libsyntax2/src/grammar.ron";
const SYNTAX_KINDS: &str = "./crates/libsyntax2/src/syntax_kinds/generated.rs";
const SYNTAX_KINDS_TEMPLATE: &str = "./crates/libsyntax2/src/syntax_kinds/generated.rs.tera";
const AST: &str = "./crates/libsyntax2/src/ast/generated.rs";
const AST_TEMPLATE: &str = "./crates/libsyntax2/src/ast/generated.rs.tera";

fn main() -> Result<()> {
    let matches = App::new("tasks")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .arg(
            Arg::with_name("verify")
                .long("--verify")
                .help("Verify that generated code is up-to-date")
                .global(true),
        )
        .subcommand(SubCommand::with_name("gen-kinds"))
        .subcommand(SubCommand::with_name("gen-tests"))
        .subcommand(SubCommand::with_name("install-code"))
        .get_matches();
    match matches.subcommand() {
        ("install-code", _) => install_code_extension()?,
        (name, Some(matches)) => run_gen_command(name, matches.is_present("verify"))?,
        _ => unreachable!(),
    }
    Ok(())
}

fn run_gen_command(name: &str, verify: bool) -> Result<()> {
    match name {
        "gen-kinds" => {
            update(Path::new(SYNTAX_KINDS), &render_template(SYNTAX_KINDS_TEMPLATE)?, verify)?;
            update(Path::new(AST), &render_template(AST_TEMPLATE)?, verify)?;
        },
        "gen-tests" => {
            gen_tests(verify)?
        },
        _ => unreachable!(),
    }
    Ok(())
}

fn update(path: &Path, contents: &str, verify: bool) -> Result<()> {
    match fs::read_to_string(path) {
        Ok(ref old_contents) if old_contents == contents => {
            return Ok(());
        }
        _ => (),
    }
    if verify {
        bail!("`{}` is not up-to-date", path.display());
    }
    eprintln!("updating {}", path.display());
    fs::write(path, contents)?;
    Ok(())
}

fn render_template(template: &str) -> Result<String> {
    let grammar: ron::value::Value = {
        let text = fs::read_to_string(GRAMMAR)?;
        ron::de::from_str(&text)?
    };
    let template = fs::read_to_string(template)?;
    let mut tera = tera::Tera::default();
    tera.add_raw_template("grammar", &template)
        .map_err(|e| format_err!("template error: {:?}", e))?;
    tera.register_global_function("concat", Box::new(concat));
    tera.register_filter("camel", |arg, _| {
        Ok(arg.as_str().unwrap().to_camel_case().into())
    });
    tera.register_filter("snake", |arg, _| {
        Ok(arg.as_str().unwrap().to_snake_case().into())
    });
    tera.register_filter("SCREAM", |arg, _| {
        Ok(arg.as_str().unwrap().to_shouty_snake_case().into())
    });
    let ret = tera
        .render("grammar", &grammar)
        .map_err(|e| format_err!("template error: {:?}", e))?;
    return Ok(ret);

    fn concat(args: HashMap<String, tera::Value>) -> tera::Result<tera::Value> {
        let mut elements = Vec::new();
        for &key in ["a", "b", "c"].iter() {
            let val = match args.get(key) {
                Some(val) => val,
                None => continue,
            };
            let val = val.as_array().unwrap();
            elements.extend(val.iter().cloned());
        }
        Ok(tera::Value::Array(elements))
    }
}

fn gen_tests(verify: bool) -> Result<()> {
    let tests = tests_from_dir(Path::new(GRAMMAR_DIR))?;

    let inline_tests_dir = Path::new(INLINE_TESTS_DIR);
    if !inline_tests_dir.is_dir() {
        fs::create_dir_all(inline_tests_dir)?;
    }
    let existing = existing_tests(inline_tests_dir)?;

    for t in existing.keys().filter(|&t| !tests.contains_key(t)) {
        panic!("Test is deleted: {}", t);
    }

    let mut new_idx = existing.len() + 2;
    for (name, test) in tests {
        let path = match existing.get(&name) {
            Some((path, _test)) => path.clone(),
            None => {
                let file_name = format!("{:04}_{}.rs", new_idx, name);
                new_idx += 1;
                inline_tests_dir.join(file_name)
            }
        };
        update(&path, &test.text, verify)?;
    }
    Ok(())
}

fn tests_from_dir(dir: &Path) -> Result<HashMap<String, Test>> {
    let mut res = HashMap::new();
    for entry in ::walkdir::WalkDir::new(dir) {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().unwrap_or_default() != "rs" {
            continue;
        }
        let text = fs::read_to_string(entry.path())?;

        for (_, test) in collect_tests(&text) {
            if let Some(old_test) = res.insert(test.name.clone(), test) {
                bail!("Duplicate test: {}", old_test.name)
            }
        }
    }
    Ok(res)
}

fn existing_tests(dir: &Path) -> Result<HashMap<String, (PathBuf, Test)>> {
    let mut res = HashMap::new();
    for file in fs::read_dir(dir)? {
        let file = file?;
        let path = file.path();
        if path.extension().unwrap_or_default() != "rs" {
            continue;
        }
        let name = {
            let file_name = path.file_name().unwrap().to_str().unwrap();
            file_name[5..file_name.len() - 3].to_string()
        };
        let text = fs::read_to_string(&path)?;
        let test = Test {
            name: name.clone(),
            text,
        };
        match res.insert(name, (path, test)) {
            Some(old) => println!("Duplicate test: {:?}", old),
            None => (),
        }
    }
    Ok(res)
}

fn install_code_extension() -> Result<()> {
    execute!(r"cargo install --path crates/server --force")?;
    execute!(
        r"
cd code
npm install
    "
    )?;
    execute!(
        r"
cd code
./node_modules/vsce/out/vsce package
    "
    )?;
    execute!(
        r"
cd code
code --install-extension ./rcf-lsp-0.0.1.vsix
    "
    )?;
    Ok(())
}
