use hir::{Ty, Def};

use crate::Cancelable;
use crate::completion::{CompletionContext, Completions, CompletionKind, CompletionItem, CompletionItemKind};

/// Complete dot accesses, i.e. fields or methods (currently only fields).
pub(super) fn complete_dot(acc: &mut Completions, ctx: &CompletionContext) -> Cancelable<()> {
    let (function, receiver) = match (&ctx.function, ctx.dot_receiver) {
        (Some(function), Some(receiver)) => (function, receiver),
        _ => return Ok(()),
    };
    let infer_result = function.infer(ctx.db)?;
    let syntax_mapping = function.body_syntax_mapping(ctx.db)?;
    let expr = match syntax_mapping.node_expr(receiver) {
        Some(expr) => expr,
        None => return Ok(()),
    };
    let receiver_ty = infer_result[expr].clone();
    if !ctx.is_call {
        complete_fields(acc, ctx, receiver_ty.clone())?;
    }
    complete_methods(acc, ctx, receiver_ty)?;
    Ok(())
}

fn complete_fields(acc: &mut Completions, ctx: &CompletionContext, receiver: Ty) -> Cancelable<()> {
    for receiver in receiver.autoderef(ctx.db) {
        match receiver {
            Ty::Adt { def_id, .. } => {
                match def_id.resolve(ctx.db)? {
                    Def::Struct(s) => {
                        for field in s.fields(ctx.db) {
                            CompletionItem::new(
                                CompletionKind::Reference,
                                field.name().to_string(),
                            )
                            .kind(CompletionItemKind::Field)
                            .set_detail(field.ty(ctx.db)?.map(|ty| ty.to_string()))
                            .add_to(acc);
                        }
                    }
                    // TODO unions
                    _ => {}
                }
            }
            Ty::Tuple(fields) => {
                for (i, _ty) in fields.iter().enumerate() {
                    CompletionItem::new(CompletionKind::Reference, i.to_string())
                        .kind(CompletionItemKind::Field)
                        .add_to(acc);
                }
            }
            _ => {}
        };
    }
    Ok(())
}

fn complete_methods(
    acc: &mut Completions,
    ctx: &CompletionContext,
    receiver: Ty,
) -> Cancelable<()> {
    receiver.iterate_methods(ctx.db, |func| {
        let sig = func.signature(ctx.db);
        if sig.has_self_param() {
            CompletionItem::new(CompletionKind::Reference, sig.name().to_string())
                .from_function(ctx, func)
                .kind(CompletionItemKind::Method)
                .add_to(acc);
        }
        Ok(None::<()>)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::completion::*;

    fn check_ref_completion(code: &str, expected_completions: &str) {
        check_completion(code, expected_completions, CompletionKind::Reference);
    }

    #[test]
    fn test_struct_field_completion() {
        check_ref_completion(
            r"
            struct A { the_field: u32 }
            fn foo(a: A) {
               a.<|>
            }
            ",
            r#"the_field "u32""#,
        );
    }

    #[test]
    fn test_struct_field_completion_self() {
        check_ref_completion(
            r"
            struct A { the_field: (u32,) }
            impl A {
                fn foo(self) {
                    self.<|>
                }
            }
            ",
            r#"the_field "(u32,)"
               foo "foo($0)""#,
        );
    }

    #[test]
    fn test_struct_field_completion_autoderef() {
        check_ref_completion(
            r"
            struct A { the_field: (u32, i32) }
            impl A {
                fn foo(&self) {
                    self.<|>
                }
            }
            ",
            r#"the_field "(u32, i32)"
               foo "foo($0)""#,
        );
    }

    #[test]
    fn test_no_struct_field_completion_for_method_call() {
        check_ref_completion(
            r"
            struct A { the_field: u32 }
            fn foo(a: A) {
               a.<|>()
            }
            ",
            r#""#,
        );
    }

    #[test]
    fn test_method_completion() {
        check_ref_completion(
            r"
            struct A {}
            impl A {
                fn the_method(&self) {}
            }
            fn foo(a: A) {
               a.<|>
            }
            ",
            r#"the_method "the_method($0)""#,
        );
    }

    #[test]
    fn test_no_non_self_method() {
        check_ref_completion(
            r"
            struct A {}
            impl A {
                fn the_method() {}
            }
            fn foo(a: A) {
               a.<|>
            }
            ",
            r#""#,
        );
    }
}
