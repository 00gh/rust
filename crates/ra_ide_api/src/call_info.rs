use test_utils::tested_by;
use ra_db::SourceDatabase;
use ra_syntax::{
    AstNode, SyntaxNode, TextUnit,
    SyntaxKind::FN_DEF,
    ast::{self, ArgListOwner},
    algo::find_node_at_offset,
};

use crate::{FilePosition, CallInfo, FunctionSignature, db::RootDatabase};

/// Computes parameter information for the given call expression.
pub(crate) fn call_info(db: &RootDatabase, position: FilePosition) -> Option<CallInfo> {
    let file = db.parse(position.file_id);
    let syntax = file.syntax();

    // Find the calling expression and it's NameRef
    let calling_node = FnCallNode::with_node(syntax, position.offset)?;
    let name_ref = calling_node.name_ref()?;

    // Resolve the function's NameRef (NOTE: this isn't entirely accurate).
    let file_symbols = crate::symbol_index::index_resolve(db, name_ref);
    let symbol = file_symbols.into_iter().find(|it| it.ptr.kind() == FN_DEF)?;
    let fn_file = db.parse(symbol.file_id);
    let fn_def = symbol.ptr.to_node(&fn_file);
    let fn_def = ast::FnDef::cast(fn_def).unwrap();
    let function = hir::source_binder::function_from_source(db, symbol.file_id, fn_def)?;

    let mut call_info = CallInfo::new(db, function);

    // If we have a calling expression let's find which argument we are on
    let num_params = call_info.parameters().len();
    let has_self = fn_def.param_list().and_then(|l| l.self_param()).is_some();

    if num_params == 1 {
        if !has_self {
            call_info.active_parameter = Some(0);
        }
    } else if num_params > 1 {
        // Count how many parameters into the call we are.
        if let Some(arg_list) = calling_node.arg_list() {
            // Number of arguments specified at the call site
            let num_args_at_callsite = arg_list.args().count();

            let arg_list_range = arg_list.syntax().range();
            if !arg_list_range.contains_inclusive(position.offset) {
                tested_by!(call_info_bad_offset);
                return None;
            }

            let mut param = std::cmp::min(
                num_args_at_callsite,
                arg_list
                    .args()
                    .take_while(|arg| arg.syntax().range().end() < position.offset)
                    .count(),
            );

            // If we are in a method account for `self`
            if has_self {
                param = param + 1;
            }

            call_info.active_parameter = Some(param);
        }
    }

    Some(call_info)
}

enum FnCallNode<'a> {
    CallExpr(&'a ast::CallExpr),
    MethodCallExpr(&'a ast::MethodCallExpr),
}

impl<'a> FnCallNode<'a> {
    pub fn with_node(syntax: &'a SyntaxNode, offset: TextUnit) -> Option<FnCallNode<'a>> {
        if let Some(expr) = find_node_at_offset::<ast::CallExpr>(syntax, offset) {
            return Some(FnCallNode::CallExpr(expr));
        }
        if let Some(expr) = find_node_at_offset::<ast::MethodCallExpr>(syntax, offset) {
            return Some(FnCallNode::MethodCallExpr(expr));
        }
        None
    }

    pub fn name_ref(&self) -> Option<&'a ast::NameRef> {
        match *self {
            FnCallNode::CallExpr(call_expr) => Some(match call_expr.expr()?.kind() {
                ast::ExprKind::PathExpr(path_expr) => path_expr.path()?.segment()?.name_ref()?,
                _ => return None,
            }),

            FnCallNode::MethodCallExpr(call_expr) => {
                call_expr.syntax().children().filter_map(ast::NameRef::cast).nth(0)
            }
        }
    }

    pub fn arg_list(&self) -> Option<&'a ast::ArgList> {
        match *self {
            FnCallNode::CallExpr(expr) => expr.arg_list(),
            FnCallNode::MethodCallExpr(expr) => expr.arg_list(),
        }
    }
}

impl CallInfo {
    fn new(db: &RootDatabase, function: hir::Function) -> Self {
        let signature = FunctionSignature::from_hir(db, function);

        CallInfo { signature, active_parameter: None }
    }

    fn parameters(&self) -> &[String] {
        &self.signature.parameters
    }
}

#[cfg(test)]
mod tests {
    use test_utils::covers;

    use crate::mock_analysis::single_file_with_position;

    use super::*;

    // These are only used when testing
    impl CallInfo {
        fn doc(&self) -> Option<hir::Documentation> {
            self.signature.doc.clone()
        }

        fn label(&self) -> String {
            self.signature.to_string()
        }
    }

    fn call_info(text: &str) -> CallInfo {
        let (analysis, position) = single_file_with_position(text);
        analysis.call_info(position).unwrap().unwrap()
    }

    #[test]
    fn test_fn_signature_two_args_first() {
        let info = call_info(
            r#"fn foo(x: u32, y: u32) -> u32 {x + y}
fn bar() { foo(<|>3, ); }"#,
        );

        assert_eq!(info.parameters(), ["x: u32", "y: u32"]);
        assert_eq!(info.active_parameter, Some(0));
    }

    #[test]
    fn test_fn_signature_two_args_second() {
        let info = call_info(
            r#"fn foo(x: u32, y: u32) -> u32 {x + y}
fn bar() { foo(3, <|>); }"#,
        );

        assert_eq!(info.parameters(), ["x: u32", "y: u32"]);
        assert_eq!(info.active_parameter, Some(1));
    }

    #[test]
    fn test_fn_signature_two_args_empty() {
        let info = call_info(
            r#"fn foo(x: u32, y: u32) -> u32 {x + y}
fn bar() { foo(<|>); }"#,
        );

        assert_eq!(info.parameters(), ["x: u32", "y: u32"]);
        assert_eq!(info.active_parameter, Some(0));
    }

    #[test]
    fn test_fn_signature_two_args_first_generics() {
        let info = call_info(
            r#"fn foo<T, U: Copy + Display>(x: T, y: U) -> u32 where T: Copy + Display, U: Debug {x + y}
fn bar() { foo(<|>3, ); }"#,
        );

        assert_eq!(info.parameters(), ["x: T", "y: U"]);
        assert_eq!(
            info.label(),
            r#"
fn foo<T, U: Copy + Display>(x: T, y: U) -> u32
where T: Copy + Display,
      U: Debug
    "#
            .trim()
        );
        assert_eq!(info.active_parameter, Some(0));
    }

    #[test]
    fn test_fn_signature_no_params() {
        let info = call_info(
            r#"fn foo<T>() -> T where T: Copy + Display {}
fn bar() { foo(<|>); }"#,
        );

        assert!(info.parameters().is_empty());
        assert_eq!(
            info.label(),
            r#"
fn foo<T>() -> T
where T: Copy + Display
    "#
            .trim()
        );
        assert!(info.active_parameter.is_none());
    }

    #[test]
    fn test_fn_signature_for_impl() {
        let info = call_info(
            r#"struct F; impl F { pub fn new() { F{}} }
fn bar() {let _ : F = F::new(<|>);}"#,
        );

        assert!(info.parameters().is_empty());
        assert_eq!(info.active_parameter, None);
    }

    #[test]
    fn test_fn_signature_for_method_self() {
        let info = call_info(
            r#"struct F;
impl F {
    pub fn new() -> F{
        F{}
    }

    pub fn do_it(&self) {}
}

fn bar() {
    let f : F = F::new();
    f.do_it(<|>);
}"#,
        );

        assert_eq!(info.parameters(), ["&self"]);
        assert_eq!(info.active_parameter, None);
    }

    #[test]
    fn test_fn_signature_for_method_with_arg() {
        let info = call_info(
            r#"struct F;
impl F {
    pub fn new() -> F{
        F{}
    }

    pub fn do_it(&self, x: i32) {}
}

fn bar() {
    let f : F = F::new();
    f.do_it(<|>);
}"#,
        );

        assert_eq!(info.parameters(), ["&self", "x: i32"]);
        assert_eq!(info.active_parameter, Some(1));
    }

    #[test]
    fn test_fn_signature_with_docs_simple() {
        let info = call_info(
            r#"
/// test
// non-doc-comment
fn foo(j: u32) -> u32 {
    j
}

fn bar() {
    let _ = foo(<|>);
}
"#,
        );

        assert_eq!(info.parameters(), ["j: u32"]);
        assert_eq!(info.active_parameter, Some(0));
        assert_eq!(info.label(), "fn foo(j: u32) -> u32");
        assert_eq!(info.doc().map(|it| it.into()), Some("test".to_string()));
    }

    #[test]
    fn test_fn_signature_with_docs() {
        let info = call_info(
            r#"
/// Adds one to the number given.
///
/// # Examples
///
/// ```
/// let five = 5;
///
/// assert_eq!(6, my_crate::add_one(5));
/// ```
pub fn add_one(x: i32) -> i32 {
    x + 1
}

pub fn do() {
    add_one(<|>
}"#,
        );

        assert_eq!(info.parameters(), ["x: i32"]);
        assert_eq!(info.active_parameter, Some(0));
        assert_eq!(info.label(), "pub fn add_one(x: i32) -> i32");
        assert_eq!(
            info.doc().map(|it| it.into()),
            Some(
                r#"Adds one to the number given.

# Examples

```
let five = 5;

assert_eq!(6, my_crate::add_one(5));
```"#
                    .to_string()
            )
        );
    }

    #[test]
    fn test_fn_signature_with_docs_impl() {
        let info = call_info(
            r#"
struct addr;
impl addr {
    /// Adds one to the number given.
    ///
    /// # Examples
    ///
    /// ```
    /// let five = 5;
    ///
    /// assert_eq!(6, my_crate::add_one(5));
    /// ```
    pub fn add_one(x: i32) -> i32 {
        x + 1
    }
}

pub fn do_it() {
    addr {};
    addr::add_one(<|>);
}"#,
        );

        assert_eq!(info.parameters(), ["x: i32"]);
        assert_eq!(info.active_parameter, Some(0));
        assert_eq!(info.label(), "pub fn add_one(x: i32) -> i32");
        assert_eq!(
            info.doc().map(|it| it.into()),
            Some(
                r#"Adds one to the number given.

# Examples

```
let five = 5;

assert_eq!(6, my_crate::add_one(5));
```"#
                    .to_string()
            )
        );
    }

    #[test]
    fn test_fn_signature_with_docs_from_actix() {
        let info = call_info(
            r#"
pub trait WriteHandler<E>
where
    Self: Actor,
    Self::Context: ActorContext,
{
    /// Method is called when writer emits error.
    ///
    /// If this method returns `ErrorAction::Continue` writer processing
    /// continues otherwise stream processing stops.
    fn error(&mut self, err: E, ctx: &mut Self::Context) -> Running {
        Running::Stop
    }

    /// Method is called when writer finishes.
    ///
    /// By default this method stops actor's `Context`.
    fn finished(&mut self, ctx: &mut Self::Context) {
        ctx.stop()
    }
}

pub fn foo() {
    WriteHandler r;
    r.finished(<|>);
}

"#,
        );

        assert_eq!(info.parameters(), ["&mut self", "ctx: &mut Self::Context"]);
        assert_eq!(info.active_parameter, Some(1));
        assert_eq!(
            info.doc().map(|it| it.into()),
            Some(
                r#"Method is called when writer finishes.

By default this method stops actor's `Context`."#
                    .to_string()
            )
        );
    }

    #[test]
    fn call_info_bad_offset() {
        covers!(call_info_bad_offset);
        let (analysis, position) = single_file_with_position(
            r#"fn foo(x: u32, y: u32) -> u32 {x + y}
               fn bar() { foo <|> (3, ); }"#,
        );
        let call_info = analysis.call_info(position).unwrap();
        assert!(call_info.is_none());
    }
}
