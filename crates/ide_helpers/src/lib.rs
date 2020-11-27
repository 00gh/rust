use hir::{Crate, Enum, Module, ScopeDef, Semantics, Trait};
use ide_db::RootDatabase;
use syntax::ast::{self, make};

pub mod insert_use;

pub fn mod_path_to_ast(path: &hir::ModPath) -> ast::Path {
    let _p = profile::span("mod_path_to_ast");
    
    let mut segments = Vec::new();
    let mut is_abs = false;
    match path.kind {
        hir::PathKind::Plain => {}
        hir::PathKind::Super(0) => segments.push(make::path_segment_self()),
        hir::PathKind::Super(n) => segments.extend((0..n).map(|_| make::path_segment_super())),
        hir::PathKind::DollarCrate(_) | hir::PathKind::Crate => {
            segments.push(make::path_segment_crate())
        }
        hir::PathKind::Abs => is_abs = true,
    }

    segments.extend(
        path.segments
            .iter()
            .map(|segment| make::path_segment(make::name_ref(&segment.to_string()))),
    );
    make::path_from_segments(segments, is_abs)
}

/// Helps with finding well-know things inside the standard library. This is
/// somewhat similar to the known paths infra inside hir, but it different; We
/// want to make sure that IDE specific paths don't become interesting inside
/// the compiler itself as well.
pub struct FamousDefs<'a, 'b>(pub &'a Semantics<'b, RootDatabase>, pub Option<Crate>);

#[allow(non_snake_case)]
impl FamousDefs<'_, '_> {
    pub const FIXTURE: &'static str = r#"//- /libcore.rs crate:core
pub mod convert {
    pub trait From<T> {
        fn from(t: T) -> Self;
    }
}

pub mod default {
    pub trait Default {
       fn default() -> Self;
    }
}

pub mod iter {
    pub use self::traits::{collect::IntoIterator, iterator::Iterator};
    mod traits {
        pub(crate) mod iterator {
            use crate::option::Option;
            pub trait Iterator {
                type Item;
                fn next(&mut self) -> Option<Self::Item>;
                fn by_ref(&mut self) -> &mut Self {
                    self
                }
                fn take(self, n: usize) -> crate::iter::Take<Self> {
                    crate::iter::Take { inner: self }
                }
            }

            impl<I: Iterator> Iterator for &mut I {
                type Item = I::Item;
                fn next(&mut self) -> Option<I::Item> {
                    (**self).next()
                }
            }
        }
        pub(crate) mod collect {
            pub trait IntoIterator {
                type Item;
            }
        }
    }

    pub use self::sources::*;
    pub(crate) mod sources {
        use super::Iterator;
        use crate::option::Option::{self, *};
        pub struct Repeat<A> {
            element: A,
        }

        pub fn repeat<T>(elt: T) -> Repeat<T> {
            Repeat { element: elt }
        }

        impl<A> Iterator for Repeat<A> {
            type Item = A;

            fn next(&mut self) -> Option<A> {
                None
            }
        }
    }

    pub use self::adapters::*;
    pub(crate) mod adapters {
        use super::Iterator;
        use crate::option::Option::{self, *};
        pub struct Take<I> { pub(crate) inner: I }
        impl<I> Iterator for Take<I> where I: Iterator {
            type Item = <I as Iterator>::Item;
            fn next(&mut self) -> Option<<I as Iterator>::Item> {
                None
            }
        }
    }
}

pub mod option {
    pub enum Option<T> { None, Some(T)}
}

pub mod prelude {
    pub use crate::{convert::From, iter::{IntoIterator, Iterator}, option::Option::{self, *}, default::Default};
}
#[prelude_import]
pub use prelude::*;
"#;

    pub fn core(&self) -> Option<Crate> {
        self.find_crate("core")
    }

    pub fn core_convert_From(&self) -> Option<Trait> {
        self.find_trait("core:convert:From")
    }

    pub fn core_option_Option(&self) -> Option<Enum> {
        self.find_enum("core:option:Option")
    }

    pub fn core_default_Default(&self) -> Option<Trait> {
        self.find_trait("core:default:Default")
    }

    pub fn core_iter_Iterator(&self) -> Option<Trait> {
        self.find_trait("core:iter:traits:iterator:Iterator")
    }

    pub fn core_iter(&self) -> Option<Module> {
        self.find_module("core:iter")
    }

    fn find_trait(&self, path: &str) -> Option<Trait> {
        match self.find_def(path)? {
            hir::ScopeDef::ModuleDef(hir::ModuleDef::Trait(it)) => Some(it),
            _ => None,
        }
    }

    fn find_enum(&self, path: &str) -> Option<Enum> {
        match self.find_def(path)? {
            hir::ScopeDef::ModuleDef(hir::ModuleDef::Adt(hir::Adt::Enum(it))) => Some(it),
            _ => None,
        }
    }

    fn find_module(&self, path: &str) -> Option<Module> {
        match self.find_def(path)? {
            hir::ScopeDef::ModuleDef(hir::ModuleDef::Module(it)) => Some(it),
            _ => None,
        }
    }

    fn find_crate(&self, name: &str) -> Option<Crate> {
        let krate = self.1?;
        let db = self.0.db;
        let res =
            krate.dependencies(db).into_iter().find(|dep| dep.name.to_string() == name)?.krate;
        Some(res)
    }

    fn find_def(&self, path: &str) -> Option<ScopeDef> {
        let db = self.0.db;
        let mut path = path.split(':');
        let trait_ = path.next_back()?;
        let std_crate = path.next()?;
        let std_crate = self.find_crate(std_crate)?;
        let mut module = std_crate.root_module(db);
        for segment in path {
            module = module.children(db).find_map(|child| {
                let name = child.name(db)?;
                if name.to_string() == segment {
                    Some(child)
                } else {
                    None
                }
            })?;
        }
        let def =
            module.scope(db, None).into_iter().find(|(name, _def)| name.to_string() == trait_)?.1;
        Some(def)
    }
}
