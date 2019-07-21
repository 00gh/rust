use hir::Substs;

use crate::completion::{CompletionContext, Completions};

pub(super) fn complete_struct_pattern(acc: &mut Completions, ctx: &CompletionContext) {
    let (ty, variant) = match ctx.struct_lit_pat.as_ref().and_then(|it| {
        Some((
            ctx.analyzer.type_of_pat(ctx.db, &it.clone().into())?,
            ctx.analyzer.resolve_struct_pattern(it)?,
        ))
    }) {
        Some(it) => it,
        _ => return,
    };
    let substs = &ty.substs().unwrap_or_else(Substs::empty);

    for field in variant.fields(ctx.db) {
        acc.add_field(ctx, field, substs);
    }
}

#[cfg(test)]
mod tests {
    use crate::completion::{do_completion, CompletionItem, CompletionKind};
    use insta::assert_debug_snapshot_matches;

    fn complete(code: &str) -> Vec<CompletionItem> {
        do_completion(code, CompletionKind::Reference)
    }

    #[test]
    fn test_struct_pattern_field() {
        let completions = complete(
            r"
            struct S { foo: u32 }

            fn process(f: S) {
                match f {
                    S { f<|>: 92 } => (),
                }
            }
            ",
        );
        assert_debug_snapshot_matches!(completions, @r###"
       ⋮[
       ⋮    CompletionItem {
       ⋮        label: "foo",
       ⋮        source_range: [117; 118),
       ⋮        delete: [117; 118),
       ⋮        insert: "foo",
       ⋮        kind: Field,
       ⋮        detail: "u32",
       ⋮    },
       ⋮]
        "###);
    }

    #[test]
    fn test_struct_pattern_enum_variant() {
        let completions = complete(
            r"
            enum E {
                S { foo: u32, bar: () }
            }

            fn process(e: E) {
                match e {
                    E::S { <|> } => (),
                }
            }
            ",
        );
        assert_debug_snapshot_matches!(completions, @r###"
       ⋮[
       ⋮    CompletionItem {
       ⋮        label: "bar",
       ⋮        source_range: [161; 161),
       ⋮        delete: [161; 161),
       ⋮        insert: "bar",
       ⋮        kind: Field,
       ⋮        detail: "()",
       ⋮    },
       ⋮    CompletionItem {
       ⋮        label: "foo",
       ⋮        source_range: [161; 161),
       ⋮        delete: [161; 161),
       ⋮        insert: "foo",
       ⋮        kind: Field,
       ⋮        detail: "u32",
       ⋮    },
       ⋮]
        "###);
    }
}
