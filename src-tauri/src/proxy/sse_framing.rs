//! SSE 帧边界识别 — 各上游 SSE provider 共用.
//!
//! SSE 标准允许 `\n\n` / `\r\n\r\n` / `\r\r` 三种空行作为帧边界. 各家方言:
//! - OpenAI Responses (Codex 反代) / Anthropic / 大多数 Anthropic 兼容代理: LF `\n\n`
//! - Google Gemini AI Studio: CRLF `\r\n\r\n`
//!
//! 本模块同时识别这两种, 取**最早出现**的边界. 返回 `(匹配起始位置, 分隔符字节长度)`,
//! 调用方用 `split_to(idx + sep_len)` 切出完整帧.
//!
//! 注: 字节序列 `\r\n\r\n` (`0D 0A 0D 0A`) 内部**不含**子串 `\n\n` (`0A 0A`) —
//! 两个 `0A` 中间隔着 `0D`. 所以 LF 搜索不会误命中 CRLF 中段, 取最小 idx 是安全的.
//!
//! 历史: Gemini 早期实现只找 `\n\n`, 导致 CRLF 流永远找不到帧边界, 客户端收到 0 字节响应.
//! 修复见 `mydata/gemini-接入总结.md` §5.1.

/// 在 buffer 里找第一个 SSE 帧边界. 返回 `Some((idx, sep_len))`, sep_len 取 2 (LF) 或 4 (CRLF).
pub fn find_sse_frame_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n");
    let lf = buf.windows(2).position(|w| w == b"\n\n");
    match (crlf, lf) {
        (Some(c), Some(l)) if l < c => Some((l, 2)),
        (Some(c), _) => Some((c, 4)),
        (None, Some(l)) => Some((l, 2)),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_only() {
        let buf = b"data: {\"x\":1}\r\n\r\n";
        let (idx, sep_len) = find_sse_frame_boundary(buf).unwrap();
        assert_eq!(sep_len, 4);
        assert_eq!(&buf[..idx], b"data: {\"x\":1}");
    }

    #[test]
    fn lf_only() {
        let buf = b"data: {\"x\":1}\n\n";
        let (idx, sep_len) = find_sse_frame_boundary(buf).unwrap();
        assert_eq!(sep_len, 2);
        assert_eq!(&buf[..idx], b"data: {\"x\":1}");
    }

    #[test]
    fn picks_earliest_when_mixed() {
        // 罕见: 帧 A 用 LF, 帧 B 用 CRLF. 应取 LF 边界 (出现更早).
        let buf = b"a\n\nb\r\n\r\n";
        let (idx, sep_len) = find_sse_frame_boundary(buf).unwrap();
        assert_eq!(sep_len, 2);
        assert_eq!(idx, 1);
    }

    #[test]
    fn none_for_partial_frame() {
        assert!(find_sse_frame_boundary(b"data: {\"x\":1}\r\n").is_none());
        assert!(find_sse_frame_boundary(b"data: {\"x\":1}").is_none());
    }
}
