//! `render` module provides utilities for rendering completion suggestions
//! into code pieces that will be presented to user.

mod macro_;
mod function;
mod builder_ext;
mod enum_variant;
mod const_;
mod type_alias;

use hir::{Documentation, HasAttrs, HirDisplay, Mutability, ScopeDef, Type};
use ide_db::RootDatabase;
use syntax::TextRange;
use test_utils::mark;

use crate::{
    config::SnippetCap, CompletionContext, CompletionItem, CompletionItemKind, CompletionKind,
    CompletionScore,
};

pub(crate) use crate::render::{
    const_::ConstRender, enum_variant::EnumVariantRender, function::FunctionRender,
    macro_::MacroRender, type_alias::TypeAliasRender,
};

#[derive(Debug)]
pub(crate) struct Render<'a> {
    ctx: RenderContext<'a>,
}

#[derive(Debug)]
pub(crate) struct RenderContext<'a> {
    completion: &'a CompletionContext<'a>,
}

impl<'a> RenderContext<'a> {
    fn new(completion: &'a CompletionContext<'a>) -> RenderContext<'a> {
        RenderContext { completion }
    }

    fn snippet_cap(&self) -> Option<SnippetCap> {
        self.completion.config.snippet_cap.clone()
    }

    fn db(&self) -> &'a RootDatabase {
        &self.completion.db
    }

    fn source_range(&self) -> TextRange {
        self.completion.source_range()
    }

    fn is_deprecated(&self, node: impl HasAttrs) -> bool {
        node.attrs(self.db()).by_key("deprecated").exists()
    }

    fn docs(&self, node: impl HasAttrs) -> Option<Documentation> {
        node.docs(self.db())
    }

    fn active_name_and_type(&self) -> Option<(String, Type)> {
        if let Some(record_field) = &self.completion.record_field_syntax {
            mark::hit!(record_field_type_match);
            let (struct_field, _local) = self.completion.sema.resolve_record_field(record_field)?;
            Some((struct_field.name(self.db()).to_string(), struct_field.signature_ty(self.db())))
        } else if let Some(active_parameter) = &self.completion.active_parameter {
            mark::hit!(active_param_type_match);
            Some((active_parameter.name.clone(), active_parameter.ty.clone()))
        } else {
            None
        }
    }
}

impl<'a> From<&'a CompletionContext<'a>> for RenderContext<'a> {
    fn from(ctx: &'a CompletionContext<'a>) -> RenderContext<'a> {
        RenderContext::new(ctx)
    }
}

impl<'a> Render<'a> {
    pub(crate) fn new(ctx: RenderContext<'a>) -> Render<'a> {
        Render { ctx }
    }

    pub(crate) fn add_field(&mut self, field: hir::Field, ty: &Type) -> CompletionItem {
        let is_deprecated = self.ctx.is_deprecated(field);
        let name = field.name(self.ctx.db());
        let mut item = CompletionItem::new(
            CompletionKind::Reference,
            self.ctx.source_range(),
            name.to_string(),
        )
        .kind(CompletionItemKind::Field)
        .detail(ty.display(self.ctx.db()).to_string())
        .set_documentation(field.docs(self.ctx.db()))
        .set_deprecated(is_deprecated);

        if let Some(score) = compute_score(&self.ctx, &ty, &name.to_string()) {
            item = item.set_score(score);
        }

        return item.build();
    }

    pub(crate) fn add_tuple_field(&mut self, field: usize, ty: &Type) -> CompletionItem {
        CompletionItem::new(CompletionKind::Reference, self.ctx.source_range(), field.to_string())
            .kind(CompletionItemKind::Field)
            .detail(ty.display(self.ctx.db()).to_string())
            .build()
    }

    pub(crate) fn render_resolution(
        self,
        local_name: String,
        resolution: &ScopeDef,
    ) -> Option<CompletionItem> {
        use hir::ModuleDef::*;

        let completion_kind = match resolution {
            ScopeDef::ModuleDef(BuiltinType(..)) => CompletionKind::BuiltinType,
            _ => CompletionKind::Reference,
        };

        let kind = match resolution {
            ScopeDef::ModuleDef(Function(func)) => {
                let item = FunctionRender::new(self.ctx, Some(local_name), *func).render();
                return Some(item);
            }
            ScopeDef::ModuleDef(EnumVariant(var)) => {
                let item = EnumVariantRender::new(self.ctx, Some(local_name), *var, None).render();
                return Some(item);
            }
            ScopeDef::MacroDef(mac) => {
                let item = MacroRender::new(self.ctx, local_name, *mac).render();
                return item;
            }

            ScopeDef::ModuleDef(Module(..)) => CompletionItemKind::Module,
            ScopeDef::ModuleDef(Adt(hir::Adt::Struct(_))) => CompletionItemKind::Struct,
            // FIXME: add CompletionItemKind::Union
            ScopeDef::ModuleDef(Adt(hir::Adt::Union(_))) => CompletionItemKind::Struct,
            ScopeDef::ModuleDef(Adt(hir::Adt::Enum(_))) => CompletionItemKind::Enum,
            ScopeDef::ModuleDef(Const(..)) => CompletionItemKind::Const,
            ScopeDef::ModuleDef(Static(..)) => CompletionItemKind::Static,
            ScopeDef::ModuleDef(Trait(..)) => CompletionItemKind::Trait,
            ScopeDef::ModuleDef(TypeAlias(..)) => CompletionItemKind::TypeAlias,
            ScopeDef::ModuleDef(BuiltinType(..)) => CompletionItemKind::BuiltinType,
            ScopeDef::GenericParam(..) => CompletionItemKind::TypeParam,
            ScopeDef::Local(..) => CompletionItemKind::Binding,
            // (does this need its own kind?)
            ScopeDef::AdtSelfType(..) | ScopeDef::ImplSelfType(..) => CompletionItemKind::TypeParam,
            ScopeDef::Unknown => {
                let item = CompletionItem::new(
                    CompletionKind::Reference,
                    self.ctx.source_range(),
                    local_name,
                )
                .kind(CompletionItemKind::UnresolvedReference)
                .build();
                return Some(item);
            }
        };

        let docs = self.docs(resolution);

        let mut item =
            CompletionItem::new(completion_kind, self.ctx.source_range(), local_name.clone());
        if let ScopeDef::Local(local) = resolution {
            let ty = local.ty(self.ctx.db());
            if !ty.is_unknown() {
                item = item.detail(ty.display(self.ctx.db()).to_string());
            }
        };

        let mut ref_match = None;
        if let ScopeDef::Local(local) = resolution {
            if let Some((active_name, active_type)) = self.ctx.active_name_and_type() {
                let ty = local.ty(self.ctx.db());
                if let Some(score) =
                    compute_score_from_active(&active_type, &active_name, &ty, &local_name)
                {
                    item = item.set_score(score);
                }
                ref_match = refed_type_matches(&active_type, &active_name, &ty, &local_name);
            }
        }

        // Add `<>` for generic types
        if self.ctx.completion.is_path_type
            && !self.ctx.completion.has_type_args
            && self.ctx.completion.config.add_call_parenthesis
        {
            if let Some(cap) = self.ctx.snippet_cap() {
                let has_non_default_type_params = match resolution {
                    ScopeDef::ModuleDef(Adt(it)) => it.has_non_default_type_params(self.ctx.db()),
                    ScopeDef::ModuleDef(TypeAlias(it)) => {
                        it.has_non_default_type_params(self.ctx.db())
                    }
                    _ => false,
                };
                if has_non_default_type_params {
                    mark::hit!(inserts_angle_brackets_for_generics);
                    item = item
                        .lookup_by(local_name.clone())
                        .label(format!("{}<…>", local_name))
                        .insert_snippet(cap, format!("{}<$0>", local_name));
                }
            }
        }

        let item = item.kind(kind).set_documentation(docs).set_ref_match(ref_match).build();
        Some(item)
    }

    fn docs(&self, resolution: &ScopeDef) -> Option<Documentation> {
        use hir::ModuleDef::*;
        match resolution {
            ScopeDef::ModuleDef(Module(it)) => it.docs(self.ctx.db()),
            ScopeDef::ModuleDef(Adt(it)) => it.docs(self.ctx.db()),
            ScopeDef::ModuleDef(EnumVariant(it)) => it.docs(self.ctx.db()),
            ScopeDef::ModuleDef(Const(it)) => it.docs(self.ctx.db()),
            ScopeDef::ModuleDef(Static(it)) => it.docs(self.ctx.db()),
            ScopeDef::ModuleDef(Trait(it)) => it.docs(self.ctx.db()),
            ScopeDef::ModuleDef(TypeAlias(it)) => it.docs(self.ctx.db()),
            _ => None,
        }
    }
}

fn compute_score_from_active(
    active_type: &Type,
    active_name: &str,
    ty: &Type,
    name: &str,
) -> Option<CompletionScore> {
    // Compute score
    // For the same type
    if active_type != ty {
        return None;
    }

    let mut res = CompletionScore::TypeMatch;

    // If same type + same name then go top position
    if active_name == name {
        res = CompletionScore::TypeAndNameMatch
    }

    Some(res)
}
fn refed_type_matches(
    active_type: &Type,
    active_name: &str,
    ty: &Type,
    name: &str,
) -> Option<(Mutability, CompletionScore)> {
    let derefed_active = active_type.remove_ref()?;
    let score = compute_score_from_active(&derefed_active, &active_name, &ty, &name)?;
    Some((
        if active_type.is_mutable_reference() { Mutability::Mut } else { Mutability::Shared },
        score,
    ))
}

fn compute_score(ctx: &RenderContext, ty: &Type, name: &str) -> Option<CompletionScore> {
    let (active_name, active_type) = ctx.active_name_and_type()?;
    compute_score_from_active(&active_type, &active_name, ty, name)
}

#[cfg(test)]
mod tests {
    use std::cmp::Reverse;

    use expect_test::{expect, Expect};
    use test_utils::mark;

    use crate::{
        test_utils::{check_edit, do_completion, get_all_items},
        CompletionConfig, CompletionKind, CompletionScore,
    };

    fn check(ra_fixture: &str, expect: Expect) {
        let actual = do_completion(ra_fixture, CompletionKind::Reference);
        expect.assert_debug_eq(&actual);
    }

    fn check_scores(ra_fixture: &str, expect: Expect) {
        fn display_score(score: Option<CompletionScore>) -> &'static str {
            match score {
                Some(CompletionScore::TypeMatch) => "[type]",
                Some(CompletionScore::TypeAndNameMatch) => "[type+name]",
                None => "[]".into(),
            }
        }

        let mut completions = get_all_items(CompletionConfig::default(), ra_fixture);
        completions.sort_by_key(|it| (Reverse(it.score()), it.label().to_string()));
        let actual = completions
            .into_iter()
            .filter(|it| it.completion_kind == CompletionKind::Reference)
            .map(|it| {
                let tag = it.kind().unwrap().tag();
                let score = display_score(it.score());
                format!("{} {} {}\n", tag, it.label(), score)
            })
            .collect::<String>();
        expect.assert_eq(&actual);
    }

    #[test]
    fn enum_detail_includes_record_fields() {
        check(
            r#"
enum Foo { Foo { x: i32, y: i32 } }

fn main() { Foo::Fo<|> }
"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "Foo",
                        source_range: 54..56,
                        delete: 54..56,
                        insert: "Foo",
                        kind: EnumVariant,
                        detail: "{ x: i32, y: i32 }",
                    },
                ]
            "#]],
        );
    }

    #[test]
    fn enum_detail_doesnt_include_tuple_fields() {
        check(
            r#"
enum Foo { Foo (i32, i32) }

fn main() { Foo::Fo<|> }
"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "Foo(…)",
                        source_range: 46..48,
                        delete: 46..48,
                        insert: "Foo($0)",
                        kind: EnumVariant,
                        lookup: "Foo",
                        detail: "(i32, i32)",
                        trigger_call_info: true,
                    },
                ]
            "#]],
        );
    }

    #[test]
    fn enum_detail_just_parentheses_for_unit() {
        check(
            r#"
enum Foo { Foo }

fn main() { Foo::Fo<|> }
"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "Foo",
                        source_range: 35..37,
                        delete: 35..37,
                        insert: "Foo",
                        kind: EnumVariant,
                        detail: "()",
                    },
                ]
            "#]],
        );
    }

    #[test]
    fn lookup_enums_by_two_qualifiers() {
        check(
            r#"
mod m {
    pub enum Spam { Foo, Bar(i32) }
}
fn main() { let _: m::Spam = S<|> }
"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "Spam::Bar(…)",
                        source_range: 75..76,
                        delete: 75..76,
                        insert: "Spam::Bar($0)",
                        kind: EnumVariant,
                        lookup: "Spam::Bar",
                        detail: "(i32)",
                        trigger_call_info: true,
                    },
                    CompletionItem {
                        label: "m",
                        source_range: 75..76,
                        delete: 75..76,
                        insert: "m",
                        kind: Module,
                    },
                    CompletionItem {
                        label: "m::Spam::Foo",
                        source_range: 75..76,
                        delete: 75..76,
                        insert: "m::Spam::Foo",
                        kind: EnumVariant,
                        lookup: "Spam::Foo",
                        detail: "()",
                    },
                    CompletionItem {
                        label: "main()",
                        source_range: 75..76,
                        delete: 75..76,
                        insert: "main()$0",
                        kind: Function,
                        lookup: "main",
                        detail: "fn main()",
                    },
                ]
            "#]],
        )
    }

    #[test]
    fn sets_deprecated_flag_in_items() {
        check(
            r#"
#[deprecated]
fn something_deprecated() {}
#[deprecated(since = "1.0.0")]
fn something_else_deprecated() {}

fn main() { som<|> }
"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "main()",
                        source_range: 121..124,
                        delete: 121..124,
                        insert: "main()$0",
                        kind: Function,
                        lookup: "main",
                        detail: "fn main()",
                    },
                    CompletionItem {
                        label: "something_deprecated()",
                        source_range: 121..124,
                        delete: 121..124,
                        insert: "something_deprecated()$0",
                        kind: Function,
                        lookup: "something_deprecated",
                        detail: "fn something_deprecated()",
                        deprecated: true,
                    },
                    CompletionItem {
                        label: "something_else_deprecated()",
                        source_range: 121..124,
                        delete: 121..124,
                        insert: "something_else_deprecated()$0",
                        kind: Function,
                        lookup: "something_else_deprecated",
                        detail: "fn something_else_deprecated()",
                        deprecated: true,
                    },
                ]
            "#]],
        );

        check(
            r#"
struct A { #[deprecated] the_field: u32 }
fn foo() { A { the<|> } }
"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "the_field",
                        source_range: 57..60,
                        delete: 57..60,
                        insert: "the_field",
                        kind: Field,
                        detail: "u32",
                        deprecated: true,
                    },
                ]
            "#]],
        );
    }

    #[test]
    fn renders_docs() {
        check(
            r#"
struct S {
    /// Field docs
    foo:
}
impl S {
    /// Method docs
    fn bar(self) { self.<|> }
}"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "bar()",
                        source_range: 94..94,
                        delete: 94..94,
                        insert: "bar()$0",
                        kind: Method,
                        lookup: "bar",
                        detail: "fn bar(self)",
                        documentation: Documentation(
                            "Method docs",
                        ),
                    },
                    CompletionItem {
                        label: "foo",
                        source_range: 94..94,
                        delete: 94..94,
                        insert: "foo",
                        kind: Field,
                        detail: "{unknown}",
                        documentation: Documentation(
                            "Field docs",
                        ),
                    },
                ]
            "#]],
        );

        check(
            r#"
use self::my<|>;

/// mod docs
mod my { }

/// enum docs
enum E {
    /// variant docs
    V
}
use self::E::*;
"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "E",
                        source_range: 10..12,
                        delete: 10..12,
                        insert: "E",
                        kind: Enum,
                        documentation: Documentation(
                            "enum docs",
                        ),
                    },
                    CompletionItem {
                        label: "V",
                        source_range: 10..12,
                        delete: 10..12,
                        insert: "V",
                        kind: EnumVariant,
                        detail: "()",
                        documentation: Documentation(
                            "variant docs",
                        ),
                    },
                    CompletionItem {
                        label: "my",
                        source_range: 10..12,
                        delete: 10..12,
                        insert: "my",
                        kind: Module,
                        documentation: Documentation(
                            "mod docs",
                        ),
                    },
                ]
            "#]],
        )
    }

    #[test]
    fn dont_render_attrs() {
        check(
            r#"
struct S;
impl S {
    #[inline]
    fn the_method(&self) { }
}
fn foo(s: S) { s.<|> }
"#,
            expect![[r#"
                [
                    CompletionItem {
                        label: "the_method()",
                        source_range: 81..81,
                        delete: 81..81,
                        insert: "the_method()$0",
                        kind: Method,
                        lookup: "the_method",
                        detail: "fn the_method(&self)",
                    },
                ]
            "#]],
        )
    }

    #[test]
    fn no_call_parens_if_fn_ptr_needed() {
        mark::check!(no_call_parens_if_fn_ptr_needed);
        check_edit(
            "foo",
            r#"
fn foo(foo: u8, bar: u8) {}
struct ManualVtable { f: fn(u8, u8) }

fn main() -> ManualVtable {
    ManualVtable { f: f<|> }
}
"#,
            r#"
fn foo(foo: u8, bar: u8) {}
struct ManualVtable { f: fn(u8, u8) }

fn main() -> ManualVtable {
    ManualVtable { f: foo }
}
"#,
        );
    }

    #[test]
    fn no_parens_in_use_item() {
        mark::check!(no_parens_in_use_item);
        check_edit(
            "foo",
            r#"
mod m { pub fn foo() {} }
use crate::m::f<|>;
"#,
            r#"
mod m { pub fn foo() {} }
use crate::m::foo;
"#,
        );
    }

    #[test]
    fn no_parens_in_call() {
        check_edit(
            "foo",
            r#"
fn foo(x: i32) {}
fn main() { f<|>(); }
"#,
            r#"
fn foo(x: i32) {}
fn main() { foo(); }
"#,
        );
        check_edit(
            "foo",
            r#"
struct Foo;
impl Foo { fn foo(&self){} }
fn f(foo: &Foo) { foo.f<|>(); }
"#,
            r#"
struct Foo;
impl Foo { fn foo(&self){} }
fn f(foo: &Foo) { foo.foo(); }
"#,
        );
    }

    #[test]
    fn inserts_angle_brackets_for_generics() {
        mark::check!(inserts_angle_brackets_for_generics);
        check_edit(
            "Vec",
            r#"
struct Vec<T> {}
fn foo(xs: Ve<|>)
"#,
            r#"
struct Vec<T> {}
fn foo(xs: Vec<$0>)
"#,
        );
        check_edit(
            "Vec",
            r#"
type Vec<T> = (T,);
fn foo(xs: Ve<|>)
"#,
            r#"
type Vec<T> = (T,);
fn foo(xs: Vec<$0>)
"#,
        );
        check_edit(
            "Vec",
            r#"
struct Vec<T = i128> {}
fn foo(xs: Ve<|>)
"#,
            r#"
struct Vec<T = i128> {}
fn foo(xs: Vec)
"#,
        );
        check_edit(
            "Vec",
            r#"
struct Vec<T> {}
fn foo(xs: Ve<|><i128>)
"#,
            r#"
struct Vec<T> {}
fn foo(xs: Vec<i128>)
"#,
        );
    }

    #[test]
    fn active_param_score() {
        mark::check!(active_param_type_match);
        check_scores(
            r#"
struct S { foo: i64, bar: u32, baz: u32 }
fn test(bar: u32) { }
fn foo(s: S) { test(s.<|>) }
"#,
            expect![[r#"
                fd bar [type+name]
                fd baz [type]
                fd foo []
            "#]],
        );
    }

    #[test]
    fn record_field_scores() {
        mark::check!(record_field_type_match);
        check_scores(
            r#"
struct A { foo: i64, bar: u32, baz: u32 }
struct B { x: (), y: f32, bar: u32 }
fn foo(a: A) { B { bar: a.<|> }; }
"#,
            expect![[r#"
                fd bar [type+name]
                fd baz [type]
                fd foo []
            "#]],
        )
    }

    #[test]
    fn record_field_and_call_scores() {
        check_scores(
            r#"
struct A { foo: i64, bar: u32, baz: u32 }
struct B { x: (), y: f32, bar: u32 }
fn f(foo: i64) {  }
fn foo(a: A) { B { bar: f(a.<|>) }; }
"#,
            expect![[r#"
                fd foo [type+name]
                fd bar []
                fd baz []
            "#]],
        );
        check_scores(
            r#"
struct A { foo: i64, bar: u32, baz: u32 }
struct B { x: (), y: f32, bar: u32 }
fn f(foo: i64) {  }
fn foo(a: A) { f(B { bar: a.<|> }); }
"#,
            expect![[r#"
                fd bar [type+name]
                fd baz [type]
                fd foo []
            "#]],
        );
    }

    #[test]
    fn prioritize_exact_ref_match() {
        check_scores(
            r#"
struct WorldSnapshot { _f: () };
fn go(world: &WorldSnapshot) { go(w<|>) }
"#,
            expect![[r#"
                bn world [type+name]
                st WorldSnapshot []
                fn go(…) []
            "#]],
        );
    }

    #[test]
    fn too_many_arguments() {
        check_scores(
            r#"
struct Foo;
fn f(foo: &Foo) { f(foo, w<|>) }
"#,
            expect![[r#"
                st Foo []
                fn f(…) []
                bn foo []
            "#]],
        );
    }
}
