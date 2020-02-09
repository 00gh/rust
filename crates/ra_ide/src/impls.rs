//! FIXME: write short doc here

use hir::{Crate, ImplBlock, SourceBinder};
use ra_db::SourceDatabase;
use ra_ide_db::RootDatabase;
use ra_syntax::{algo::find_node_at_offset, ast, AstNode};

use crate::{display::ToNav, FilePosition, NavigationTarget, RangeInfo};

pub(crate) fn goto_implementation(
    db: &RootDatabase,
    position: FilePosition,
) -> Option<RangeInfo<Vec<NavigationTarget>>> {
    let parse = db.parse(position.file_id);
    let syntax = parse.tree().syntax().clone();
    let mut sb = SourceBinder::new(db);

    let krate = sb.to_module_def(position.file_id)?.krate();

    if let Some(nominal_def) = find_node_at_offset::<ast::NominalDef>(&syntax, position.offset) {
        return Some(RangeInfo::new(
            nominal_def.syntax().text_range(),
            impls_for_def(&mut sb, position, &nominal_def, krate)?,
        ));
    } else if let Some(trait_def) = find_node_at_offset::<ast::TraitDef>(&syntax, position.offset) {
        return Some(RangeInfo::new(
            trait_def.syntax().text_range(),
            impls_for_trait(&mut sb, position, &trait_def, krate)?,
        ));
    }

    None
}

fn impls_for_def(
    sb: &mut SourceBinder<RootDatabase>,
    position: FilePosition,
    node: &ast::NominalDef,
    krate: Crate,
) -> Option<Vec<NavigationTarget>> {
    let ty = match node {
        ast::NominalDef::StructDef(def) => {
            let src = hir::InFile { file_id: position.file_id.into(), value: def.clone() };
            sb.to_def(src)?.ty(sb.db)
        }
        ast::NominalDef::EnumDef(def) => {
            let src = hir::InFile { file_id: position.file_id.into(), value: def.clone() };
            sb.to_def(src)?.ty(sb.db)
        }
        ast::NominalDef::UnionDef(def) => {
            let src = hir::InFile { file_id: position.file_id.into(), value: def.clone() };
            sb.to_def(src)?.ty(sb.db)
        }
    };

    let impls = ImplBlock::all_in_crate(sb.db, krate);

    Some(
        impls
            .into_iter()
            .filter(|impl_block| ty.is_equal_for_find_impls(&impl_block.target_ty(sb.db)))
            .map(|imp| imp.to_nav(sb.db))
            .collect(),
    )
}

fn impls_for_trait(
    sb: &mut SourceBinder<RootDatabase>,
    position: FilePosition,
    node: &ast::TraitDef,
    krate: Crate,
) -> Option<Vec<NavigationTarget>> {
    let src = hir::InFile { file_id: position.file_id.into(), value: node.clone() };
    let tr = sb.to_def(src)?;

    let impls = ImplBlock::for_trait(sb.db, krate, tr);

    Some(impls.into_iter().map(|imp| imp.to_nav(sb.db)).collect())
}

#[cfg(test)]
mod tests {
    use crate::mock_analysis::analysis_and_position;

    fn check_goto(fixture: &str, expected: &[&str]) {
        let (analysis, pos) = analysis_and_position(fixture);

        let mut navs = analysis.goto_implementation(pos).unwrap().unwrap().info;
        assert_eq!(navs.len(), expected.len());
        navs.sort_by_key(|nav| (nav.file_id(), nav.full_range().start()));
        navs.into_iter().enumerate().for_each(|(i, nav)| nav.assert_match(expected[i]));
    }

    #[test]
    fn goto_implementation_works() {
        check_goto(
            "
            //- /lib.rs
            struct Foo<|>;
            impl Foo {}
            ",
            &["impl IMPL_BLOCK FileId(1) [12; 23)"],
        );
    }

    #[test]
    fn goto_implementation_works_multiple_blocks() {
        check_goto(
            "
            //- /lib.rs
            struct Foo<|>;
            impl Foo {}
            impl Foo {}
            ",
            &["impl IMPL_BLOCK FileId(1) [12; 23)", "impl IMPL_BLOCK FileId(1) [24; 35)"],
        );
    }

    #[test]
    fn goto_implementation_works_multiple_mods() {
        check_goto(
            "
            //- /lib.rs
            struct Foo<|>;
            mod a {
                impl super::Foo {}
            }
            mod b {
                impl super::Foo {}
            }
            ",
            &["impl IMPL_BLOCK FileId(1) [24; 42)", "impl IMPL_BLOCK FileId(1) [57; 75)"],
        );
    }

    #[test]
    fn goto_implementation_works_multiple_files() {
        check_goto(
            "
            //- /lib.rs
            struct Foo<|>;
            mod a;
            mod b;
            //- /a.rs
            impl crate::Foo {}
            //- /b.rs
            impl crate::Foo {}
            ",
            &["impl IMPL_BLOCK FileId(2) [0; 18)", "impl IMPL_BLOCK FileId(3) [0; 18)"],
        );
    }

    #[test]
    fn goto_implementation_for_trait() {
        check_goto(
            "
            //- /lib.rs
            trait T<|> {}
            struct Foo;
            impl T for Foo {}
            ",
            &["impl IMPL_BLOCK FileId(1) [23; 40)"],
        );
    }

    #[test]
    fn goto_implementation_for_trait_multiple_files() {
        check_goto(
            "
            //- /lib.rs
            trait T<|> {};
            struct Foo;
            mod a;
            mod b;
            //- /a.rs
            impl crate::T for crate::Foo {}
            //- /b.rs
            impl crate::T for crate::Foo {}
            ",
            &["impl IMPL_BLOCK FileId(2) [0; 31)", "impl IMPL_BLOCK FileId(3) [0; 31)"],
        );
    }

    #[test]
    fn goto_implementation_all_impls() {
        check_goto(
            "
            //- /lib.rs
            trait T {}
            struct Foo<|>;
            impl Foo {}
            impl T for Foo {}
            impl T for &Foo {}
            ",
            &[
                "impl IMPL_BLOCK FileId(1) [23; 34)",
                "impl IMPL_BLOCK FileId(1) [35; 52)",
                "impl IMPL_BLOCK FileId(1) [53; 71)",
            ],
        );
    }

    #[test]
    fn goto_implementation_to_builtin_derive() {
        check_goto(
            "
            //- /lib.rs
            #[derive(Copy)]
            struct Foo<|>;
            ",
            &["impl IMPL_BLOCK FileId(1) [0; 15)"],
        );
    }
}
