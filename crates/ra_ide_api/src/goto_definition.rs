use ra_db::{FileId, Cancelable, SyntaxDatabase};
use ra_syntax::{
    AstNode, ast,
    algo::find_node_at_offset,
};

use crate::{FilePosition, NavigationTarget, db::RootDatabase, RangeInfo};

pub(crate) fn goto_definition(
    db: &RootDatabase,
    position: FilePosition,
) -> Cancelable<Option<RangeInfo<Vec<NavigationTarget>>>> {
    let file = db.source_file(position.file_id);
    let syntax = file.syntax();
    if let Some(name_ref) = find_node_at_offset::<ast::NameRef>(syntax, position.offset) {
        let navs = reference_definition(db, position.file_id, name_ref)?.to_vec();
        return Ok(Some(RangeInfo::new(
            name_ref.syntax().range(),
            navs.to_vec(),
        )));
    }
    if let Some(name) = find_node_at_offset::<ast::Name>(syntax, position.offset) {
        let navs = ctry!(name_definition(db, position.file_id, name)?);
        return Ok(Some(RangeInfo::new(name.syntax().range(), navs)));
    }
    Ok(None)
}

pub(crate) enum ReferenceResult {
    Exact(NavigationTarget),
    Approximate(Vec<NavigationTarget>),
}

impl ReferenceResult {
    fn to_vec(self) -> Vec<NavigationTarget> {
        use self::ReferenceResult::*;
        match self {
            Exact(target) => vec![target],
            Approximate(vec) => vec,
        }
    }
}

pub(crate) fn reference_definition(
    db: &RootDatabase,
    file_id: FileId,
    name_ref: &ast::NameRef,
) -> Cancelable<ReferenceResult> {
    use self::ReferenceResult::*;
    if let Some(function) =
        hir::source_binder::function_from_child_node(db, file_id, name_ref.syntax())
    {
        let scope = function.scopes(db);
        // First try to resolve the symbol locally
        if let Some(entry) = scope.resolve_local_name(name_ref) {
            let nav = NavigationTarget::from_scope_entry(file_id, &entry);
            return Ok(Exact(nav));
        };

        // Next check if it is a method
        if let Some(method_call) = name_ref
            .syntax()
            .parent()
            .and_then(ast::MethodCallExpr::cast)
        {
            let infer_result = function.infer(db)?;
            let syntax_mapping = function.body_syntax_mapping(db);
            let expr = ast::Expr::cast(method_call.syntax()).unwrap();
            if let Some(def_id) = syntax_mapping
                .node_expr(expr)
                .and_then(|it| infer_result.method_resolution(it))
            {
                if let Some(target) = NavigationTarget::from_def(db, def_id.resolve(db)) {
                    return Ok(Exact(target));
                }
            };
        }
    }
    // Then try module name resolution
    if let Some(module) = hir::source_binder::module_from_child_node(db, file_id, name_ref.syntax())
    {
        if let Some(path) = name_ref
            .syntax()
            .ancestors()
            .find_map(ast::Path::cast)
            .and_then(hir::Path::from_ast)
        {
            let resolved = module.resolve_path(db, &path)?;
            if let Some(def_id) = resolved.take_types().or(resolved.take_values()) {
                if let Some(target) = NavigationTarget::from_def(db, def_id.resolve(db)) {
                    return Ok(Exact(target));
                }
            }
        }
    }
    // If that fails try the index based approach.
    let navs = db
        .index_resolve(name_ref)
        .into_iter()
        .map(NavigationTarget::from_symbol)
        .collect();
    Ok(Approximate(navs))
}

fn name_definition(
    db: &RootDatabase,
    file_id: FileId,
    name: &ast::Name,
) -> Cancelable<Option<Vec<NavigationTarget>>> {
    if let Some(module) = name.syntax().parent().and_then(ast::Module::cast) {
        if module.has_semi() {
            if let Some(child_module) =
                hir::source_binder::module_from_declaration(db, file_id, module)
            {
                let nav = NavigationTarget::from_module(db, child_module);
                return Ok(Some(vec![nav]));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use crate::mock_analysis::analysis_and_position;

    fn check_goto(fixuture: &str, expected: &str) {
        let (analysis, pos) = analysis_and_position(fixuture);

        let mut navs = analysis.goto_definition(pos).unwrap().unwrap().info;
        assert_eq!(navs.len(), 1);
        let nav = navs.pop().unwrap();
        nav.assert_match(expected);
    }

    #[test]
    fn goto_definition_works_in_items() {
        check_goto(
            "
            //- /lib.rs
            struct Foo;
            enum E { X(Foo<|>) }
            ",
            "Foo STRUCT_DEF FileId(1) [0; 11) [7; 10)",
        );
    }

    #[test]
    fn goto_definition_resolves_correct_name() {
        check_goto(
            "
            //- /lib.rs
            use a::Foo;
            mod a;
            mod b;
            enum E { X(Foo<|>) }
            //- /a.rs
            struct Foo;
            //- /b.rs
            struct Foo;
            ",
            "Foo STRUCT_DEF FileId(2) [0; 11) [7; 10)",
        );
    }

    #[test]
    fn goto_definition_works_for_module_declaration() {
        check_goto(
            "
            //- /lib.rs
            mod <|>foo;
            //- /foo.rs
            // empty
            ",
            "foo SOURCE_FILE FileId(2) [0; 10)",
        );

        check_goto(
            "
            //- /lib.rs
            mod <|>foo;
            //- /foo/mod.rs
            // empty
            ",
            "foo SOURCE_FILE FileId(2) [0; 10)",
        );
    }

    #[test]
    fn goto_definition_works_for_methods() {
        check_goto(
            "
            //- /lib.rs
            struct Foo;
            impl Foo {
                fn frobnicate(&self) {  }
            }

            fn bar(foo: &Foo) {
                foo.frobnicate<|>();
            }
            ",
            "frobnicate FN_DEF FileId(1) [27; 52) [30; 40)",
        );

        check_goto(
            "
            //- /lib.rs
            mod <|>foo;
            //- /foo/mod.rs
            // empty
            ",
            "foo SOURCE_FILE FileId(2) [0; 10)",
        );
    }
}
