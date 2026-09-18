pub mod discovery;
pub mod dto;
pub mod http;
pub mod sse;

pub use http::{Client, ClientError, EventStream};
