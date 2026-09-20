// src/routes.rs

// module declarations
pub mod api;
pub mod auth;
pub mod health_check;
pub mod pages;

// re-exports
pub use api::*;
pub use auth::*;
pub use health_check::*;
pub use pages::*;
