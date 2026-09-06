//! 网页管理界面 (/ui). 与代理 API 共用端口, 路径前缀分流.
//!
//! 子路由必须在 server.rs 的 auth_layer / cors_layer 之后 merge, 让 /ui 子树
//! 绕开代理 token 校验与 `Access-Control-Allow-Origin: *`——带凭据的管理 API
//! 绝不能配通配 CORS. 详见 docs/superpowers/specs/2026-09-06-web-ui-runtime-bridge-design.md

pub mod auth;
