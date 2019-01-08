use ra_syntax::{SourceFile, TextRange, TextUnit};

use crate::LocalEdit;
pub use test_utils::*;

pub fn check_action<F: Fn(&SourceFile, TextUnit) -> Option<LocalEdit>>(
    before: &str,
    after: &str,
    f: F,
) {
    let (before_cursor_pos, before) = extract_offset(before);
    let file = SourceFile::parse(&before);
    let result = f(&file, before_cursor_pos).expect("code action is not applicable");
    let actual = result.edit.apply(&before);
    let actual_cursor_pos = match result.cursor_position {
        None => result.edit.apply_to_offset(before_cursor_pos).unwrap(),
        Some(off) => off,
    };
    let actual = add_cursor(&actual, actual_cursor_pos);
    assert_eq_text!(after, &actual);
}

pub fn check_action_range<F: Fn(&SourceFile, TextRange) -> Option<LocalEdit>>(
    before: &str,
    after: &str,
    f: F,
) {
    let (range, before) = extract_range(before);
    let file = SourceFile::parse(&before);
    let result = f(&file, range).expect("code action is not applicable");
    let actual = result.edit.apply(&before);
    let actual_cursor_pos = match result.cursor_position {
        None => result.edit.apply_to_offset(range.start()).unwrap(),
        Some(off) => off,
    };
    let actual = add_cursor(&actual, actual_cursor_pos);
    assert_eq_text!(after, &actual);
}
