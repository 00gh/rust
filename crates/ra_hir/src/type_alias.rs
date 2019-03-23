//! HIR for type aliases (i.e. the `type` keyword).

use std::sync::Arc;

use crate::{TypeAlias, db::DefDatabase, type_ref::TypeRef};

pub(crate) fn type_alias_ref_query(db: &impl DefDatabase, typ: TypeAlias) -> Arc<TypeRef> {
    let (_, node) = typ.source(db);
    Arc::new(TypeRef::from_ast_opt(node.type_ref()))
}
