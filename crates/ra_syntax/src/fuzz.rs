use crate::{SourceFile, validation, TextUnit, TextRange, AstNode};
use ra_text_edit::AtomTextEdit;
use std::str::{self, FromStr};

fn check_file_invariants(file: &SourceFile) {
    let root = file.syntax();
    validation::validate_block_structure(root);
    let _ = file.errors();
}

pub fn check_parser(text: &str) {
    let file = SourceFile::parse(text);
    check_file_invariants(&file);
}

#[derive(Debug, Clone)]
pub struct CheckReparse {
    text: String,
    edit: AtomTextEdit,
    edited_text: String,
}

impl CheckReparse {
    pub fn from_data(data: &[u8]) -> Option<Self> {
        const PREFIX: &'static str = "fn main(){\n\t";
        const SUFFIX: &'static str = "\n}";

        let data = str::from_utf8(data).ok()?;
        let mut lines = data.lines();
        let delete_start = usize::from_str(lines.next()?).ok()? + PREFIX.len();
        let delete_len = usize::from_str(lines.next()?).ok()?;
        let insert = lines.next()?.to_string();
        let text = lines.collect::<Vec<_>>().join("\n");
        let text = format!("{}{}{}", PREFIX, text, SUFFIX);
        text.get(delete_start..delete_start.checked_add(delete_len)?)?; // make sure delete is a valid range
        let delete = TextRange::offset_len(
            TextUnit::from_usize(delete_start),
            TextUnit::from_usize(delete_len),
        );
        let edited_text =
            format!("{}{}{}", &text[..delete_start], &insert, &text[delete_start + delete_len..]);
        let edit = AtomTextEdit { delete, insert };
        Some(CheckReparse { text, edit, edited_text })
    }

    pub fn run(&self) {
        let file = SourceFile::parse(&self.text);
        let new_file = file.reparse(&self.edit);
        check_file_invariants(&new_file);
        assert_eq!(&new_file.syntax().text().to_string(), &self.edited_text);
        let full_reparse = SourceFile::parse(&self.edited_text);
        for (a, b) in new_file.syntax().descendants().zip(full_reparse.syntax().descendants()) {
            if (a.kind(), a.range()) != (b.kind(), b.range()) {
                eprint!("original:\n{}", file.syntax().debug_dump());
                eprint!("reparsed:\n{}", new_file.syntax().debug_dump());
                eprint!("full reparse:\n{}", full_reparse.syntax().debug_dump());
                assert_eq!(
                    format!("{:?}", a),
                    format!("{:?}", b),
                    "different syntax tree produced by the full reparse"
                );
            }
        }
        // FIXME
        // assert_eq!(new_file.errors(), full_reparse.errors());
    }
}
