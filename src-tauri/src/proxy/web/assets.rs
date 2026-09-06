//! /ui 静态资源. 生产模式从 Tauri 打进二进制的 frontendDist 读 (asset_resolver),
//! dev 模式 asset_resolver 回退读 ../dist 目录 (需先 pnpm build).
//!
//! 前端在 web 模式用 HashRouter, pathname 永远是 /ui/, 所以 index.html 回落只是保险.
//! HTML 响应带独立 CSP: 内联脚本 (index.html 的主题预设) 用 sha256 hash 放行,
//! 不用 'unsafe-inline'.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

use crate::state::AppState;

/// 与 tauri.conf.json 的 CSP 同源, 去掉 ipc / asset 协议项 (浏览器里没有这些).
/// script-src 单独拼 (含内联脚本 hash).
const CSP_BASE: &str = "default-src 'self'; \
connect-src 'self' data: https://github.com https://*.githubusercontent.com https://d.cc-router.catonthe.top; \
img-src 'self' data: blob:; \
style-src 'self' 'unsafe-inline'; font-src 'self' data: https://cdn.jsdelivr.net https://registry.npmmirror.com; \
object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

/// 归一化 /ui/ 之后的相对路径. 空 → index.html; 含 `..` 段一律回落 index.html.
pub fn normalize_rel(raw: &str) -> String {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() || trimmed.split('/').any(|seg| seg == "..") {
        return "index.html".to_string();
    }
    trimmed.to_string()
}

pub fn cache_control_for(rel: &str, is_html: bool) -> &'static str {
    if !is_html && rel.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

/// 提取所有不带 src 属性的 <script> 内联体, 返回 base64(sha256(body)) 列表.
/// 只做最小扫描 (index.html 由 Vite 生成, 结构可控), 不引 HTML parser.
pub fn inline_script_hashes(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(start) = html[cursor..].find("<script") {
        let open_at = cursor + start;
        let Some(gt) = html[open_at..].find('>') else { break };
        let open_tag = &html[open_at..open_at + gt + 1];
        let body_start = open_at + gt + 1;
        let Some(close) = html[body_start..].find("</script>") else { break };
        let body = &html[body_start..body_start + close];
        if !open_tag.contains("src=") {
            out.push(STANDARD.encode(Sha256::digest(body.as_bytes())));
        }
        cursor = body_start + close + "</script>".len();
    }
    out
}

pub fn csp_for_html(html: &str) -> String {
    let mut script = String::from("script-src 'self'");
    for h in inline_script_hashes(html) {
        script.push_str(&format!(" 'sha256-{h}'"));
    }
    format!("{CSP_BASE}; {script}")
}

/// GET /ui → 308 /ui/. 相对资源路径 (Vite base "./") 依赖尾斜杠.
pub async fn redirect_root() -> Redirect {
    Redirect::permanent("/ui/")
}

pub async fn serve_index(State(state): State<AppState>) -> Response {
    serve(&state, "index.html")
}

pub async fn serve_path(State(state): State<AppState>, Path(rel): Path<String>) -> Response {
    let rel = normalize_rel(&rel);
    serve(&state, &rel)
}

fn serve(state: &AppState, rel: &str) -> Response {
    let resolver = state.app_handle.asset_resolver();
    // 生产模式 resolver 自带 index.html 回落; dev 模式直读磁盘无回落, 这里补一次.
    let asset = resolver
        .get(rel.to_string())
        .or_else(|| resolver.get("index.html".to_string()));
    let Some(asset) = asset else {
        return (StatusCode::NOT_FOUND, "web ui assets not found (dev: run `pnpm build` first)")
            .into_response();
    };
    let is_html = asset.mime_type.starts_with("text/html");
    let mut resp = Response::new(Body::from(asset.bytes.clone()));
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&asset.mime_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control_for(rel, is_html)),
    );
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if is_html {
        let html = String::from_utf8_lossy(&asset.bytes);
        if let Ok(v) = HeaderValue::from_str(&csp_for_html(&html)) {
            headers.insert(header::CONTENT_SECURITY_POLICY, v);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rel_strips_slashes_and_defaults_index() {
        assert_eq!(normalize_rel(""), "index.html");
        assert_eq!(normalize_rel("/"), "index.html");
        assert_eq!(normalize_rel("assets/app.js"), "assets/app.js");
        assert_eq!(normalize_rel("/assets/app.js"), "assets/app.js");
        assert_eq!(normalize_rel("../secret"), "index.html", "路径穿越一律回落 index");
    }

    #[test]
    fn cache_control_immutable_only_for_hashed_assets() {
        assert_eq!(
            cache_control_for("assets/index-abc123.js", false),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control_for("index.html", true), "no-cache");
        assert_eq!(cache_control_for("assets/whatever.html", true), "no-cache");
        assert_eq!(cache_control_for("logo.png", false), "no-cache");
    }

    #[test]
    fn inline_script_hash_matches_known_vector() {
        // sha256("alert(1)") base64 = 5jFwqAoc..., 用 sha2 现算做对照, 锁的是提取逻辑
        let html = r#"<html><head><script>alert(1)</script><script src="/a.js"></script><script type="module" src="./b.js"></script></head></html>"#;
        let hashes = inline_script_hashes(html);
        assert_eq!(hashes.len(), 1, "带 src 的不算内联");
        let want = STANDARD.encode(Sha256::digest(b"alert(1)"));
        assert_eq!(hashes[0], want);
    }

    #[test]
    fn inline_script_hash_preserves_whitespace() {
        let html = "<script>\n  var a = 1;\n</script>";
        let hashes = inline_script_hashes(html);
        let want = STANDARD.encode(Sha256::digest(b"\n  var a = 1;\n"));
        assert_eq!(hashes, vec![want]);
    }

    #[test]
    fn csp_contains_self_and_hash_and_no_ipc() {
        let csp = csp_for_html("<script>x()</script>");
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("connect-src 'self' data:"));
        assert!(csp.contains("https://d.cc-router.catonthe.top"));
        assert!(csp.contains("script-src 'self' 'sha256-"));
        assert!(!csp.contains("ipc:"));
        assert!(!csp.contains("asset:"));
        assert!(csp.contains("https://cdn.jsdelivr.net"), "小票字体 CDN 保留");
    }
}
