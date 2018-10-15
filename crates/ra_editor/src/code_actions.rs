use join_to_string::join;

use ra_syntax::{
    File, TextUnit, TextRange, Direction,
    ast::{self, AstNode, AttrsOwner, TypeParamsOwner, NameOwner},
    SyntaxKind::{COMMA, WHITESPACE},
    SyntaxNodeRef,
    algo::{
        find_leaf_at_offset,
        find_covering_node,
    },
};

use crate::{EditBuilder, Edit, find_node_at_offset};

#[derive(Debug)]
pub struct LocalEdit {
    pub edit: Edit,
    pub cursor_position: Option<TextUnit>,
}

pub fn flip_comma<'a>(file: &'a File, offset: TextUnit) -> Option<impl FnOnce() -> LocalEdit + 'a> {
    let syntax = file.syntax();

    let comma = find_leaf_at_offset(syntax, offset).find(|leaf| leaf.kind() == COMMA)?;
    let prev = non_trivia_sibling(comma, Direction::Prev)?;
    let next = non_trivia_sibling(comma, Direction::Next)?;
    Some(move || {
        let mut edit = EditBuilder::new();
        edit.replace(prev.range(), next.text().to_string());
        edit.replace(next.range(), prev.text().to_string());
        LocalEdit {
            edit: edit.finish(),
            cursor_position: None,
        }
    })
}

pub fn add_derive<'a>(file: &'a File, offset: TextUnit) -> Option<impl FnOnce() -> LocalEdit + 'a> {
    let nominal = find_node_at_offset::<ast::NominalDef>(file.syntax(), offset)?;
    Some(move || {
        let derive_attr = nominal
            .attrs()
            .filter_map(|x| x.as_call())
            .filter(|(name, _arg)| name == "derive")
            .map(|(_name, arg)| arg)
            .next();
        let mut edit = EditBuilder::new();
        let offset = match derive_attr {
            None => {
                let node_start = nominal.syntax().range().start();
                edit.insert(node_start, "#[derive()]\n".to_string());
                node_start + TextUnit::of_str("#[derive(")
            }
            Some(tt) => {
                tt.syntax().range().end() - TextUnit::of_char(')')
            }
        };
        LocalEdit {
            edit: edit.finish(),
            cursor_position: Some(offset),
        }
    })
}

pub fn add_impl<'a>(file: &'a File, offset: TextUnit) -> Option<impl FnOnce() -> LocalEdit + 'a> {
    let nominal = find_node_at_offset::<ast::NominalDef>(file.syntax(), offset)?;
    let name = nominal.name()?;

    Some(move || {
        let type_params = nominal.type_param_list();
        let mut edit = EditBuilder::new();
        let start_offset = nominal.syntax().range().end();
        let mut buf = String::new();
        buf.push_str("\n\nimpl");
        if let Some(type_params) = type_params {
            type_params.syntax().text()
                .push_to(&mut buf);
        }
        buf.push_str(" ");
        buf.push_str(name.text().as_str());
        if let Some(type_params) = type_params {
            let lifetime_params = type_params.lifetime_params().filter_map(|it| it.lifetime()).map(|it| it.text());
            let type_params = type_params.type_params().filter_map(|it| it.name()).map(|it| it.text());
            join(lifetime_params.chain(type_params))
                .surround_with("<", ">")
                .to_buf(&mut buf);
        }
        buf.push_str(" {\n");
        let offset = start_offset + TextUnit::of_str(&buf);
        buf.push_str("\n}");
        edit.insert(start_offset, buf);
        LocalEdit {
            edit: edit.finish(),
            cursor_position: Some(offset),
        }
    })
}

pub fn introduce_variable<'a>(file: &'a File, range: TextRange) -> Option<impl FnOnce() -> LocalEdit + 'a> {
    let node = find_covering_node(file.syntax(), range);
    let expr = node.ancestors().filter_map(ast::Expr::cast).next()?;
    let anchor_stmt = expr.syntax().ancestors().filter_map(ast::Stmt::cast).next()?;
    let indent = anchor_stmt.syntax().prev_sibling()?;
    if indent.kind() != WHITESPACE {
        return None;
    }
    Some(move || {
        let mut buf = String::new();
        let mut edit = EditBuilder::new();

        buf.push_str("let var_name = ");
        expr.syntax().text().push_to(&mut buf);
        if expr.syntax().range().start() == anchor_stmt.syntax().range().start() {
            edit.replace(expr.syntax().range(), buf);
        } else {
            buf.push_str(";");
            indent.text().push_to(&mut buf);
            edit.replace(expr.syntax().range(), "var_name".to_string());
            edit.insert(anchor_stmt.syntax().range().start(), buf);
        }
        let cursor_position = anchor_stmt.syntax().range().start() + TextUnit::of_str("let ");
        LocalEdit {
            edit: edit.finish(),
            cursor_position: Some(cursor_position),
        }
    })
}

fn non_trivia_sibling(node: SyntaxNodeRef, direction: Direction) -> Option<SyntaxNodeRef> {
    node.siblings(direction)
        .skip(1)
        .find(|node| !node.kind().is_trivia())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{check_action, check_action_range};

    #[test]
    fn test_swap_comma() {
        check_action(
            "fn foo(x: i32,<|> y: Result<(), ()>) {}",
            "fn foo(y: Result<(), ()>,<|> x: i32) {}",
            |file, off| flip_comma(file, off).map(|f| f()),
        )
    }

    #[test]
    fn test_add_derive() {
        check_action(
            "struct Foo { a: i32, <|>}",
            "#[derive(<|>)]\nstruct Foo { a: i32, }",
            |file, off| add_derive(file, off).map(|f| f()),
        );
        check_action(
            "struct Foo { <|> a: i32, }",
            "#[derive(<|>)]\nstruct Foo {  a: i32, }",
            |file, off| add_derive(file, off).map(|f| f()),
        );
        check_action(
            "#[derive(Clone)]\nstruct Foo { a: i32<|>, }",
            "#[derive(Clone<|>)]\nstruct Foo { a: i32, }",
            |file, off| add_derive(file, off).map(|f| f()),
        );
    }

    #[test]
    fn test_add_impl() {
        check_action(
            "struct Foo {<|>}\n",
            "struct Foo {}\n\nimpl Foo {\n<|>\n}\n",
            |file, off| add_impl(file, off).map(|f| f()),
        );
        check_action(
            "struct Foo<T: Clone> {<|>}",
            "struct Foo<T: Clone> {}\n\nimpl<T: Clone> Foo<T> {\n<|>\n}",
            |file, off| add_impl(file, off).map(|f| f()),
        );
        check_action(
            "struct Foo<'a, T: Foo<'a>> {<|>}",
            "struct Foo<'a, T: Foo<'a>> {}\n\nimpl<'a, T: Foo<'a>> Foo<'a, T> {\n<|>\n}",
            |file, off| add_impl(file, off).map(|f| f()),
        );
    }

    #[test]
    fn test_intrdoduce_var_simple() {
        check_action_range(
            "
fn foo() {
    foo(<|>1 + 1<|>);
}", "
fn foo() {
    let <|>var_name = 1 + 1;
    foo(var_name);
}",
            |file, range| introduce_variable(file, range).map(|f| f()),
        );
    }
    #[test]
    fn test_intrdoduce_var_expr_stmt() {
check_action_range(
            "
fn foo() {
    <|>1 + 1<|>;
}", "
fn foo() {
    let <|>var_name = 1 + 1;
}",
            |file, range| introduce_variable(file, range).map(|f| f()),
        );
    }

}
