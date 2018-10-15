#[macro_use]
extern crate failure;
#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate serde_json;
extern crate languageserver_types;
#[macro_use]
extern crate crossbeam_channel;
extern crate rayon;
#[macro_use]
extern crate log;
extern crate drop_bomb;
extern crate url_serde;
extern crate walkdir;
extern crate im;
extern crate relative_path;
extern crate cargo_metadata;
extern crate rustc_hash;

extern crate gen_lsp_server;
extern crate ra_editor;
extern crate ra_analysis;
extern crate ra_syntax;

mod caps;
pub mod req;
mod conv;
mod main_loop;
mod vfs;
mod path_map;
mod server_world;
mod project_model;
pub mod thread_watcher;

pub type Result<T> = ::std::result::Result<T, ::failure::Error>;
pub use crate::{
    main_loop::main_loop,
    caps::server_capabilities,
};
