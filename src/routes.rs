// src/routes.rs

// module declarations
pub mod api;
pub mod auth;
pub mod feed;
pub mod health_check;
pub mod media_serving;
pub mod pages;
pub mod profile;
pub mod social;
pub mod upload;

// re-exports
pub use api::*;
pub use auth::*;
pub use feed::*;
pub use health_check::*;
pub use media_serving::*;
pub use pages::*;
pub use profile::*;
pub use social::*;
pub use upload::*;
