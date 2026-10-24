//! One module per input format. Each returns an [`crate::ImportPlan`].

pub(crate) mod bruno;
pub(crate) mod curl;
pub(crate) mod har;
pub(crate) mod http_file;
pub(crate) mod hurl;
pub(crate) mod insomnia;
pub(crate) mod openapi;
pub(crate) mod opencollection;
pub(crate) mod postman;
