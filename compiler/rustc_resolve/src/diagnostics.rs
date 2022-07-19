use std::ptr;

use rustc_ast::ptr::P;
use rustc_ast::visit::{self, Visitor};
use rustc_ast::{self as ast, Crate, ItemKind, ModKind, NodeId, Path, CRATE_NODE_ID};
use rustc_ast_pretty::pprust;
use rustc_data_structures::fx::FxHashSet;
use rustc_errors::struct_span_err;
use rustc_errors::{Applicability, Diagnostic, DiagnosticBuilder, ErrorGuaranteed, MultiSpan};
use rustc_feature::BUILTIN_ATTRIBUTES;
use rustc_hir::def::Namespace::{self, *};
use rustc_hir::def::{self, CtorKind, CtorOf, DefKind, NonMacroAttrKind, PerNS};
use rustc_hir::def_id::{DefId, LocalDefId, CRATE_DEF_ID, LOCAL_CRATE};
use rustc_hir::PrimTy;
use rustc_index::vec::IndexVec;
use rustc_middle::bug;
use rustc_middle::ty::DefIdTree;
use rustc_session::lint::builtin::ABSOLUTE_PATHS_NOT_STARTING_WITH_CRATE;
use rustc_session::lint::builtin::MACRO_EXPANDED_MACRO_EXPORTS_ACCESSED_BY_ABSOLUTE_PATHS;
use rustc_session::lint::BuiltinLintDiagnostics;
use rustc_session::Session;
use rustc_span::edition::Edition;
use rustc_span::hygiene::MacroKind;
use rustc_span::lev_distance::find_best_match_for_name;
use rustc_span::source_map::SourceMap;
use rustc_span::symbol::{kw, sym, Ident, Symbol};
use rustc_span::{BytePos, Span};
use tracing::debug;

use crate::imports::{Import, ImportKind, ImportResolver};
use crate::late::{PatternSource, Rib};
use crate::path_names_to_string;
use crate::{AmbiguityError, AmbiguityErrorMisc, AmbiguityKind, BindingError, Finalize};
use crate::{HasGenericParams, MacroRulesScope, Module, ModuleKind, ModuleOrUniformRoot};
use crate::{LexicalScopeBinding, NameBinding, NameBindingKind, PrivacyError, VisResolutionError};
use crate::{ParentScope, PathResult, ResolutionError, Resolver, Scope, ScopeSet};
use crate::{Segment, UseError};

#[cfg(test)]
mod tests;

type Res = def::Res<ast::NodeId>;

/// A vector of spans and replacements, a message and applicability.
pub(crate) type Suggestion = (Vec<(Span, String)>, String, Applicability);

/// Potential candidate for an undeclared or out-of-scope label - contains the ident of a
/// similarly named label and whether or not it is reachable.
pub(crate) type LabelSuggestion = (Ident, bool);

pub(crate) enum SuggestionTarget {
    /// The target has a similar name as the name used by the programmer (probably a typo)
    SimilarlyNamed,
    /// The target is the only valid item that can be used in the corresponding context
    SingleItem,
}

pub(crate) struct TypoSuggestion {
    pub candidate: Symbol,
    pub res: Res,
    pub target: SuggestionTarget,
}

impl TypoSuggestion {
    pub(crate) fn typo_from_res(candidate: Symbol, res: Res) -> TypoSuggestion {
        Self { candidate, res, target: SuggestionTarget::SimilarlyNamed }
    }
    pub(crate) fn single_item_from_res(candidate: Symbol, res: Res) -> TypoSuggestion {
        Self { candidate, res, target: SuggestionTarget::SingleItem }
    }
}

/// A free importable items suggested in case of resolution failure.
pub(crate) struct ImportSuggestion {
    pub did: Option<DefId>,
    pub descr: &'static str,
    pub path: Path,
    pub accessible: bool,
    /// An extra note that should be issued if this item is suggested
    pub note: Option<String>,
}

/// Adjust the impl span so that just the `impl` keyword is taken by removing
/// everything after `<` (`"impl<T> Iterator for A<T> {}" -> "impl"`) and
/// everything after the first whitespace (`"impl Iterator for A" -> "impl"`).
///
/// *Attention*: the method used is very fragile since it essentially duplicates the work of the
/// parser. If you need to use this function or something similar, please consider updating the
/// `source_map` functions and this function to something more robust.
fn reduce_impl_span_to_impl_keyword(sm: &SourceMap, impl_span: Span) -> Span {
    let impl_span = sm.span_until_char(impl_span, '<');
    sm.span_until_whitespace(impl_span)
}

impl<'a> Resolver<'a> {
    pub(crate) fn report_errors(&mut self, krate: &Crate) {
        self.report_with_use_injections(krate);

        for &(span_use, span_def) in &self.macro_expanded_macro_export_errors {
            let msg = "macro-expanded `macro_export` macros from the current crate \
                       cannot be referred to by absolute paths";
            self.lint_buffer.buffer_lint_with_diagnostic(
                MACRO_EXPANDED_MACRO_EXPORTS_ACCESSED_BY_ABSOLUTE_PATHS,
                CRATE_NODE_ID,
                span_use,
                msg,
                BuiltinLintDiagnostics::MacroExpandedMacroExportsAccessedByAbsolutePaths(span_def),
            );
        }

        for ambiguity_error in &self.ambiguity_errors {
            self.report_ambiguity_error(ambiguity_error);
        }

        let mut reported_spans = FxHashSet::default();
        for error in &self.privacy_errors {
            if reported_spans.insert(error.dedup_span) {
                self.report_privacy_error(error);
            }
        }
    }

    fn report_with_use_injections(&mut self, krate: &Crate) {
        for UseError { mut err, candidates, def_id, instead, suggestion, path } in
            self.use_injections.drain(..)
        {
            let (span, found_use) = if let Some(def_id) = def_id.as_local() {
                UsePlacementFinder::check(krate, self.def_id_to_node_id[def_id])
            } else {
                (None, FoundUse::No)
            };
            if !candidates.is_empty() {
                show_candidates(
                    &self.session,
                    &self.source_span,
                    &mut err,
                    span,
                    &candidates,
                    if instead { Instead::Yes } else { Instead::No },
                    found_use,
                    IsPattern::No,
                    path,
                );
            } else if let Some((span, msg, sugg, appl)) = suggestion {
                err.span_suggestion(span, msg, sugg, appl);
            }
            err.emit();
        }
    }

    pub(crate) fn report_conflict<'b>(
        &mut self,
        parent: Module<'_>,
        ident: Ident,
        ns: Namespace,
        new_binding: &NameBinding<'b>,
        old_binding: &NameBinding<'b>,
    ) {
        // Error on the second of two conflicting names
        if old_binding.span.lo() > new_binding.span.lo() {
            return self.report_conflict(parent, ident, ns, old_binding, new_binding);
        }

        let container = match parent.kind {
            ModuleKind::Def(kind, _, _) => kind.descr(parent.def_id()),
            ModuleKind::Block(..) => "block",
        };

        let old_noun = match old_binding.is_import() {
            true => "import",
            false => "definition",
        };

        let new_participle = match new_binding.is_import() {
            true => "imported",
            false => "defined",
        };

        let (name, span) =
            (ident.name, self.session.source_map().guess_head_span(new_binding.span));

        if let Some(s) = self.name_already_seen.get(&name) {
            if s == &span {
                return;
            }
        }

        let old_kind = match (ns, old_binding.module()) {
            (ValueNS, _) => "value",
            (MacroNS, _) => "macro",
            (TypeNS, _) if old_binding.is_extern_crate() => "extern crate",
            (TypeNS, Some(module)) if module.is_normal() => "module",
            (TypeNS, Some(module)) if module.is_trait() => "trait",
            (TypeNS, _) => "type",
        };

        let msg = format!("the name `{}` is defined multiple times", name);

        let mut err = match (old_binding.is_extern_crate(), new_binding.is_extern_crate()) {
            (true, true) => struct_span_err!(self.session, span, E0259, "{}", msg),
            (true, _) | (_, true) => match new_binding.is_import() && old_binding.is_import() {
                true => struct_span_err!(self.session, span, E0254, "{}", msg),
                false => struct_span_err!(self.session, span, E0260, "{}", msg),
            },
            _ => match (old_binding.is_import(), new_binding.is_import()) {
                (false, false) => struct_span_err!(self.session, span, E0428, "{}", msg),
                (true, true) => struct_span_err!(self.session, span, E0252, "{}", msg),
                _ => struct_span_err!(self.session, span, E0255, "{}", msg),
            },
        };

        err.note(&format!(
            "`{}` must be defined only once in the {} namespace of this {}",
            name,
            ns.descr(),
            container
        ));

        err.span_label(span, format!("`{}` re{} here", name, new_participle));
        err.span_label(
            self.session.source_map().guess_head_span(old_binding.span),
            format!("previous {} of the {} `{}` here", old_noun, old_kind, name),
        );

        // See https://github.com/rust-lang/rust/issues/32354
        use NameBindingKind::Import;
        let import = match (&new_binding.kind, &old_binding.kind) {
            // If there are two imports where one or both have attributes then prefer removing the
            // import without attributes.
            (Import { import: new, .. }, Import { import: old, .. })
                if {
                    !new_binding.span.is_dummy()
                        && !old_binding.span.is_dummy()
                        && (new.has_attributes || old.has_attributes)
                } =>
            {
                if old.has_attributes {
                    Some((new, new_binding.span, true))
                } else {
                    Some((old, old_binding.span, true))
                }
            }
            // Otherwise prioritize the new binding.
            (Import { import, .. }, other) if !new_binding.span.is_dummy() => {
                Some((import, new_binding.span, other.is_import()))
            }
            (other, Import { import, .. }) if !old_binding.span.is_dummy() => {
                Some((import, old_binding.span, other.is_import()))
            }
            _ => None,
        };

        // Check if the target of the use for both bindings is the same.
        let duplicate = new_binding.res().opt_def_id() == old_binding.res().opt_def_id();
        let has_dummy_span = new_binding.span.is_dummy() || old_binding.span.is_dummy();
        let from_item =
            self.extern_prelude.get(&ident).map_or(true, |entry| entry.introduced_by_item);
        // Only suggest removing an import if both bindings are to the same def, if both spans
        // aren't dummy spans. Further, if both bindings are imports, then the ident must have
        // been introduced by an item.
        let should_remove_import = duplicate
            && !has_dummy_span
            && ((new_binding.is_extern_crate() || old_binding.is_extern_crate()) || from_item);

        match import {
            Some((import, span, true)) if should_remove_import && import.is_nested() => {
                self.add_suggestion_for_duplicate_nested_use(&mut err, import, span)
            }
            Some((import, _, true)) if should_remove_import && !import.is_glob() => {
                // Simple case - remove the entire import. Due to the above match arm, this can
                // only be a single use so just remove it entirely.
                err.tool_only_span_suggestion(
                    import.use_span_with_attributes,
                    "remove unnecessary import",
                    "",
                    Applicability::MaybeIncorrect,
                );
            }
            Some((import, span, _)) => {
                self.add_suggestion_for_rename_of_use(&mut err, name, import, span)
            }
            _ => {}
        }

        err.emit();
        self.name_already_seen.insert(name, span);
    }

    /// This function adds a suggestion to change the binding name of a new import that conflicts
    /// with an existing import.
    ///
    /// ```text,ignore (diagnostic)
    /// help: you can use `as` to change the binding name of the import
    ///    |
    /// LL | use foo::bar as other_bar;
    ///    |     ^^^^^^^^^^^^^^^^^^^^^
    /// ```
    fn add_suggestion_for_rename_of_use(
        &self,
        err: &mut Diagnostic,
        name: Symbol,
        import: &Import<'_>,
        binding_span: Span,
    ) {
        let suggested_name = if name.as_str().chars().next().unwrap().is_uppercase() {
            format!("Other{}", name)
        } else {
            format!("other_{}", name)
        };

        let mut suggestion = None;
        match import.kind {
            ImportKind::Single { type_ns_only: true, .. } => {
                suggestion = Some(format!("self as {}", suggested_name))
            }
            ImportKind::Single { source, .. } => {
                if let Some(pos) =
                    source.span.hi().0.checked_sub(binding_span.lo().0).map(|pos| pos as usize)
                {
                    if let Ok(snippet) = self.session.source_map().span_to_snippet(binding_span) {
                        if pos <= snippet.len() {
                            suggestion = Some(format!(
                                "{} as {}{}",
                                &snippet[..pos],
                                suggested_name,
                                if snippet.ends_with(';') { ";" } else { "" }
                            ))
                        }
                    }
                }
            }
            ImportKind::ExternCrate { source, target } => {
                suggestion = Some(format!(
                    "extern crate {} as {};",
                    source.unwrap_or(target.name),
                    suggested_name,
                ))
            }
            _ => unreachable!(),
        }

        let rename_msg = "you can use `as` to change the binding name of the import";
        if let Some(suggestion) = suggestion {
            err.span_suggestion(
                binding_span,
                rename_msg,
                suggestion,
                Applicability::MaybeIncorrect,
            );
        } else {
            err.span_label(binding_span, rename_msg);
        }
    }

    /// This function adds a suggestion to remove an unnecessary binding from an import that is
    /// nested. In the following example, this function will be invoked to remove the `a` binding
    /// in the second use statement:
    ///
    /// ```ignore (diagnostic)
    /// use issue_52891::a;
    /// use issue_52891::{d, a, e};
    /// ```
    ///
    /// The following suggestion will be added:
    ///
    /// ```ignore (diagnostic)
    /// use issue_52891::{d, a, e};
    ///                      ^-- help: remove unnecessary import
    /// ```
    ///
    /// If the nested use contains only one import then the suggestion will remove the entire
    /// line.
    ///
    /// It is expected that the provided import is nested - this isn't checked by the
    /// function. If this invariant is not upheld, this function's behaviour will be unexpected
    /// as characters expected by span manipulations won't be present.
    fn add_suggestion_for_duplicate_nested_use(
        &self,
        err: &mut Diagnostic,
        import: &Import<'_>,
        binding_span: Span,
    ) {
        assert!(import.is_nested());
        let message = "remove unnecessary import";

        // Two examples will be used to illustrate the span manipulations we're doing:
        //
        // - Given `use issue_52891::{d, a, e};` where `a` is a duplicate then `binding_span` is
        //   `a` and `import.use_span` is `issue_52891::{d, a, e};`.
        // - Given `use issue_52891::{d, e, a};` where `a` is a duplicate then `binding_span` is
        //   `a` and `import.use_span` is `issue_52891::{d, e, a};`.

        let (found_closing_brace, span) =
            find_span_of_binding_until_next_binding(self.session, binding_span, import.use_span);

        // If there was a closing brace then identify the span to remove any trailing commas from
        // previous imports.
        if found_closing_brace {
            if let Some(span) = extend_span_to_previous_binding(self.session, span) {
                err.tool_only_span_suggestion(span, message, "", Applicability::MaybeIncorrect);
            } else {
                // Remove the entire line if we cannot extend the span back, this indicates an
                // `issue_52891::{self}` case.
                err.span_suggestion(
                    import.use_span_with_attributes,
                    message,
                    "",
                    Applicability::MaybeIncorrect,
                );
            }

            return;
        }

        err.span_suggestion(span, message, "", Applicability::MachineApplicable);
    }

    pub(crate) fn lint_if_path_starts_with_module(
        &mut self,
        finalize: Option<Finalize>,
        path: &[Segment],
        second_binding: Option<&NameBinding<'_>>,
    ) {
        let Some(Finalize { node_id, root_span, .. }) = finalize else {
            return;
        };

        let first_name = match path.get(0) {
            // In the 2018 edition this lint is a hard error, so nothing to do
            Some(seg) if seg.ident.span.rust_2015() && self.session.rust_2015() => seg.ident.name,
            _ => return,
        };

        // We're only interested in `use` paths which should start with
        // `{{root}}` currently.
        if first_name != kw::PathRoot {
            return;
        }

        match path.get(1) {
            // If this import looks like `crate::...` it's already good
            Some(Segment { ident, .. }) if ident.name == kw::Crate => return,
            // Otherwise go below to see if it's an extern crate
            Some(_) => {}
            // If the path has length one (and it's `PathRoot` most likely)
            // then we don't know whether we're gonna be importing a crate or an
            // item in our crate. Defer this lint to elsewhere
            None => return,
        }

        // If the first element of our path was actually resolved to an
        // `ExternCrate` (also used for `crate::...`) then no need to issue a
        // warning, this looks all good!
        if let Some(binding) = second_binding {
            if let NameBindingKind::Import { import, .. } = binding.kind {
                // Careful: we still want to rewrite paths from renamed extern crates.
                if let ImportKind::ExternCrate { source: None, .. } = import.kind {
                    return;
                }
            }
        }

        let diag = BuiltinLintDiagnostics::AbsPathWithModule(root_span);
        self.lint_buffer.buffer_lint_with_diagnostic(
            ABSOLUTE_PATHS_NOT_STARTING_WITH_CRATE,
            node_id,
            root_span,
            "absolute paths must start with `self`, `super`, \
             `crate`, or an external crate name in the 2018 edition",
            diag,
        );
    }

    pub(crate) fn add_module_candidates(
        &mut self,
        module: Module<'a>,
        names: &mut Vec<TypoSuggestion>,
        filter_fn: &impl Fn(Res) -> bool,
    ) {
        for (key, resolution) in self.resolutions(module).borrow().iter() {
            if let Some(binding) = resolution.borrow().binding {
                let res = binding.res();
                if filter_fn(res) {
                    names.push(TypoSuggestion::typo_from_res(key.ident.name, res));
                }
            }
        }
    }

    /// Combines an error with provided span and emits it.
    ///
    /// This takes the error provided, combines it with the span and any additional spans inside the
    /// error and emits it.
    pub(crate) fn report_error(&mut self, span: Span, resolution_error: ResolutionError<'a>) {
        self.into_struct_error(span, resolution_error).emit();
    }

    pub(crate) fn into_struct_error(
        &mut self,
        span: Span,
        resolution_error: ResolutionError<'a>,
    ) -> DiagnosticBuilder<'_, ErrorGuaranteed> {
        match resolution_error {
            ResolutionError::GenericParamsFromOuterFunction(outer_res, has_generic_params) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0401,
                    "can't use generic parameters from outer function",
                );
                err.span_label(span, "use of generic parameter from outer function");

                let sm = self.session.source_map();
                match outer_res {
                    Res::SelfTy { trait_: maybe_trait_defid, alias_to: maybe_impl_defid } => {
                        if let Some(impl_span) =
                            maybe_impl_defid.and_then(|(def_id, _)| self.opt_span(def_id))
                        {
                            err.span_label(
                                reduce_impl_span_to_impl_keyword(sm, impl_span),
                                "`Self` type implicitly declared here, by this `impl`",
                            );
                        }
                        match (maybe_trait_defid, maybe_impl_defid) {
                            (Some(_), None) => {
                                err.span_label(span, "can't use `Self` here");
                            }
                            (_, Some(_)) => {
                                err.span_label(span, "use a type here instead");
                            }
                            (None, None) => bug!("`impl` without trait nor type?"),
                        }
                        return err;
                    }
                    Res::Def(DefKind::TyParam, def_id) => {
                        if let Some(span) = self.opt_span(def_id) {
                            err.span_label(span, "type parameter from outer function");
                        }
                    }
                    Res::Def(DefKind::ConstParam, def_id) => {
                        if let Some(span) = self.opt_span(def_id) {
                            err.span_label(span, "const parameter from outer function");
                        }
                    }
                    _ => {
                        bug!(
                            "GenericParamsFromOuterFunction should only be used with Res::SelfTy, \
                            DefKind::TyParam or DefKind::ConstParam"
                        );
                    }
                }

                if has_generic_params == HasGenericParams::Yes {
                    // Try to retrieve the span of the function signature and generate a new
                    // message with a local type or const parameter.
                    let sugg_msg = "try using a local generic parameter instead";
                    if let Some((sugg_span, snippet)) = sm.generate_local_type_param_snippet(span) {
                        // Suggest the modification to the user
                        err.span_suggestion(
                            sugg_span,
                            sugg_msg,
                            snippet,
                            Applicability::MachineApplicable,
                        );
                    } else if let Some(sp) = sm.generate_fn_name_span(span) {
                        err.span_label(
                            sp,
                            "try adding a local generic parameter in this method instead"
                                .to_string(),
                        );
                    } else {
                        err.help("try using a local generic parameter instead");
                    }
                }

                err
            }
            ResolutionError::NameAlreadyUsedInParameterList(name, first_use_span) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0403,
                    "the name `{}` is already used for a generic \
                     parameter in this item's generic parameters",
                    name,
                );
                err.span_label(span, "already used");
                err.span_label(first_use_span, format!("first use of `{}`", name));
                err
            }
            ResolutionError::MethodNotMemberOfTrait(method, trait_, candidate) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0407,
                    "method `{}` is not a member of trait `{}`",
                    method,
                    trait_
                );
                err.span_label(span, format!("not a member of trait `{}`", trait_));
                if let Some(candidate) = candidate {
                    err.span_suggestion(
                        method.span,
                        "there is an associated function with a similar name",
                        candidate.to_ident_string(),
                        Applicability::MaybeIncorrect,
                    );
                }
                err
            }
            ResolutionError::TypeNotMemberOfTrait(type_, trait_, candidate) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0437,
                    "type `{}` is not a member of trait `{}`",
                    type_,
                    trait_
                );
                err.span_label(span, format!("not a member of trait `{}`", trait_));
                if let Some(candidate) = candidate {
                    err.span_suggestion(
                        type_.span,
                        "there is an associated type with a similar name",
                        candidate.to_ident_string(),
                        Applicability::MaybeIncorrect,
                    );
                }
                err
            }
            ResolutionError::ConstNotMemberOfTrait(const_, trait_, candidate) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0438,
                    "const `{}` is not a member of trait `{}`",
                    const_,
                    trait_
                );
                err.span_label(span, format!("not a member of trait `{}`", trait_));
                if let Some(candidate) = candidate {
                    err.span_suggestion(
                        const_.span,
                        "there is an associated constant with a similar name",
                        candidate.to_ident_string(),
                        Applicability::MaybeIncorrect,
                    );
                }
                err
            }
            ResolutionError::VariableNotBoundInPattern(binding_error, parent_scope) => {
                let BindingError { name, target, origin, could_be_path } = binding_error;

                let target_sp = target.iter().copied().collect::<Vec<_>>();
                let origin_sp = origin.iter().copied().collect::<Vec<_>>();

                let msp = MultiSpan::from_spans(target_sp.clone());
                let mut err = struct_span_err!(
                    self.session,
                    msp,
                    E0408,
                    "variable `{}` is not bound in all patterns",
                    name,
                );
                for sp in target_sp {
                    err.span_label(sp, format!("pattern doesn't bind `{}`", name));
                }
                for sp in origin_sp {
                    err.span_label(sp, "variable not in all patterns");
                }
                if could_be_path {
                    let import_suggestions = self.lookup_import_candidates(
                        Ident::with_dummy_span(name),
                        Namespace::ValueNS,
                        &parent_scope,
                        &|res: Res| match res {
                            Res::Def(
                                DefKind::Ctor(CtorOf::Variant, CtorKind::Const)
                                | DefKind::Ctor(CtorOf::Struct, CtorKind::Const)
                                | DefKind::Const
                                | DefKind::AssocConst,
                                _,
                            ) => true,
                            _ => false,
                        },
                    );

                    if import_suggestions.is_empty() {
                        let help_msg = format!(
                            "if you meant to match on a variant or a `const` item, consider \
                             making the path in the pattern qualified: `path::to::ModOrType::{}`",
                            name,
                        );
                        err.span_help(span, &help_msg);
                    }
                    show_candidates(
                        &self.session,
                        &self.source_span,
                        &mut err,
                        Some(span),
                        &import_suggestions,
                        Instead::No,
                        FoundUse::Yes,
                        IsPattern::Yes,
                        vec![],
                    );
                }
                err
            }
            ResolutionError::VariableBoundWithDifferentMode(variable_name, first_binding_span) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0409,
                    "variable `{}` is bound inconsistently across alternatives separated by `|`",
                    variable_name
                );
                err.span_label(span, "bound in different ways");
                err.span_label(first_binding_span, "first binding");
                err
            }
            ResolutionError::IdentifierBoundMoreThanOnceInParameterList(identifier) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0415,
                    "identifier `{}` is bound more than once in this parameter list",
                    identifier
                );
                err.span_label(span, "used as parameter more than once");
                err
            }
            ResolutionError::IdentifierBoundMoreThanOnceInSamePattern(identifier) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0416,
                    "identifier `{}` is bound more than once in the same pattern",
                    identifier
                );
                err.span_label(span, "used in a pattern more than once");
                err
            }
            ResolutionError::UndeclaredLabel { name, suggestion } => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0426,
                    "use of undeclared label `{}`",
                    name
                );

                err.span_label(span, format!("undeclared label `{}`", name));

                match suggestion {
                    // A reachable label with a similar name exists.
                    Some((ident, true)) => {
                        err.span_label(ident.span, "a label with a similar name is reachable");
                        err.span_suggestion(
                            span,
                            "try using similarly named label",
                            ident.name,
                            Applicability::MaybeIncorrect,
                        );
                    }
                    // An unreachable label with a similar name exists.
                    Some((ident, false)) => {
                        err.span_label(
                            ident.span,
                            "a label with a similar name exists but is unreachable",
                        );
                    }
                    // No similarly-named labels exist.
                    None => (),
                }

                err
            }
            ResolutionError::SelfImportsOnlyAllowedWithin { root, span_with_rename } => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0429,
                    "{}",
                    "`self` imports are only allowed within a { } list"
                );

                // None of the suggestions below would help with a case like `use self`.
                if !root {
                    // use foo::bar::self        -> foo::bar
                    // use foo::bar::self as abc -> foo::bar as abc
                    err.span_suggestion(
                        span,
                        "consider importing the module directly",
                        "",
                        Applicability::MachineApplicable,
                    );

                    // use foo::bar::self        -> foo::bar::{self}
                    // use foo::bar::self as abc -> foo::bar::{self as abc}
                    let braces = vec![
                        (span_with_rename.shrink_to_lo(), "{".to_string()),
                        (span_with_rename.shrink_to_hi(), "}".to_string()),
                    ];
                    err.multipart_suggestion(
                        "alternatively, use the multi-path `use` syntax to import `self`",
                        braces,
                        Applicability::MachineApplicable,
                    );
                }
                err
            }
            ResolutionError::SelfImportCanOnlyAppearOnceInTheList => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0430,
                    "`self` import can only appear once in an import list"
                );
                err.span_label(span, "can only appear once in an import list");
                err
            }
            ResolutionError::SelfImportOnlyInImportListWithNonEmptyPrefix => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0431,
                    "`self` import can only appear in an import list with \
                                                a non-empty prefix"
                );
                err.span_label(span, "can only appear in an import list with a non-empty prefix");
                err
            }
            ResolutionError::FailedToResolve { label, suggestion } => {
                let mut err =
                    struct_span_err!(self.session, span, E0433, "failed to resolve: {}", &label);
                err.span_label(span, label);

                if let Some((suggestions, msg, applicability)) = suggestion {
                    if suggestions.is_empty() {
                        err.help(&msg);
                        return err;
                    }
                    err.multipart_suggestion(&msg, suggestions, applicability);
                }

                err
            }
            ResolutionError::CannotCaptureDynamicEnvironmentInFnItem => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0434,
                    "{}",
                    "can't capture dynamic environment in a fn item"
                );
                err.help("use the `|| { ... }` closure form instead");
                err
            }
            ResolutionError::AttemptToUseNonConstantValueInConstant(ident, sugg, current) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0435,
                    "attempt to use a non-constant value in a constant"
                );
                // let foo =...
                //     ^^^ given this Span
                // ------- get this Span to have an applicable suggestion

                // edit:
                // only do this if the const and usage of the non-constant value are on the same line
                // the further the two are apart, the higher the chance of the suggestion being wrong

                let sp = self
                    .session
                    .source_map()
                    .span_extend_to_prev_str(ident.span, current, true, false);

                match sp {
                    Some(sp) if !self.session.source_map().is_multiline(sp) => {
                        let sp = sp.with_lo(BytePos(sp.lo().0 - (current.len() as u32)));
                        err.span_suggestion(
                            sp,
                            &format!("consider using `{}` instead of `{}`", sugg, current),
                            format!("{} {}", sugg, ident),
                            Applicability::MaybeIncorrect,
                        );
                        err.span_label(span, "non-constant value");
                    }
                    _ => {
                        err.span_label(ident.span, &format!("this would need to be a `{}`", sugg));
                    }
                }

                err
            }
            ResolutionError::BindingShadowsSomethingUnacceptable {
                shadowing_binding,
                name,
                participle,
                article,
                shadowed_binding,
                shadowed_binding_span,
            } => {
                let shadowed_binding_descr = shadowed_binding.descr();
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0530,
                    "{}s cannot shadow {}s",
                    shadowing_binding.descr(),
                    shadowed_binding_descr,
                );
                err.span_label(
                    span,
                    format!("cannot be named the same as {} {}", article, shadowed_binding_descr),
                );
                match (shadowing_binding, shadowed_binding) {
                    (
                        PatternSource::Match,
                        Res::Def(DefKind::Ctor(CtorOf::Variant | CtorOf::Struct, CtorKind::Fn), _),
                    ) => {
                        err.span_suggestion(
                            span,
                            "try specify the pattern arguments",
                            format!("{}(..)", name),
                            Applicability::Unspecified,
                        );
                    }
                    _ => (),
                }
                let msg =
                    format!("the {} `{}` is {} here", shadowed_binding_descr, name, participle);
                err.span_label(shadowed_binding_span, msg);
                err
            }
            ResolutionError::ForwardDeclaredGenericParam => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0128,
                    "generic parameters with a default cannot use \
                                                forward declared identifiers"
                );
                err.span_label(span, "defaulted generic parameters cannot be forward declared");
                err
            }
            ResolutionError::ParamInTyOfConstParam(name) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0770,
                    "the type of const parameters must not depend on other generic parameters"
                );
                err.span_label(
                    span,
                    format!("the type must not depend on the parameter `{}`", name),
                );
                err
            }
            ResolutionError::ParamInNonTrivialAnonConst { name, is_type } => {
                let mut err = self.session.struct_span_err(
                    span,
                    "generic parameters may not be used in const operations",
                );
                err.span_label(span, &format!("cannot perform const operation using `{}`", name));

                if is_type {
                    err.note("type parameters may not be used in const expressions");
                } else {
                    err.help(&format!(
                        "const parameters may only be used as standalone arguments, i.e. `{}`",
                        name
                    ));
                }

                if self.session.is_nightly_build() {
                    err.help(
                        "use `#![feature(generic_const_exprs)]` to allow generic const expressions",
                    );
                }

                err
            }
            ResolutionError::SelfInGenericParamDefault => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0735,
                    "generic parameters cannot use `Self` in their defaults"
                );
                err.span_label(span, "`Self` in generic parameter default");
                err
            }
            ResolutionError::UnreachableLabel { name, definition_span, suggestion } => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0767,
                    "use of unreachable label `{}`",
                    name,
                );

                err.span_label(definition_span, "unreachable label defined here");
                err.span_label(span, format!("unreachable label `{}`", name));
                err.note(
                    "labels are unreachable through functions, closures, async blocks and modules",
                );

                match suggestion {
                    // A reachable label with a similar name exists.
                    Some((ident, true)) => {
                        err.span_label(ident.span, "a label with a similar name is reachable");
                        err.span_suggestion(
                            span,
                            "try using similarly named label",
                            ident.name,
                            Applicability::MaybeIncorrect,
                        );
                    }
                    // An unreachable label with a similar name exists.
                    Some((ident, false)) => {
                        err.span_label(
                            ident.span,
                            "a label with a similar name exists but is also unreachable",
                        );
                    }
                    // No similarly-named labels exist.
                    None => (),
                }

                err
            }
            ResolutionError::TraitImplMismatch {
                name,
                kind,
                code,
                trait_item_span,
                trait_path,
            } => {
                let mut err = self.session.struct_span_err_with_code(
                    span,
                    &format!(
                        "item `{}` is an associated {}, which doesn't match its trait `{}`",
                        name, kind, trait_path,
                    ),
                    code,
                );
                err.span_label(span, "does not match trait");
                err.span_label(trait_item_span, "item in trait");
                err
            }
            ResolutionError::InvalidAsmSym => {
                let mut err = self.session.struct_span_err(span, "invalid `sym` operand");
                err.span_label(span, "is a local variable");
                err.help("`sym` operands must refer to either a function or a static");
                err
            }
        }
    }

    pub(crate) fn report_vis_error(
        &mut self,
        vis_resolution_error: VisResolutionError<'_>,
    ) -> ErrorGuaranteed {
        match vis_resolution_error {
            VisResolutionError::Relative2018(span, path) => {
                let mut err = self.session.struct_span_err(
                    span,
                    "relative paths are not supported in visibilities in 2018 edition or later",
                );
                err.span_suggestion(
                    path.span,
                    "try",
                    format!("crate::{}", pprust::path_to_string(&path)),
                    Applicability::MaybeIncorrect,
                );
                err
            }
            VisResolutionError::AncestorOnly(span) => struct_span_err!(
                self.session,
                span,
                E0742,
                "visibilities can only be restricted to ancestor modules"
            ),
            VisResolutionError::FailedToResolve(span, label, suggestion) => {
                self.into_struct_error(span, ResolutionError::FailedToResolve { label, suggestion })
            }
            VisResolutionError::ExpectedFound(span, path_str, res) => {
                let mut err = struct_span_err!(
                    self.session,
                    span,
                    E0577,
                    "expected module, found {} `{}`",
                    res.descr(),
                    path_str
                );
                err.span_label(span, "not a module");
                err
            }
            VisResolutionError::Indeterminate(span) => struct_span_err!(
                self.session,
                span,
                E0578,
                "cannot determine resolution for the visibility"
            ),
            VisResolutionError::ModuleOnly(span) => {
                self.session.struct_span_err(span, "visibility must resolve to a module")
            }
        }
        .emit()
    }

    /// Lookup typo candidate in scope for a macro or import.
    fn early_lookup_typo_candidate(
        &mut self,
        scope_set: ScopeSet<'a>,
        parent_scope: &ParentScope<'a>,
        ident: Ident,
        filter_fn: &impl Fn(Res) -> bool,
    ) -> Option<TypoSuggestion> {
        let mut suggestions = Vec::new();
        let ctxt = ident.span.ctxt();
        self.visit_scopes(scope_set, parent_scope, ctxt, |this, scope, use_prelude, _| {
            match scope {
                Scope::DeriveHelpers(expn_id) => {
                    let res = Res::NonMacroAttr(NonMacroAttrKind::DeriveHelper);
                    if filter_fn(res) {
                        suggestions.extend(
                            this.helper_attrs
                                .get(&expn_id)
                                .into_iter()
                                .flatten()
                                .map(|ident| TypoSuggestion::typo_from_res(ident.name, res)),
                        );
                    }
                }
                Scope::DeriveHelpersCompat => {
                    let res = Res::NonMacroAttr(NonMacroAttrKind::DeriveHelperCompat);
                    if filter_fn(res) {
                        for derive in parent_scope.derives {
                            let parent_scope = &ParentScope { derives: &[], ..*parent_scope };
                            if let Ok((Some(ext), _)) = this.resolve_macro_path(
                                derive,
                                Some(MacroKind::Derive),
                                parent_scope,
                                false,
                                false,
                            ) {
                                suggestions.extend(
                                    ext.helper_attrs
                                        .iter()
                                        .map(|name| TypoSuggestion::typo_from_res(*name, res)),
                                );
                            }
                        }
                    }
                }
                Scope::MacroRules(macro_rules_scope) => {
                    if let MacroRulesScope::Binding(macro_rules_binding) = macro_rules_scope.get() {
                        let res = macro_rules_binding.binding.res();
                        if filter_fn(res) {
                            suggestions.push(TypoSuggestion::typo_from_res(
                                macro_rules_binding.ident.name,
                                res,
                            ))
                        }
                    }
                }
                Scope::CrateRoot => {
                    let root_ident = Ident::new(kw::PathRoot, ident.span);
                    let root_module = this.resolve_crate_root(root_ident);
                    this.add_module_candidates(root_module, &mut suggestions, filter_fn);
                }
                Scope::Module(module, _) => {
                    this.add_module_candidates(module, &mut suggestions, filter_fn);
                }
                Scope::RegisteredAttrs => {
                    let res = Res::NonMacroAttr(NonMacroAttrKind::Registered);
                    if filter_fn(res) {
                        suggestions.extend(
                            this.registered_attrs
                                .iter()
                                .map(|ident| TypoSuggestion::typo_from_res(ident.name, res)),
                        );
                    }
                }
                Scope::MacroUsePrelude => {
                    suggestions.extend(this.macro_use_prelude.iter().filter_map(
                        |(name, binding)| {
                            let res = binding.res();
                            filter_fn(res).then_some(TypoSuggestion::typo_from_res(*name, res))
                        },
                    ));
                }
                Scope::BuiltinAttrs => {
                    let res = Res::NonMacroAttr(NonMacroAttrKind::Builtin(kw::Empty));
                    if filter_fn(res) {
                        suggestions.extend(
                            BUILTIN_ATTRIBUTES
                                .iter()
                                .map(|attr| TypoSuggestion::typo_from_res(attr.name, res)),
                        );
                    }
                }
                Scope::ExternPrelude => {
                    suggestions.extend(this.extern_prelude.iter().filter_map(|(ident, _)| {
                        let res = Res::Def(DefKind::Mod, CRATE_DEF_ID.to_def_id());
                        filter_fn(res).then_some(TypoSuggestion::typo_from_res(ident.name, res))
                    }));
                }
                Scope::ToolPrelude => {
                    let res = Res::NonMacroAttr(NonMacroAttrKind::Tool);
                    suggestions.extend(
                        this.registered_tools
                            .iter()
                            .map(|ident| TypoSuggestion::typo_from_res(ident.name, res)),
                    );
                }
                Scope::StdLibPrelude => {
                    if let Some(prelude) = this.prelude {
                        let mut tmp_suggestions = Vec::new();
                        this.add_module_candidates(prelude, &mut tmp_suggestions, filter_fn);
                        suggestions.extend(
                            tmp_suggestions
                                .into_iter()
                                .filter(|s| use_prelude || this.is_builtin_macro(s.res)),
                        );
                    }
                }
                Scope::BuiltinTypes => {
                    suggestions.extend(PrimTy::ALL.iter().filter_map(|prim_ty| {
                        let res = Res::PrimTy(*prim_ty);
                        filter_fn(res).then_some(TypoSuggestion::typo_from_res(prim_ty.name(), res))
                    }))
                }
            }

            None::<()>
        });

        // Make sure error reporting is deterministic.
        suggestions.sort_by(|a, b| a.candidate.as_str().partial_cmp(b.candidate.as_str()).unwrap());

        match find_best_match_for_name(
            &suggestions.iter().map(|suggestion| suggestion.candidate).collect::<Vec<Symbol>>(),
            ident.name,
            None,
        ) {
            Some(found) if found != ident.name => {
                suggestions.into_iter().find(|suggestion| suggestion.candidate == found)
            }
            _ => None,
        }
    }

    fn lookup_import_candidates_from_module<FilterFn>(
        &mut self,
        lookup_ident: Ident,
        namespace: Namespace,
        parent_scope: &ParentScope<'a>,
        start_module: Module<'a>,
        crate_name: Ident,
        filter_fn: FilterFn,
    ) -> Vec<ImportSuggestion>
    where
        FilterFn: Fn(Res) -> bool,
    {
        let mut candidates = Vec::new();
        let mut seen_modules = FxHashSet::default();
        let mut worklist = vec![(start_module, Vec::<ast::PathSegment>::new(), true)];
        let mut worklist_via_import = vec![];

        while let Some((in_module, path_segments, accessible)) = match worklist.pop() {
            None => worklist_via_import.pop(),
            Some(x) => Some(x),
        } {
            let in_module_is_extern = !in_module.def_id().is_local();
            // We have to visit module children in deterministic order to avoid
            // instabilities in reported imports (#43552).
            in_module.for_each_child(self, |this, ident, ns, name_binding| {
                // avoid non-importable candidates
                if !name_binding.is_importable() {
                    return;
                }

                let child_accessible =
                    accessible && this.is_accessible_from(name_binding.vis, parent_scope.module);

                // do not venture inside inaccessible items of other crates
                if in_module_is_extern && !child_accessible {
                    return;
                }

                let via_import = name_binding.is_import() && !name_binding.is_extern_crate();

                // There is an assumption elsewhere that paths of variants are in the enum's
                // declaration and not imported. With this assumption, the variant component is
                // chopped and the rest of the path is assumed to be the enum's own path. For
                // errors where a variant is used as the type instead of the enum, this causes
                // funny looking invalid suggestions, i.e `foo` instead of `foo::MyEnum`.
                if via_import && name_binding.is_possibly_imported_variant() {
                    return;
                }

                // #90113: Do not count an inaccessible reexported item as a candidate.
                if let NameBindingKind::Import { binding, .. } = name_binding.kind {
                    if this.is_accessible_from(binding.vis, parent_scope.module)
                        && !this.is_accessible_from(name_binding.vis, parent_scope.module)
                    {
                        return;
                    }
                }

                // collect results based on the filter function
                // avoid suggesting anything from the same module in which we are resolving
                // avoid suggesting anything with a hygienic name
                if ident.name == lookup_ident.name
                    && ns == namespace
                    && !ptr::eq(in_module, parent_scope.module)
                    && !ident.span.normalize_to_macros_2_0().from_expansion()
                {
                    let res = name_binding.res();
                    if filter_fn(res) {
                        // create the path
                        let mut segms = path_segments.clone();
                        if lookup_ident.span.rust_2018() {
                            // crate-local absolute paths start with `crate::` in edition 2018
                            // FIXME: may also be stabilized for Rust 2015 (Issues #45477, #44660)
                            segms.insert(0, ast::PathSegment::from_ident(crate_name));
                        }

                        segms.push(ast::PathSegment::from_ident(ident));
                        let path = Path { span: name_binding.span, segments: segms, tokens: None };
                        let did = match res {
                            Res::Def(DefKind::Ctor(..), did) => this.opt_parent(did),
                            _ => res.opt_def_id(),
                        };

                        if child_accessible {
                            // Remove invisible match if exists
                            if let Some(idx) = candidates
                                .iter()
                                .position(|v: &ImportSuggestion| v.did == did && !v.accessible)
                            {
                                candidates.remove(idx);
                            }
                        }

                        if candidates.iter().all(|v: &ImportSuggestion| v.did != did) {
                            // See if we're recommending TryFrom, TryInto, or FromIterator and add
                            // a note about editions
                            let note = if let Some(did) = did {
                                let requires_note = !did.is_local()
                                    && this.cstore().item_attrs_untracked(did, this.session).any(
                                        |attr| {
                                            if attr.has_name(sym::rustc_diagnostic_item) {
                                                [sym::TryInto, sym::TryFrom, sym::FromIterator]
                                                    .map(|x| Some(x))
                                                    .contains(&attr.value_str())
                                            } else {
                                                false
                                            }
                                        },
                                    );

                                requires_note.then(|| {
                                    format!(
                                        "'{}' is included in the prelude starting in Edition 2021",
                                        path_names_to_string(&path)
                                    )
                                })
                            } else {
                                None
                            };

                            candidates.push(ImportSuggestion {
                                did,
                                descr: res.descr(),
                                path,
                                accessible: child_accessible,
                                note,
                            });
                        }
                    }
                }

                // collect submodules to explore
                if let Some(module) = name_binding.module() {
                    // form the path
                    let mut path_segments = path_segments.clone();
                    path_segments.push(ast::PathSegment::from_ident(ident));

                    let is_extern_crate_that_also_appears_in_prelude =
                        name_binding.is_extern_crate() && lookup_ident.span.rust_2018();

                    if !is_extern_crate_that_also_appears_in_prelude {
                        // add the module to the lookup
                        if seen_modules.insert(module.def_id()) {
                            if via_import { &mut worklist_via_import } else { &mut worklist }
                                .push((module, path_segments, child_accessible));
                        }
                    }
                }
            })
        }

        // If only some candidates are accessible, take just them
        if !candidates.iter().all(|v: &ImportSuggestion| !v.accessible) {
            candidates = candidates.into_iter().filter(|x| x.accessible).collect();
        }

        candidates
    }

    /// When name resolution fails, this method can be used to look up candidate
    /// entities with the expected name. It allows filtering them using the
    /// supplied predicate (which should be used to only accept the types of
    /// definitions expected, e.g., traits). The lookup spans across all crates.
    ///
    /// N.B., the method does not look into imports, but this is not a problem,
    /// since we report the definitions (thus, the de-aliased imports).
    pub(crate) fn lookup_import_candidates<FilterFn>(
        &mut self,
        lookup_ident: Ident,
        namespace: Namespace,
        parent_scope: &ParentScope<'a>,
        filter_fn: FilterFn,
    ) -> Vec<ImportSuggestion>
    where
        FilterFn: Fn(Res) -> bool,
    {
        let mut suggestions = self.lookup_import_candidates_from_module(
            lookup_ident,
            namespace,
            parent_scope,
            self.graph_root,
            Ident::with_dummy_span(kw::Crate),
            &filter_fn,
        );

        if lookup_ident.span.rust_2018() {
            let extern_prelude_names = self.extern_prelude.clone();
            for (ident, _) in extern_prelude_names.into_iter() {
                if ident.span.from_expansion() {
                    // Idents are adjusted to the root context before being
                    // resolved in the extern prelude, so reporting this to the
                    // user is no help. This skips the injected
                    // `extern crate std` in the 2018 edition, which would
                    // otherwise cause duplicate suggestions.
                    continue;
                }
                if let Some(crate_id) = self.crate_loader.maybe_process_path_extern(ident.name) {
                    let crate_root = self.expect_module(crate_id.as_def_id());
                    suggestions.extend(self.lookup_import_candidates_from_module(
                        lookup_ident,
                        namespace,
                        parent_scope,
                        crate_root,
                        ident,
                        &filter_fn,
                    ));
                }
            }
        }

        suggestions
    }

    pub(crate) fn unresolved_macro_suggestions(
        &mut self,
        err: &mut Diagnostic,
        macro_kind: MacroKind,
        parent_scope: &ParentScope<'a>,
        ident: Ident,
    ) {
        let is_expected = &|res: Res| res.macro_kind() == Some(macro_kind);
        let suggestion = self.early_lookup_typo_candidate(
            ScopeSet::Macro(macro_kind),
            parent_scope,
            ident,
            is_expected,
        );
        self.add_typo_suggestion(err, suggestion, ident.span);

        let import_suggestions =
            self.lookup_import_candidates(ident, Namespace::MacroNS, parent_scope, is_expected);
        show_candidates(
            &self.session,
            &self.source_span,
            err,
            None,
            &import_suggestions,
            Instead::No,
            FoundUse::Yes,
            IsPattern::No,
            vec![],
        );

        if macro_kind == MacroKind::Derive && (ident.name == sym::Send || ident.name == sym::Sync) {
            let msg = format!("unsafe traits like `{}` should be implemented explicitly", ident);
            err.span_note(ident.span, &msg);
            return;
        }
        if self.macro_names.contains(&ident.normalize_to_macros_2_0()) {
            err.help("have you added the `#[macro_use]` on the module/import?");
            return;
        }
        if ident.name == kw::Default
            && let ModuleKind::Def(DefKind::Enum, def_id, _) = parent_scope.module.kind
            && let Some(span) = self.opt_span(def_id)
        {
            let source_map = self.session.source_map();
            let head_span = source_map.guess_head_span(span);
            if let Ok(head) = source_map.span_to_snippet(head_span) {
                err.span_suggestion(head_span, "consider adding a derive", format!("#[derive(Default)]\n{head}"), Applicability::MaybeIncorrect);
            } else {
                err.span_help(
                    head_span,
                    "consider adding `#[derive(Default)]` to this enum",
                );
            }
        }
        for ns in [Namespace::MacroNS, Namespace::TypeNS, Namespace::ValueNS] {
            if let Ok(binding) = self.early_resolve_ident_in_lexical_scope(
                ident,
                ScopeSet::All(ns, false),
                &parent_scope,
                None,
                false,
                None,
            ) {
                let desc = match binding.res() {
                    Res::Def(DefKind::Macro(MacroKind::Bang), _) => {
                        "a function-like macro".to_string()
                    }
                    Res::Def(DefKind::Macro(MacroKind::Attr), _) | Res::NonMacroAttr(..) => {
                        format!("an attribute: `#[{}]`", ident)
                    }
                    Res::Def(DefKind::Macro(MacroKind::Derive), _) => {
                        format!("a derive macro: `#[derive({})]`", ident)
                    }
                    Res::ToolMod => {
                        // Don't confuse the user with tool modules.
                        continue;
                    }
                    Res::Def(DefKind::Trait, _) if macro_kind == MacroKind::Derive => {
                        "only a trait, without a derive macro".to_string()
                    }
                    res => format!(
                        "{} {}, not {} {}",
                        res.article(),
                        res.descr(),
                        macro_kind.article(),
                        macro_kind.descr_expected(),
                    ),
                };
                if let crate::NameBindingKind::Import { import, .. } = binding.kind {
                    if !import.span.is_dummy() {
                        err.span_note(
                            import.span,
                            &format!("`{}` is imported here, but it is {}", ident, desc),
                        );
                        // Silence the 'unused import' warning we might get,
                        // since this diagnostic already covers that import.
                        self.record_use(ident, binding, false);
                        return;
                    }
                }
                err.note(&format!("`{}` is in scope, but it is {}", ident, desc));
                return;
            }
        }
    }

    pub(crate) fn add_typo_suggestion(
        &self,
        err: &mut Diagnostic,
        suggestion: Option<TypoSuggestion>,
        span: Span,
    ) -> bool {
        let suggestion = match suggestion {
            None => return false,
            // We shouldn't suggest underscore.
            Some(suggestion) if suggestion.candidate == kw::Underscore => return false,
            Some(suggestion) => suggestion,
        };
        let def_span = suggestion.res.opt_def_id().and_then(|def_id| match def_id.krate {
            LOCAL_CRATE => self.opt_span(def_id),
            _ => Some(
                self.session
                    .source_map()
                    .guess_head_span(self.cstore().get_span_untracked(def_id, self.session)),
            ),
        });
        if let Some(def_span) = def_span {
            if span.overlaps(def_span) {
                // Don't suggest typo suggestion for itself like in the following:
                // error[E0423]: expected function, tuple struct or tuple variant, found struct `X`
                //   --> $DIR/issue-64792-bad-unicode-ctor.rs:3:14
                //    |
                // LL | struct X {}
                //    | ----------- `X` defined here
                // LL |
                // LL | const Y: X = X("ö");
                //    | -------------^^^^^^- similarly named constant `Y` defined here
                //    |
                // help: use struct literal syntax instead
                //    |
                // LL | const Y: X = X {};
                //    |              ^^^^
                // help: a constant with a similar name exists
                //    |
                // LL | const Y: X = Y("ö");
                //    |              ^
                return false;
            }
            let prefix = match suggestion.target {
                SuggestionTarget::SimilarlyNamed => "similarly named ",
                SuggestionTarget::SingleItem => "",
            };

            err.span_label(
                self.session.source_map().guess_head_span(def_span),
                &format!(
                    "{}{} `{}` defined here",
                    prefix,
                    suggestion.res.descr(),
                    suggestion.candidate,
                ),
            );
        }
        let msg = match suggestion.target {
            SuggestionTarget::SimilarlyNamed => format!(
                "{} {} with a similar name exists",
                suggestion.res.article(),
                suggestion.res.descr()
            ),
            SuggestionTarget::SingleItem => {
                format!("maybe you meant this {}", suggestion.res.descr())
            }
        };
        err.span_suggestion(span, &msg, suggestion.candidate, Applicability::MaybeIncorrect);
        true
    }

    fn binding_description(&self, b: &NameBinding<'_>, ident: Ident, from_prelude: bool) -> String {
        let res = b.res();
        if b.span.is_dummy() || !self.session.source_map().is_span_accessible(b.span) {
            // These already contain the "built-in" prefix or look bad with it.
            let add_built_in =
                !matches!(b.res(), Res::NonMacroAttr(..) | Res::PrimTy(..) | Res::ToolMod);
            let (built_in, from) = if from_prelude {
                ("", " from prelude")
            } else if b.is_extern_crate()
                && !b.is_import()
                && self.session.opts.externs.get(ident.as_str()).is_some()
            {
                ("", " passed with `--extern`")
            } else if add_built_in {
                (" built-in", "")
            } else {
                ("", "")
            };

            let a = if built_in.is_empty() { res.article() } else { "a" };
            format!("{a}{built_in} {thing}{from}", thing = res.descr())
        } else {
            let introduced = if b.is_import() { "imported" } else { "defined" };
            format!("the {thing} {introduced} here", thing = res.descr())
        }
    }

    fn report_ambiguity_error(&self, ambiguity_error: &AmbiguityError<'_>) {
        let AmbiguityError { kind, ident, b1, b2, misc1, misc2 } = *ambiguity_error;
        let (b1, b2, misc1, misc2, swapped) = if b2.span.is_dummy() && !b1.span.is_dummy() {
            // We have to print the span-less alternative first, otherwise formatting looks bad.
            (b2, b1, misc2, misc1, true)
        } else {
            (b1, b2, misc1, misc2, false)
        };

        let mut err = struct_span_err!(self.session, ident.span, E0659, "`{ident}` is ambiguous");
        err.span_label(ident.span, "ambiguous name");
        err.note(&format!("ambiguous because of {}", kind.descr()));

        let mut could_refer_to = |b: &NameBinding<'_>, misc: AmbiguityErrorMisc, also: &str| {
            let what = self.binding_description(b, ident, misc == AmbiguityErrorMisc::FromPrelude);
            let note_msg = format!("`{ident}` could{also} refer to {what}");

            let thing = b.res().descr();
            let mut help_msgs = Vec::new();
            if b.is_glob_import()
                && (kind == AmbiguityKind::GlobVsGlob
                    || kind == AmbiguityKind::GlobVsExpanded
                    || kind == AmbiguityKind::GlobVsOuter && swapped != also.is_empty())
            {
                help_msgs.push(format!(
                    "consider adding an explicit import of `{ident}` to disambiguate"
                ))
            }
            if b.is_extern_crate() && ident.span.rust_2018() {
                help_msgs.push(format!("use `::{ident}` to refer to this {thing} unambiguously"))
            }
            if misc == AmbiguityErrorMisc::SuggestCrate {
                help_msgs
                    .push(format!("use `crate::{ident}` to refer to this {thing} unambiguously"))
            } else if misc == AmbiguityErrorMisc::SuggestSelf {
                help_msgs
                    .push(format!("use `self::{ident}` to refer to this {thing} unambiguously"))
            }

            err.span_note(b.span, &note_msg);
            for (i, help_msg) in help_msgs.iter().enumerate() {
                let or = if i == 0 { "" } else { "or " };
                err.help(&format!("{}{}", or, help_msg));
            }
        };

        could_refer_to(b1, misc1, "");
        could_refer_to(b2, misc2, " also");
        err.emit();
    }

    /// If the binding refers to a tuple struct constructor with fields,
    /// returns the span of its fields.
    fn ctor_fields_span(&self, binding: &NameBinding<'_>) -> Option<Span> {
        if let NameBindingKind::Res(
            Res::Def(DefKind::Ctor(CtorOf::Struct, CtorKind::Fn), ctor_def_id),
            _,
        ) = binding.kind
        {
            let def_id = self.parent(ctor_def_id);
            let fields = self.field_names.get(&def_id)?;
            return fields.iter().map(|name| name.span).reduce(Span::to); // None for `struct Foo()`
        }
        None
    }

    fn report_privacy_error(&self, privacy_error: &PrivacyError<'_>) {
        let PrivacyError { ident, binding, .. } = *privacy_error;

        let res = binding.res();
        let ctor_fields_span = self.ctor_fields_span(binding);
        let plain_descr = res.descr().to_string();
        let nonimport_descr =
            if ctor_fields_span.is_some() { plain_descr + " constructor" } else { plain_descr };
        let import_descr = nonimport_descr.clone() + " import";
        let get_descr =
            |b: &NameBinding<'_>| if b.is_import() { &import_descr } else { &nonimport_descr };

        // Print the primary message.
        let descr = get_descr(binding);
        let mut err =
            struct_span_err!(self.session, ident.span, E0603, "{} `{}` is private", descr, ident);
        err.span_label(ident.span, &format!("private {}", descr));
        if let Some(span) = ctor_fields_span {
            err.span_label(span, "a constructor is private if any of the fields is private");
        }

        // Print the whole import chain to make it easier to see what happens.
        let first_binding = binding;
        let mut next_binding = Some(binding);
        let mut next_ident = ident;
        while let Some(binding) = next_binding {
            let name = next_ident;
            next_binding = match binding.kind {
                _ if res == Res::Err => None,
                NameBindingKind::Import { binding, import, .. } => match import.kind {
                    _ if binding.span.is_dummy() => None,
                    ImportKind::Single { source, .. } => {
                        next_ident = source;
                        Some(binding)
                    }
                    ImportKind::Glob { .. } | ImportKind::MacroUse => Some(binding),
                    ImportKind::ExternCrate { .. } => None,
                },
                _ => None,
            };

            let first = ptr::eq(binding, first_binding);
            let msg = format!(
                "{and_refers_to}the {item} `{name}`{which} is defined here{dots}",
                and_refers_to = if first { "" } else { "...and refers to " },
                item = get_descr(binding),
                which = if first { "" } else { " which" },
                dots = if next_binding.is_some() { "..." } else { "" },
            );
            let def_span = self.session.source_map().guess_head_span(binding.span);
            let mut note_span = MultiSpan::from_span(def_span);
            if !first && binding.vis.is_public() {
                note_span.push_span_label(def_span, "consider importing it directly");
            }
            err.span_note(note_span, &msg);
        }

        err.emit();
    }

    pub(crate) fn find_similarly_named_module_or_crate(
        &mut self,
        ident: Symbol,
        current_module: &Module<'a>,
    ) -> Option<Symbol> {
        let mut candidates = self
            .extern_prelude
            .iter()
            .map(|(ident, _)| ident.name)
            .chain(
                self.module_map
                    .iter()
                    .filter(|(_, module)| {
                        current_module.is_ancestor_of(module) && !ptr::eq(current_module, *module)
                    })
                    .flat_map(|(_, module)| module.kind.name()),
            )
            .filter(|c| !c.to_string().is_empty())
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        match find_best_match_for_name(&candidates, ident, None) {
            Some(sugg) if sugg == ident => None,
            sugg => sugg,
        }
    }

    pub(crate) fn report_path_resolution_error(
        &mut self,
        path: &[Segment],
        opt_ns: Option<Namespace>, // `None` indicates a module path in import
        parent_scope: &ParentScope<'a>,
        ribs: Option<&PerNS<Vec<Rib<'a>>>>,
        ignore_binding: Option<&'a NameBinding<'a>>,
        module: Option<ModuleOrUniformRoot<'a>>,
        i: usize,
        ident: Ident,
    ) -> (String, Option<Suggestion>) {
        let is_last = i == path.len() - 1;
        let ns = if is_last { opt_ns.unwrap_or(TypeNS) } else { TypeNS };
        let module_res = match module {
            Some(ModuleOrUniformRoot::Module(module)) => module.res(),
            _ => None,
        };
        if module_res == self.graph_root.res() {
            let is_mod = |res| matches!(res, Res::Def(DefKind::Mod, _));
            let mut candidates = self.lookup_import_candidates(ident, TypeNS, parent_scope, is_mod);
            candidates
                .sort_by_cached_key(|c| (c.path.segments.len(), pprust::path_to_string(&c.path)));
            if let Some(candidate) = candidates.get(0) {
                (
                    String::from("unresolved import"),
                    Some((
                        vec![(ident.span, pprust::path_to_string(&candidate.path))],
                        String::from("a similar path exists"),
                        Applicability::MaybeIncorrect,
                    )),
                )
            } else if self.session.edition() == Edition::Edition2015 {
                (
                    format!("maybe a missing crate `{ident}`?"),
                    Some((
                        vec![],
                        format!(
                            "consider adding `extern crate {ident}` to use the `{ident}` crate"
                        ),
                        Applicability::MaybeIncorrect,
                    )),
                )
            } else {
                (format!("could not find `{ident}` in the crate root"), None)
            }
        } else if i > 0 {
            let parent = path[i - 1].ident.name;
            let parent = match parent {
                // ::foo is mounted at the crate root for 2015, and is the extern
                // prelude for 2018+
                kw::PathRoot if self.session.edition() > Edition::Edition2015 => {
                    "the list of imported crates".to_owned()
                }
                kw::PathRoot | kw::Crate => "the crate root".to_owned(),
                _ => format!("`{parent}`"),
            };

            let mut msg = format!("could not find `{}` in {}", ident, parent);
            if ns == TypeNS || ns == ValueNS {
                let ns_to_try = if ns == TypeNS { ValueNS } else { TypeNS };
                let binding = if let Some(module) = module {
                    self.resolve_ident_in_module(
                        module,
                        ident,
                        ns_to_try,
                        parent_scope,
                        None,
                        ignore_binding,
                    ).ok()
                } else if let Some(ribs) = ribs
                    && let Some(TypeNS | ValueNS) = opt_ns
                {
                    match self.resolve_ident_in_lexical_scope(
                        ident,
                        ns_to_try,
                        parent_scope,
                        None,
                        &ribs[ns_to_try],
                        ignore_binding,
                    ) {
                        // we found a locally-imported or available item/module
                        Some(LexicalScopeBinding::Item(binding)) => Some(binding),
                        _ => None,
                    }
                } else {
                    let scopes = ScopeSet::All(ns_to_try, opt_ns.is_none());
                    self.early_resolve_ident_in_lexical_scope(
                        ident,
                        scopes,
                        parent_scope,
                        None,
                        false,
                        ignore_binding,
                    ).ok()
                };
                if let Some(binding) = binding {
                    let mut found = |what| {
                        msg = format!(
                            "expected {}, found {} `{}` in {}",
                            ns.descr(),
                            what,
                            ident,
                            parent
                        )
                    };
                    if binding.module().is_some() {
                        found("module")
                    } else {
                        match binding.res() {
                            Res::Def(kind, id) => found(kind.descr(id)),
                            _ => found(ns_to_try.descr()),
                        }
                    }
                };
            }
            (msg, None)
        } else if ident.name == kw::SelfUpper {
            ("`Self` is only available in impls, traits, and type definitions".to_string(), None)
        } else if ident.name.as_str().chars().next().map_or(false, |c| c.is_ascii_uppercase()) {
            // Check whether the name refers to an item in the value namespace.
            let binding = if let Some(ribs) = ribs {
                self.resolve_ident_in_lexical_scope(
                    ident,
                    ValueNS,
                    parent_scope,
                    None,
                    &ribs[ValueNS],
                    ignore_binding,
                )
            } else {
                None
            };
            let match_span = match binding {
                // Name matches a local variable. For example:
                // ```
                // fn f() {
                //     let Foo: &str = "";
                //     println!("{}", Foo::Bar); // Name refers to local
                //                               // variable `Foo`.
                // }
                // ```
                Some(LexicalScopeBinding::Res(Res::Local(id))) => {
                    Some(*self.pat_span_map.get(&id).unwrap())
                }
                // Name matches item from a local name binding
                // created by `use` declaration. For example:
                // ```
                // pub Foo: &str = "";
                //
                // mod submod {
                //     use super::Foo;
                //     println!("{}", Foo::Bar); // Name refers to local
                //                               // binding `Foo`.
                // }
                // ```
                Some(LexicalScopeBinding::Item(name_binding)) => Some(name_binding.span),
                _ => None,
            };
            let suggestion = if let Some(span) = match_span {
                Some((
                    vec![(span, String::from(""))],
                    format!("`{}` is defined here, but is not a type", ident),
                    Applicability::MaybeIncorrect,
                ))
            } else {
                None
            };

            (format!("use of undeclared type `{}`", ident), suggestion)
        } else {
            let suggestion = if ident.name == sym::alloc {
                Some((
                    vec![],
                    String::from("add `extern crate alloc` to use the `alloc` crate"),
                    Applicability::MaybeIncorrect,
                ))
            } else {
                self.find_similarly_named_module_or_crate(ident.name, &parent_scope.module).map(
                    |sugg| {
                        (
                            vec![(ident.span, sugg.to_string())],
                            String::from("there is a crate or module with a similar name"),
                            Applicability::MaybeIncorrect,
                        )
                    },
                )
            };
            (format!("use of undeclared crate or module `{}`", ident), suggestion)
        }
    }
}

impl<'a, 'b> ImportResolver<'a, 'b> {
    /// Adds suggestions for a path that cannot be resolved.
    pub(crate) fn make_path_suggestion(
        &mut self,
        span: Span,
        mut path: Vec<Segment>,
        parent_scope: &ParentScope<'b>,
    ) -> Option<(Vec<Segment>, Vec<String>)> {
        debug!("make_path_suggestion: span={:?} path={:?}", span, path);

        match (path.get(0), path.get(1)) {
            // `{{root}}::ident::...` on both editions.
            // On 2015 `{{root}}` is usually added implicitly.
            (Some(fst), Some(snd))
                if fst.ident.name == kw::PathRoot && !snd.ident.is_path_segment_keyword() => {}
            // `ident::...` on 2018.
            (Some(fst), _)
                if fst.ident.span.rust_2018() && !fst.ident.is_path_segment_keyword() =>
            {
                // Insert a placeholder that's later replaced by `self`/`super`/etc.
                path.insert(0, Segment::from_ident(Ident::empty()));
            }
            _ => return None,
        }

        self.make_missing_self_suggestion(path.clone(), parent_scope)
            .or_else(|| self.make_missing_crate_suggestion(path.clone(), parent_scope))
            .or_else(|| self.make_missing_super_suggestion(path.clone(), parent_scope))
            .or_else(|| self.make_external_crate_suggestion(path, parent_scope))
    }

    /// Suggest a missing `self::` if that resolves to an correct module.
    ///
    /// ```text
    ///    |
    /// LL | use foo::Bar;
    ///    |     ^^^ did you mean `self::foo`?
    /// ```
    fn make_missing_self_suggestion(
        &mut self,
        mut path: Vec<Segment>,
        parent_scope: &ParentScope<'b>,
    ) -> Option<(Vec<Segment>, Vec<String>)> {
        // Replace first ident with `self` and check if that is valid.
        path[0].ident.name = kw::SelfLower;
        let result = self.r.maybe_resolve_path(&path, None, parent_scope);
        debug!("make_missing_self_suggestion: path={:?} result={:?}", path, result);
        if let PathResult::Module(..) = result { Some((path, Vec::new())) } else { None }
    }

    /// Suggests a missing `crate::` if that resolves to an correct module.
    ///
    /// ```text
    ///    |
    /// LL | use foo::Bar;
    ///    |     ^^^ did you mean `crate::foo`?
    /// ```
    fn make_missing_crate_suggestion(
        &mut self,
        mut path: Vec<Segment>,
        parent_scope: &ParentScope<'b>,
    ) -> Option<(Vec<Segment>, Vec<String>)> {
        // Replace first ident with `crate` and check if that is valid.
        path[0].ident.name = kw::Crate;
        let result = self.r.maybe_resolve_path(&path, None, parent_scope);
        debug!("make_missing_crate_suggestion:  path={:?} result={:?}", path, result);
        if let PathResult::Module(..) = result {
            Some((
                path,
                vec![
                    "`use` statements changed in Rust 2018; read more at \
                     <https://doc.rust-lang.org/edition-guide/rust-2018/module-system/path-\
                     clarity.html>"
                        .to_string(),
                ],
            ))
        } else {
            None
        }
    }

    /// Suggests a missing `super::` if that resolves to an correct module.
    ///
    /// ```text
    ///    |
    /// LL | use foo::Bar;
    ///    |     ^^^ did you mean `super::foo`?
    /// ```
    fn make_missing_super_suggestion(
        &mut self,
        mut path: Vec<Segment>,
        parent_scope: &ParentScope<'b>,
    ) -> Option<(Vec<Segment>, Vec<String>)> {
        // Replace first ident with `crate` and check if that is valid.
        path[0].ident.name = kw::Super;
        let result = self.r.maybe_resolve_path(&path, None, parent_scope);
        debug!("make_missing_super_suggestion:  path={:?} result={:?}", path, result);
        if let PathResult::Module(..) = result { Some((path, Vec::new())) } else { None }
    }

    /// Suggests a missing external crate name if that resolves to an correct module.
    ///
    /// ```text
    ///    |
    /// LL | use foobar::Baz;
    ///    |     ^^^^^^ did you mean `baz::foobar`?
    /// ```
    ///
    /// Used when importing a submodule of an external crate but missing that crate's
    /// name as the first part of path.
    fn make_external_crate_suggestion(
        &mut self,
        mut path: Vec<Segment>,
        parent_scope: &ParentScope<'b>,
    ) -> Option<(Vec<Segment>, Vec<String>)> {
        if path[1].ident.span.rust_2015() {
            return None;
        }

        // Sort extern crate names in *reverse* order to get
        // 1) some consistent ordering for emitted diagnostics, and
        // 2) `std` suggestions before `core` suggestions.
        let mut extern_crate_names =
            self.r.extern_prelude.iter().map(|(ident, _)| ident.name).collect::<Vec<_>>();
        extern_crate_names.sort_by(|a, b| b.as_str().partial_cmp(a.as_str()).unwrap());

        for name in extern_crate_names.into_iter() {
            // Replace first ident with a crate name and check if that is valid.
            path[0].ident.name = name;
            let result = self.r.maybe_resolve_path(&path, None, parent_scope);
            debug!(
                "make_external_crate_suggestion: name={:?} path={:?} result={:?}",
                name, path, result
            );
            if let PathResult::Module(..) = result {
                return Some((path, Vec::new()));
            }
        }

        None
    }

    /// Suggests importing a macro from the root of the crate rather than a module within
    /// the crate.
    ///
    /// ```text
    /// help: a macro with this name exists at the root of the crate
    ///    |
    /// LL | use issue_59764::makro;
    ///    |     ^^^^^^^^^^^^^^^^^^
    ///    |
    ///    = note: this could be because a macro annotated with `#[macro_export]` will be exported
    ///            at the root of the crate instead of the module where it is defined
    /// ```
    pub(crate) fn check_for_module_export_macro(
        &mut self,
        import: &'b Import<'b>,
        module: ModuleOrUniformRoot<'b>,
        ident: Ident,
    ) -> Option<(Option<Suggestion>, Vec<String>)> {
        let ModuleOrUniformRoot::Module(mut crate_module) = module else {
            return None;
        };

        while let Some(parent) = crate_module.parent {
            crate_module = parent;
        }

        if ModuleOrUniformRoot::same_def(ModuleOrUniformRoot::Module(crate_module), module) {
            // Don't make a suggestion if the import was already from the root of the
            // crate.
            return None;
        }

        let resolutions = self.r.resolutions(crate_module).borrow();
        let resolution = resolutions.get(&self.r.new_key(ident, MacroNS))?;
        let binding = resolution.borrow().binding()?;
        if let Res::Def(DefKind::Macro(MacroKind::Bang), _) = binding.res() {
            let module_name = crate_module.kind.name().unwrap();
            let import_snippet = match import.kind {
                ImportKind::Single { source, target, .. } if source != target => {
                    format!("{} as {}", source, target)
                }
                _ => format!("{}", ident),
            };

            let mut corrections: Vec<(Span, String)> = Vec::new();
            if !import.is_nested() {
                // Assume this is the easy case of `use issue_59764::foo::makro;` and just remove
                // intermediate segments.
                corrections.push((import.span, format!("{}::{}", module_name, import_snippet)));
            } else {
                // Find the binding span (and any trailing commas and spaces).
                //   ie. `use a::b::{c, d, e};`
                //                      ^^^
                let (found_closing_brace, binding_span) = find_span_of_binding_until_next_binding(
                    self.r.session,
                    import.span,
                    import.use_span,
                );
                debug!(
                    "check_for_module_export_macro: found_closing_brace={:?} binding_span={:?}",
                    found_closing_brace, binding_span
                );

                let mut removal_span = binding_span;
                if found_closing_brace {
                    // If the binding span ended with a closing brace, as in the below example:
                    //   ie. `use a::b::{c, d};`
                    //                      ^
                    // Then expand the span of characters to remove to include the previous
                    // binding's trailing comma.
                    //   ie. `use a::b::{c, d};`
                    //                    ^^^
                    if let Some(previous_span) =
                        extend_span_to_previous_binding(self.r.session, binding_span)
                    {
                        debug!("check_for_module_export_macro: previous_span={:?}", previous_span);
                        removal_span = removal_span.with_lo(previous_span.lo());
                    }
                }
                debug!("check_for_module_export_macro: removal_span={:?}", removal_span);

                // Remove the `removal_span`.
                corrections.push((removal_span, "".to_string()));

                // Find the span after the crate name and if it has nested imports immediately
                // after the crate name already.
                //   ie. `use a::b::{c, d};`
                //               ^^^^^^^^^
                //   or  `use a::{b, c, d}};`
                //               ^^^^^^^^^^^
                let (has_nested, after_crate_name) = find_span_immediately_after_crate_name(
                    self.r.session,
                    module_name,
                    import.use_span,
                );
                debug!(
                    "check_for_module_export_macro: has_nested={:?} after_crate_name={:?}",
                    has_nested, after_crate_name
                );

                let source_map = self.r.session.source_map();

                // Add the import to the start, with a `{` if required.
                let start_point = source_map.start_point(after_crate_name);
                if let Ok(start_snippet) = source_map.span_to_snippet(start_point) {
                    corrections.push((
                        start_point,
                        if has_nested {
                            // In this case, `start_snippet` must equal '{'.
                            format!("{}{}, ", start_snippet, import_snippet)
                        } else {
                            // In this case, add a `{`, then the moved import, then whatever
                            // was there before.
                            format!("{{{}, {}", import_snippet, start_snippet)
                        },
                    ));
                }

                // Add a `};` to the end if nested, matching the `{` added at the start.
                if !has_nested {
                    corrections.push((source_map.end_point(after_crate_name), "};".to_string()));
                }
            }

            let suggestion = Some((
                corrections,
                String::from("a macro with this name exists at the root of the crate"),
                Applicability::MaybeIncorrect,
            ));
            let note = vec![
                "this could be because a macro annotated with `#[macro_export]` will be exported \
                 at the root of the crate instead of the module where it is defined"
                    .to_string(),
            ];
            Some((suggestion, note))
        } else {
            None
        }
    }
}

/// Given a `binding_span` of a binding within a use statement:
///
/// ```ignore (illustrative)
/// use foo::{a, b, c};
/// //           ^
/// ```
///
/// then return the span until the next binding or the end of the statement:
///
/// ```ignore (illustrative)
/// use foo::{a, b, c};
/// //           ^^^
/// ```
fn find_span_of_binding_until_next_binding(
    sess: &Session,
    binding_span: Span,
    use_span: Span,
) -> (bool, Span) {
    let source_map = sess.source_map();

    // Find the span of everything after the binding.
    //   ie. `a, e};` or `a};`
    let binding_until_end = binding_span.with_hi(use_span.hi());

    // Find everything after the binding but not including the binding.
    //   ie. `, e};` or `};`
    let after_binding_until_end = binding_until_end.with_lo(binding_span.hi());

    // Keep characters in the span until we encounter something that isn't a comma or
    // whitespace.
    //   ie. `, ` or ``.
    //
    // Also note whether a closing brace character was encountered. If there
    // was, then later go backwards to remove any trailing commas that are left.
    let mut found_closing_brace = false;
    let after_binding_until_next_binding =
        source_map.span_take_while(after_binding_until_end, |&ch| {
            if ch == '}' {
                found_closing_brace = true;
            }
            ch == ' ' || ch == ','
        });

    // Combine the two spans.
    //   ie. `a, ` or `a`.
    //
    // Removing these would leave `issue_52891::{d, e};` or `issue_52891::{d, e, };`
    let span = binding_span.with_hi(after_binding_until_next_binding.hi());

    (found_closing_brace, span)
}

/// Given a `binding_span`, return the span through to the comma or opening brace of the previous
/// binding.
///
/// ```ignore (illustrative)
/// use foo::a::{a, b, c};
/// //            ^^--- binding span
/// //            |
/// //            returned span
///
/// use foo::{a, b, c};
/// //        --- binding span
/// ```
fn extend_span_to_previous_binding(sess: &Session, binding_span: Span) -> Option<Span> {
    let source_map = sess.source_map();

    // `prev_source` will contain all of the source that came before the span.
    // Then split based on a command and take the first (ie. closest to our span)
    // snippet. In the example, this is a space.
    let prev_source = source_map.span_to_prev_source(binding_span).ok()?;

    let prev_comma = prev_source.rsplit(',').collect::<Vec<_>>();
    let prev_starting_brace = prev_source.rsplit('{').collect::<Vec<_>>();
    if prev_comma.len() <= 1 || prev_starting_brace.len() <= 1 {
        return None;
    }

    let prev_comma = prev_comma.first().unwrap();
    let prev_starting_brace = prev_starting_brace.first().unwrap();

    // If the amount of source code before the comma is greater than
    // the amount of source code before the starting brace then we've only
    // got one item in the nested item (eg. `issue_52891::{self}`).
    if prev_comma.len() > prev_starting_brace.len() {
        return None;
    }

    Some(binding_span.with_lo(BytePos(
        // Take away the number of bytes for the characters we've found and an
        // extra for the comma.
        binding_span.lo().0 - (prev_comma.as_bytes().len() as u32) - 1,
    )))
}

/// Given a `use_span` of a binding within a use statement, returns the highlighted span and if
/// it is a nested use tree.
///
/// ```ignore (illustrative)
/// use foo::a::{b, c};
/// //       ^^^^^^^^^^ -- false
///
/// use foo::{a, b, c};
/// //       ^^^^^^^^^^ -- true
///
/// use foo::{a, b::{c, d}};
/// //       ^^^^^^^^^^^^^^^ -- true
/// ```
fn find_span_immediately_after_crate_name(
    sess: &Session,
    module_name: Symbol,
    use_span: Span,
) -> (bool, Span) {
    debug!(
        "find_span_immediately_after_crate_name: module_name={:?} use_span={:?}",
        module_name, use_span
    );
    let source_map = sess.source_map();

    // Using `use issue_59764::foo::{baz, makro};` as an example throughout..
    let mut num_colons = 0;
    // Find second colon.. `use issue_59764:`
    let until_second_colon = source_map.span_take_while(use_span, |c| {
        if *c == ':' {
            num_colons += 1;
        }
        !matches!(c, ':' if num_colons == 2)
    });
    // Find everything after the second colon.. `foo::{baz, makro};`
    let from_second_colon = use_span.with_lo(until_second_colon.hi() + BytePos(1));

    let mut found_a_non_whitespace_character = false;
    // Find the first non-whitespace character in `from_second_colon`.. `f`
    let after_second_colon = source_map.span_take_while(from_second_colon, |c| {
        if found_a_non_whitespace_character {
            return false;
        }
        if !c.is_whitespace() {
            found_a_non_whitespace_character = true;
        }
        true
    });

    // Find the first `{` in from_second_colon.. `foo::{`
    let next_left_bracket = source_map.span_through_char(from_second_colon, '{');

    (next_left_bracket == after_second_colon, from_second_colon)
}

/// A suggestion has already been emitted, change the wording slightly to clarify that both are
/// independent options.
enum Instead {
    Yes,
    No,
}

/// Whether an existing place with an `use` item was found.
enum FoundUse {
    Yes,
    No,
}

/// Whether a binding is part of a pattern or an expression. Used for diagnostics.
enum IsPattern {
    /// The binding is part of a pattern
    Yes,
    /// The binding is part of an expression
    No,
}

/// When an entity with a given name is not available in scope, we search for
/// entities with that name in all crates. This method allows outputting the
/// results of this search in a programmer-friendly way
fn show_candidates(
    session: &Session,
    source_span: &IndexVec<LocalDefId, Span>,
    err: &mut Diagnostic,
    // This is `None` if all placement locations are inside expansions
    use_placement_span: Option<Span>,
    candidates: &[ImportSuggestion],
    instead: Instead,
    found_use: FoundUse,
    is_pattern: IsPattern,
    path: Vec<Segment>,
) {
    if candidates.is_empty() {
        return;
    }

    let mut accessible_path_strings: Vec<(String, &str, Option<DefId>, &Option<String>)> =
        Vec::new();
    let mut inaccessible_path_strings: Vec<(String, &str, Option<DefId>, &Option<String>)> =
        Vec::new();

    candidates.iter().for_each(|c| {
        (if c.accessible { &mut accessible_path_strings } else { &mut inaccessible_path_strings })
            .push((path_names_to_string(&c.path), c.descr, c.did, &c.note))
    });

    // we want consistent results across executions, but candidates are produced
    // by iterating through a hash map, so make sure they are ordered:
    for path_strings in [&mut accessible_path_strings, &mut inaccessible_path_strings] {
        path_strings.sort_by(|a, b| a.0.cmp(&b.0));
        let core_path_strings =
            path_strings.drain_filter(|p| p.0.starts_with("core::")).collect::<Vec<_>>();
        path_strings.extend(core_path_strings);
        path_strings.dedup_by(|a, b| a.0 == b.0);
    }

    if !accessible_path_strings.is_empty() {
        let (determiner, kind, name) = if accessible_path_strings.len() == 1 {
            ("this", accessible_path_strings[0].1, format!(" `{}`", accessible_path_strings[0].0))
        } else {
            ("one of these", "items", String::new())
        };

        let instead = if let Instead::Yes = instead { " instead" } else { "" };
        let mut msg = if let IsPattern::Yes = is_pattern {
            format!(
                "if you meant to match on {}{}{}, use the full path in the pattern",
                kind, instead, name
            )
        } else {
            format!("consider importing {} {}{}", determiner, kind, instead)
        };

        for note in accessible_path_strings.iter().flat_map(|cand| cand.3.as_ref()) {
            err.note(note);
        }

        if let (IsPattern::Yes, Some(span)) = (is_pattern, use_placement_span) {
            err.span_suggestions(
                span,
                &msg,
                accessible_path_strings.into_iter().map(|a| a.0),
                Applicability::MaybeIncorrect,
            );
        } else if let Some(span) = use_placement_span {
            for candidate in &mut accessible_path_strings {
                // produce an additional newline to separate the new use statement
                // from the directly following item.
                let additional_newline = if let FoundUse::Yes = found_use { "" } else { "\n" };
                candidate.0 = format!("use {};\n{}", &candidate.0, additional_newline);
            }

            err.span_suggestions(
                span,
                &msg,
                accessible_path_strings.into_iter().map(|a| a.0),
                Applicability::MaybeIncorrect,
            );
            if let [first, .., last] = &path[..] {
                err.span_suggestion_verbose(
                    first.ident.span.until(last.ident.span),
                    &format!("if you import `{}`, refer to it directly", last.ident),
                    "",
                    Applicability::Unspecified,
                );
            }
        } else {
            msg.push(':');

            for candidate in accessible_path_strings {
                msg.push('\n');
                msg.push_str(&candidate.0);
            }

            err.note(&msg);
        }
    } else {
        assert!(!inaccessible_path_strings.is_empty());

        let prefix =
            if let IsPattern::Yes = is_pattern { "you might have meant to match on " } else { "" };
        if inaccessible_path_strings.len() == 1 {
            let (name, descr, def_id, note) = &inaccessible_path_strings[0];
            let msg = format!(
                "{}{} `{}`{} exists but is inaccessible",
                prefix,
                descr,
                name,
                if let IsPattern::Yes = is_pattern { ", which" } else { "" }
            );

            if let Some(local_def_id) = def_id.and_then(|did| did.as_local()) {
                let span = source_span[local_def_id];
                let span = session.source_map().guess_head_span(span);
                let mut multi_span = MultiSpan::from_span(span);
                multi_span.push_span_label(span, "not accessible");
                err.span_note(multi_span, &msg);
            } else {
                err.note(&msg);
            }
            if let Some(note) = (*note).as_deref() {
                err.note(note);
            }
        } else {
            let (_, descr_first, _, _) = &inaccessible_path_strings[0];
            let descr = if inaccessible_path_strings
                .iter()
                .skip(1)
                .all(|(_, descr, _, _)| descr == descr_first)
            {
                descr_first.to_string()
            } else {
                "item".to_string()
            };
            let plural_descr =
                if descr.ends_with("s") { format!("{}es", descr) } else { format!("{}s", descr) };

            let mut msg = format!("{}these {} exist but are inaccessible", prefix, plural_descr);
            let mut has_colon = false;

            let mut spans = Vec::new();
            for (name, _, def_id, _) in &inaccessible_path_strings {
                if let Some(local_def_id) = def_id.and_then(|did| did.as_local()) {
                    let span = source_span[local_def_id];
                    let span = session.source_map().guess_head_span(span);
                    spans.push((name, span));
                } else {
                    if !has_colon {
                        msg.push(':');
                        has_colon = true;
                    }
                    msg.push('\n');
                    msg.push_str(name);
                }
            }

            let mut multi_span = MultiSpan::from_spans(spans.iter().map(|(_, sp)| *sp).collect());
            for (name, span) in spans {
                multi_span.push_span_label(span, format!("`{}`: not accessible", name));
            }

            for note in inaccessible_path_strings.iter().flat_map(|cand| cand.3.as_ref()) {
                err.note(note);
            }

            err.span_note(multi_span, &msg);
        }
    }
}

#[derive(Debug)]
struct UsePlacementFinder {
    target_module: NodeId,
    first_legal_span: Option<Span>,
    first_use_span: Option<Span>,
}

impl UsePlacementFinder {
    fn check(krate: &Crate, target_module: NodeId) -> (Option<Span>, FoundUse) {
        let mut finder =
            UsePlacementFinder { target_module, first_legal_span: None, first_use_span: None };
        finder.visit_crate(krate);
        if let Some(use_span) = finder.first_use_span {
            (Some(use_span), FoundUse::Yes)
        } else {
            (finder.first_legal_span, FoundUse::No)
        }
    }
}

impl<'tcx> visit::Visitor<'tcx> for UsePlacementFinder {
    fn visit_crate(&mut self, c: &Crate) {
        if self.target_module == CRATE_NODE_ID {
            let inject = c.spans.inject_use_span;
            if is_span_suitable_for_use_injection(inject) {
                self.first_legal_span = Some(inject);
            }
            self.first_use_span = search_for_any_use_in_items(&c.items);
            return;
        } else {
            visit::walk_crate(self, c);
        }
    }

    fn visit_item(&mut self, item: &'tcx ast::Item) {
        if self.target_module == item.id {
            if let ItemKind::Mod(_, ModKind::Loaded(items, _inline, mod_spans)) = &item.kind {
                let inject = mod_spans.inject_use_span;
                if is_span_suitable_for_use_injection(inject) {
                    self.first_legal_span = Some(inject);
                }
                self.first_use_span = search_for_any_use_in_items(items);
                return;
            }
        } else {
            visit::walk_item(self, item);
        }
    }
}

fn search_for_any_use_in_items(items: &[P<ast::Item>]) -> Option<Span> {
    for item in items {
        if let ItemKind::Use(..) = item.kind {
            if is_span_suitable_for_use_injection(item.span) {
                return Some(item.span.shrink_to_lo());
            }
        }
    }
    return None;
}

fn is_span_suitable_for_use_injection(s: Span) -> bool {
    // don't suggest placing a use before the prelude
    // import or other generated ones
    !s.from_expansion()
}

/// Convert the given number into the corresponding ordinal
pub(crate) fn ordinalize(v: usize) -> String {
    let suffix = match ((11..=13).contains(&(v % 100)), v % 10) {
        (false, 1) => "st",
        (false, 2) => "nd",
        (false, 3) => "rd",
        _ => "th",
    };
    format!("{v}{suffix}")
}
