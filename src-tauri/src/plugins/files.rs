//! 插件文件的纯函数: 路径规则、文件头嗅探、SVG 检查、包哈希、data URL。

use std::collections::BTreeMap;

use base64::Engine;
use sha2::{Digest, Sha256};

/// 相对路径: 不以 / 开头、不含 `.` / `..` 段、每段只含 `[A-Za-z0-9._-]` (与 TS `isSafePath` 一致)。
pub fn is_safe_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 200
        && p.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        })
}

pub fn ext_of(p: &str) -> String {
    p.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// 支持的资源类型 → MIME。不在表里的扩展名一律拒绝。
pub fn mime_of(p: &str) -> Option<&'static str> {
    Some(match ext_of(p).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        // 文本资源 (小票模板 / 样式): 原文交给前端校验, 不转 data URL
        "html" => "text/html",
        "css" => "text/css",
        _ => return None,
    })
}

/// 内容与扩展名是否一致; SVG 另做安全检查。返回给用户看的原因。
pub fn is_text_asset(p: &str) -> bool {
    matches!(ext_of(p).as_str(), "html" | "css")
}

/// 文本资源上限: 模板 / 样式不该很大
pub const MAX_TEXT_BYTES: usize = 256 * 1024;

pub fn check_asset(path: &str, bytes: &[u8]) -> Result<(), String> {
    if is_text_asset(path) {
        if bytes.len() > MAX_TEXT_BYTES {
            return Err(format!("{path}: 超过 256 KB"));
        }
        return std::str::from_utf8(bytes).map(|_| ()).map_err(|_| format!("{path}: 不是 UTF-8 文本"));
    }
    let ok = match ext_of(path).as_str() {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "webp" => bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "woff2" => bytes.starts_with(b"wOF2"),
        "woff" => bytes.starts_with(b"wOFF"),
        "ttf" => bytes.starts_with(&[0, 1, 0, 0]) || bytes.starts_with(b"true"),
        "otf" => bytes.starts_with(b"OTTO"),
        "svg" => return check_svg(bytes).map_err(|why| format!("{path}: {why}")),
        _ => return Err(format!("{path}: 不支持的文件类型")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{path}: 文件内容与扩展名不符"))
    }
}

/// SVG 只当图片用 (CSS 背景里的 SVG 本来就不执行脚本), 但仍拒绝脚本、事件属性与外部引用:
/// 一是纵深防御, 二是插件不允许联网。
fn check_svg(bytes: &[u8]) -> Result<(), &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "SVG 不是 UTF-8 文本")?;
    let lower = text.to_ascii_lowercase();
    if !lower.contains("<svg") {
        return Err("不是 SVG");
    }
    for bad in ["<script", "<foreignobject", "javascript:", "<!entity", "@import"] {
        if lower.contains(bad) {
            return Err("SVG 含有脚本或外部引用");
        }
    }
    // on*= 事件属性: 空白后紧跟 on + 字母 + 可选空白 + =
    let b = lower.as_bytes();
    for i in 1..b.len().saturating_sub(3) {
        if b[i - 1].is_ascii_whitespace() && b[i] == b'o' && b[i + 1] == b'n' && b[i + 2].is_ascii_alphabetic() {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'=' {
                return Err("SVG 含有事件属性");
            }
        }
    }
    // 外部引用: href / url( 后面跟 http(s): 或 //
    for marker in ["href", "url("] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(marker) {
            let rest = lower[from + pos + marker.len()..].trim_start_matches(|c: char| c == '=' || c == '"' || c == '\'' || c.is_whitespace());
            if rest.starts_with("http:") || rest.starts_with("https:") || rest.starts_with("//") {
                return Err("SVG 引用了外部资源");
            }
            from += pos + marker.len();
        }
    }
    Ok(())
}

/// 包哈希 (与 `scripts/lib/plugin-hash.mjs` 同算法, 测试向量 `src/plugins/hash-vectors.json`):
/// 按路径字节序遍历, 依次喂入 路径 + 0x00 + 内容长度 (u64 大端) + 内容, 整体 SHA-256, 小写 hex。
pub fn package_hash(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut h = Sha256::new();
    for (path, content) in files {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update((content.len() as u64).to_be_bytes());
        h.update(content);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn data_url(path: &str, bytes: &[u8]) -> Option<String> {
    let mime = mime_of(path)?;
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths() {
        for p in ["a.png", "assets/paper.png", "a/b/c.woff2", "x-y_z.json"] {
            assert!(is_safe_path(p), "{p}");
        }
        for p in ["", "/etc/passwd", "../x", "a/../b", "a//b", "a/./b", "a\\b", "中文.png", "a b.png", "C:x"] {
            assert!(!is_safe_path(p), "{p:?}");
        }
    }

    #[test]
    fn sniffs_file_headers() {
        assert!(check_asset("a.png", b"\x89PNG\r\n\x1a\n....").is_ok());
        assert!(check_asset("a.png", b"GIF89a").is_err());
        assert!(check_asset("a.woff2", b"wOF2....").is_ok());
        assert!(check_asset("a.woff2", b"<svg>").is_err());
        assert!(check_asset("a.exe", b"MZ").is_err());
        assert!(check_asset("t.html", "<div>小票</div>".as_bytes()).is_ok());
        assert!(check_asset("s.css", b"\xff\xfe").is_err());
        assert!(check_asset("s.css", &vec![b'a'; MAX_TEXT_BYTES + 1]).is_err());
    }

    #[test]
    fn svg_safety() {
        let ok = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path d="M0 0h10" fill="#000"/></svg>"##;
        assert!(check_asset("a.svg", ok).is_ok(), "xmlns 的 http 不是外部引用");
        for bad in [
            &br#"<svg><script>alert(1)</script></svg>"#[..],
            br#"<svg onload="x()"></svg>"#,
            br#"<svg><rect onclick = "x()"/></svg>"#,
            br#"<svg><image href="https://evil.example/a.png"/></svg>"#,
            br#"<svg><image xlink:href='//evil.example/a.png'/></svg>"#,
            br#"<svg><style>rect{fill:url(https://e.example/x)}</style></svg>"#,
            br#"<svg><a href="javascript:x()"/></svg>"#,
            br#"<svg><foreignObject/></svg>"#,
            br#"not an svg"#,
        ] {
            assert!(check_asset("a.svg", bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    /// 与 TS 版共用的测试向量: 两边的哈希必须一致, 否则社区安装永远对不上。
    #[test]
    fn hash_vectors_match_ts() {
        let raw = include_str!("../../../src/plugins/hash-vectors.json");
        let cases: Vec<serde_json::Value> = serde_json::from_str(raw).unwrap();
        assert!(cases.len() >= 3);
        for case in cases {
            let files: BTreeMap<String, Vec<u8>> = case["files"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().as_bytes().to_vec()))
                .collect();
            assert_eq!(package_hash(&files), case["sha256"].as_str().unwrap());
        }
    }

    #[test]
    fn data_urls() {
        assert_eq!(data_url("a.png", b"hi").unwrap(), "data:image/png;base64,aGk=");
        assert!(data_url("a.txt", b"hi").is_none());
    }
}
