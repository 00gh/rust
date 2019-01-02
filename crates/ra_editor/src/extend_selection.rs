use ra_syntax::{
    algo::{find_covering_node, find_leaf_at_offset, LeafAtOffset},
    Direction,
    SyntaxKind::*,
    SyntaxNodeRef, TextRange, TextUnit,
};

pub fn extend_selection(root: SyntaxNodeRef, range: TextRange) -> Option<TextRange> {
    let string_kinds = [COMMENT, STRING, RAW_STRING, BYTE_STRING, RAW_BYTE_STRING];
    if range.is_empty() {
        let offset = range.start();
        let mut leaves = find_leaf_at_offset(root, offset);
        if leaves.clone().all(|it| it.kind() == WHITESPACE) {
            return Some(extend_ws(root, leaves.next()?, offset));
        }
        let leaf_range = match leaves {
            LeafAtOffset::None => return None,
            LeafAtOffset::Single(l) => {
                if string_kinds.contains(&l.kind()) {
                    extend_single_word_in_comment_or_string(l, offset).unwrap_or_else(|| l.range())
                } else {
                    l.range()
                }
            }
            LeafAtOffset::Between(l, r) => pick_best(l, r).range(),
        };
        return Some(leaf_range);
    };
    let node = find_covering_node(root, range);
    if string_kinds.contains(&node.kind()) && range == node.range() {
        if let Some(range) = extend_comments(node) {
            return Some(range);
        }
    }

    match node.ancestors().skip_while(|n| n.range() == range).next() {
        None => None,
        Some(parent) => Some(parent.range()),
    }
}

fn extend_single_word_in_comment_or_string(
    leaf: SyntaxNodeRef,
    offset: TextUnit,
) -> Option<TextRange> {
    let text: &str = leaf.leaf_text()?;
    let cursor_position: u32 = (offset - leaf.range().start()).into();

    let (before, after) = text.split_at(cursor_position as usize);

    fn non_word_char(c: char) -> bool {
        !(c.is_alphanumeric() || c == '_')
    }

    let start_idx = before.rfind(non_word_char)? as u32;
    let end_idx = after.find(non_word_char).unwrap_or(after.len()) as u32;

    let from: TextUnit = (start_idx + 1).into();
    let to: TextUnit = (cursor_position + end_idx).into();

    let range = TextRange::from_to(from, to);
    if range.is_empty() {
        None
    } else {
        Some(range + leaf.range().start())
    }
}

fn extend_ws(root: SyntaxNodeRef, ws: SyntaxNodeRef, offset: TextUnit) -> TextRange {
    let ws_text = ws.leaf_text().unwrap();
    let suffix = TextRange::from_to(offset, ws.range().end()) - ws.range().start();
    let prefix = TextRange::from_to(ws.range().start(), offset) - ws.range().start();
    let ws_suffix = &ws_text.as_str()[suffix];
    let ws_prefix = &ws_text.as_str()[prefix];
    if ws_text.contains('\n') && !ws_suffix.contains('\n') {
        if let Some(node) = ws.next_sibling() {
            let start = match ws_prefix.rfind('\n') {
                Some(idx) => ws.range().start() + TextUnit::from((idx + 1) as u32),
                None => node.range().start(),
            };
            let end = if root.text().char_at(node.range().end()) == Some('\n') {
                node.range().end() + TextUnit::of_char('\n')
            } else {
                node.range().end()
            };
            return TextRange::from_to(start, end);
        }
    }
    ws.range()
}

fn pick_best<'a>(l: SyntaxNodeRef<'a>, r: SyntaxNodeRef<'a>) -> SyntaxNodeRef<'a> {
    return if priority(r) > priority(l) { r } else { l };
    fn priority(n: SyntaxNodeRef) -> usize {
        match n.kind() {
            WHITESPACE => 0,
            IDENT | SELF_KW | SUPER_KW | CRATE_KW | LIFETIME => 2,
            _ => 1,
        }
    }
}

fn extend_comments(node: SyntaxNodeRef) -> Option<TextRange> {
    let prev = adj_comments(node, Direction::Prev);
    let next = adj_comments(node, Direction::Next);
    if prev != next {
        Some(TextRange::from_to(prev.range().start(), next.range().end()))
    } else {
        None
    }
}

fn adj_comments(node: SyntaxNodeRef, dir: Direction) -> SyntaxNodeRef {
    let mut res = node;
    for node in node.siblings(dir) {
        match node.kind() {
            COMMENT => res = node,
            WHITESPACE if !node.leaf_text().unwrap().as_str().contains("\n\n") => (),
            _ => break,
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use ra_syntax::SourceFileNode;
    use test_utils::extract_offset;

    fn do_check(before: &str, afters: &[&str]) {
        let (cursor, before) = extract_offset(before);
        let file = SourceFileNode::parse(&before);
        let mut range = TextRange::offset_len(cursor, 0.into());
        for &after in afters {
            range = extend_selection(file.syntax(), range).unwrap();
            let actual = &before[range];
            assert_eq!(after, actual);
        }
    }

    #[test]
    fn test_extend_selection_arith() {
        do_check(r#"fn foo() { <|>1 + 1 }"#, &["1", "1 + 1", "{ 1 + 1 }"]);
    }

    #[test]
    fn test_extend_selection_start_of_the_lind() {
        do_check(
            r#"
impl S {
<|>    fn foo() {

    }
}"#,
            &["    fn foo() {\n\n    }\n"],
        );
    }

    #[test]
    fn test_extend_selection_doc_comments() {
        do_check(
            r#"
struct A;

/// bla
/// bla
struct B {
    <|>
}
            "#,
            &[
                "\n    \n",
                "{\n    \n}",
                "/// bla\n/// bla\nstruct B {\n    \n}",
            ],
        )
    }

    #[test]
    fn test_extend_selection_comments() {
        do_check(
            r#"
fn bar(){}

// fn foo() {
// 1 + <|>1
// }

// fn foo(){}
    "#,
            &["1", "// 1 + 1", "// fn foo() {\n// 1 + 1\n// }"],
        );

        do_check(
            r#"
// #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// pub enum Direction {
//  <|>   Next,
//     Prev
// }
"#,
            &[
                "//     Next,",
                "// #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n// pub enum Direction {\n//     Next,\n//     Prev\n// }",
            ],
        );

        do_check(
            r#"
/*
foo
_bar1<|>*/
    "#,
            &["_bar1", "/*\nfoo\n_bar1*/"],
        );

        do_check(
            r#"
//!<|>foo_2 bar
    "#,
            &["foo_2", "//!foo_2 bar"],
        );

        do_check(
            r#"
/<|>/foo bar
    "#,
            &["//foo bar"],
        );
    }

    #[test]
    fn test_extend_selection_prefer_idents() {
        do_check(
            r#"
fn main() { foo<|>+bar;}
    "#,
            &["foo", "foo+bar"],
        );
        do_check(
            r#"
fn main() { foo+<|>bar;}
    "#,
            &["bar", "foo+bar"],
        );
    }

    #[test]
    fn test_extend_selection_prefer_lifetimes() {
        do_check(r#"fn foo<<|>'a>() {}"#, &["'a", "<'a>"]);
        do_check(r#"fn foo<'a<|>>() {}"#, &["'a", "<'a>"]);
    }

    #[test]
    fn test_extend_selection_select_first_word() {
        do_check(r#"// foo bar b<|>az quxx"#, &["baz", "// foo bar baz quxx"]);
        do_check(
            r#"
impl S {
    fn foo() {
        // hel<|>lo world
    }
}
        "#,
            &["hello", "// hello world"],
        );
    }

    #[test]
    fn test_extend_selection_string() {
        do_check(
            r#"
fn bar(){}

" fn f<|>oo() {"
    "#,
            &["foo", "\" fn foo() {\""],
        );
    }
}
