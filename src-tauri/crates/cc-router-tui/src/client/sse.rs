//! 增量 SSE 解析器。线格式是自家后端 (axum `Sse`) 发的:
//!   事件  `event: <name>\ndata: <json>\n\n`
//!   保活  `: keepalive\n\n`   (注释行, 每 15 秒)
//! 不引第三方 eventsource crate (维护停滞), 这里只实现用得到的子集: event / data / 注释,
//! 忽略 id / retry。按字节缓冲到 `\n` 再解码, 所以 TCP 把一个多字节字符拆到两个 chunk 里也没事。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub name: String,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    name: String,
    data: String,
    has_data: bool,
}

impl SseParser {
    /// 喂入一段字节, 返回其中完整结束 (遇到空行) 的事件。不完整的尾巴留在内部缓冲里。
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.line(&String::from_utf8_lossy(&line), &mut out);
        }
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if self.has_data {
                out.push(SseEvent {
                    name: if self.name.is_empty() { "message".into() } else { std::mem::take(&mut self.name) },
                    data: std::mem::take(&mut self.data),
                });
            }
            self.name.clear();
            self.data.clear();
            self.has_data = false;
            return;
        }
        if line.starts_with(':') {
            return; // 注释 / 保活
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.name = value.to_string(),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            _ => {} // id / retry / 未知字段
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(name: &str, data: &str) -> SseEvent {
        SseEvent { name: name.into(), data: data.into() }
    }

    /// 字节取自 axum 0.8 `response/sse.rs` 的序列化: `event: ..\ndata: ..\n\n`, 保活 `: keepalive\n\n`。
    const WIRE: &[u8] = b": keepalive\n\nevent: subscription_state_changed\ndata: \"0b9f\"\n\nevent: route_attempt_finished\ndata: {\"subscription_id\":\"0b9f\",\"virtual_model\":\"model-sonnet\",\"success\":true}\n\n";

    #[test]
    fn parses_events_and_skips_keepalive() {
        let got = SseParser::default().push(WIRE);
        assert_eq!(
            got,
            vec![
                ev("subscription_state_changed", "\"0b9f\""),
                ev("route_attempt_finished", "{\"subscription_id\":\"0b9f\",\"virtual_model\":\"model-sonnet\",\"success\":true}"),
            ]
        );
    }

    /// 无论 TCP 怎么切, 结果都必须一样 —— 逐字节喂是最狠的切法。
    #[test]
    fn byte_at_a_time_gives_the_same_result() {
        let mut p = SseParser::default();
        let mut got = Vec::new();
        for b in WIRE {
            got.extend(p.push(&[*b]));
        }
        assert_eq!(got, SseParser::default().push(WIRE));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn multibyte_char_split_across_chunks_survives() {
        let wire = "event: e\ndata: \"智谱\"\n\n".as_bytes();
        let cut = wire.iter().position(|b| *b == 0xE6).unwrap() + 1; // 切在「智」的第一个字节之后
        let mut p = SseParser::default();
        let mut got = p.push(&wire[..cut]);
        got.extend(p.push(&wire[cut..]));
        assert_eq!(got, vec![ev("e", "\"智谱\"")]);
    }

    #[test]
    fn multiline_data_is_joined_with_newline_and_crlf_is_tolerated() {
        let got = SseParser::default().push(b"event: e\r\ndata: a\r\ndata: b\r\n\r\n");
        assert_eq!(got, vec![ev("e", "a\nb")]);
    }

    #[test]
    fn event_without_name_defaults_to_message_and_blank_event_is_dropped() {
        let got = SseParser::default().push(b"data: x\n\n\n\nevent: only-name\n\n");
        assert_eq!(got, vec![ev("message", "x")]);
    }

    #[test]
    fn incomplete_tail_is_held_until_completed() {
        let mut p = SseParser::default();
        assert!(p.push(b"event: e\ndata: 1\n").is_empty());
        assert_eq!(p.push(b"\n"), vec![ev("e", "1")]);
    }
}
