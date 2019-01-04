mod scope;

use std::{
    cmp::{max, min},
    sync::Arc,
};

use ra_db::Cancelable;
use ra_syntax::{
    TextRange, TextUnit,
    ast::{self, AstNode, DocCommentsOwner, NameOwner},
};

use crate::{DefId, DefKind, HirDatabase, ty::InferenceResult, Module, Crate, impl_block::ImplBlock};

pub use self::scope::FnScopes;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    def_id: DefId,
}

impl Function {
    pub(crate) fn new(def_id: DefId) -> Function {
        Function { def_id }
    }

    pub fn def_id(&self) -> DefId {
        self.def_id
    }

    pub fn syntax(&self, db: &impl HirDatabase) -> ast::FnDefNode {
        let def_loc = self.def_id.loc(db);
        assert!(def_loc.kind == DefKind::Function);
        let syntax = db.file_item(def_loc.source_item_id);
        ast::FnDef::cast(syntax.borrowed()).unwrap().owned()
    }

    pub fn scopes(&self, db: &impl HirDatabase) -> Arc<FnScopes> {
        db.fn_scopes(self.def_id)
    }

    pub fn signature_info(&self, db: &impl HirDatabase) -> Option<FnSignatureInfo> {
        let syntax = self.syntax(db);
        FnSignatureInfo::new(syntax.borrowed())
    }

    pub fn infer(&self, db: &impl HirDatabase) -> Cancelable<Arc<InferenceResult>> {
        db.infer(self.def_id)
    }

    pub fn module(&self, db: &impl HirDatabase) -> Cancelable<Module> {
        self.def_id.module(db)
    }

    pub fn krate(&self, db: &impl HirDatabase) -> Cancelable<Option<Crate>> {
        self.def_id.krate(db)
    }

    /// The containing impl block, if this is a method.
    pub fn impl_block(&self, db: &impl HirDatabase) -> Cancelable<Option<ImplBlock>> {
        self.def_id.impl_block(db)
    }
}

#[derive(Debug, Clone)]
pub struct FnSignatureInfo {
    pub name: String,
    pub label: String,
    pub ret_type: Option<String>,
    pub params: Vec<String>,
    pub doc: Option<String>,
}

impl FnSignatureInfo {
    fn new(node: ast::FnDef) -> Option<Self> {
        let name = node.name()?.text().to_string();

        let mut doc = None;

        // Strip the body out for the label.
        let mut label: String = if let Some(body) = node.body() {
            let body_range = body.syntax().range();
            let label: String = node
                .syntax()
                .children()
                .filter(|child| !child.range().is_subrange(&body_range))
                .map(|node| node.text().to_string())
                .collect();
            label
        } else {
            node.syntax().text().to_string()
        };

        if let Some((comment_range, docs)) = FnSignatureInfo::extract_doc_comments(node) {
            let comment_range = comment_range
                .checked_sub(node.syntax().range().start())
                .unwrap();
            let start = comment_range.start().to_usize();
            let end = comment_range.end().to_usize();

            // Remove the comment from the label
            label.replace_range(start..end, "");

            // Massage markdown
            let mut processed_lines = Vec::new();
            let mut in_code_block = false;
            for line in docs.lines() {
                if line.starts_with("```") {
                    in_code_block = !in_code_block;
                }

                let line = if in_code_block && line.starts_with("```") && !line.contains("rust") {
                    "```rust".into()
                } else {
                    line.to_string()
                };

                processed_lines.push(line);
            }

            if !processed_lines.is_empty() {
                doc = Some(processed_lines.join("\n"));
            }
        }

        let params = FnSignatureInfo::param_list(node);
        let ret_type = node.ret_type().map(|r| r.syntax().text().to_string());

        Some(FnSignatureInfo {
            name,
            ret_type,
            params,
            label: label.trim().to_owned(),
            doc,
        })
    }

    fn extract_doc_comments(node: ast::FnDef) -> Option<(TextRange, String)> {
        if node.doc_comments().count() == 0 {
            return None;
        }

        let comment_text = node.doc_comment_text();

        let (begin, end) = node
            .doc_comments()
            .map(|comment| comment.syntax().range())
            .map(|range| (range.start().to_usize(), range.end().to_usize()))
            .fold((std::usize::MAX, std::usize::MIN), |acc, range| {
                (min(acc.0, range.0), max(acc.1, range.1))
            });

        let range = TextRange::from_to(TextUnit::from_usize(begin), TextUnit::from_usize(end));

        Some((range, comment_text))
    }

    fn param_list(node: ast::FnDef) -> Vec<String> {
        let mut res = vec![];
        if let Some(param_list) = node.param_list() {
            if let Some(self_param) = param_list.self_param() {
                res.push(self_param.syntax().text().to_string())
            }

            // Maybe use param.pat here? See if we can just extract the name?
            //res.extend(param_list.params().map(|p| p.syntax().text().to_string()));
            res.extend(
                param_list
                    .params()
                    .filter_map(|p| p.pat())
                    .map(|pat| pat.syntax().text().to_string()),
            );
        }
        res
    }
}
