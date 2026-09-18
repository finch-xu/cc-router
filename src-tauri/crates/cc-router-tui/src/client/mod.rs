pub mod discovery;
pub mod http;
pub mod sse;

pub use http::{Client, ClientError, EventStream};
