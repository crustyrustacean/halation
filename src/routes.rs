// src/routes.rs

// module declarations
pub mod api;
pub mod auth;
pub mod health_check;
pub mod media_serving;
pub mod pages;
pub mod upload;

// re-exports
pub use api::*;
pub use auth::*;
pub use health_check::*;
pub use media_serving::*;
pub use pages::*;
pub use upload::*;
