//! AWS Event Stream 二进制流解码器.
//!
//! AWS Event Stream 是 AWS 自定义的帧格式, 用于 CodeWhisperer / S3 Select / Transcribe 等服务的
//! 流式响应. cc-router 接 Kiro 时上游 (q.{region}.amazonaws.com/generateAssistantResponse)
//! 用这个格式 streaming 返回, 必须先解出 message 才能继续走 Anthropic SSE 翻译.
//!
//! ## 帧格式
//!
//! ```text
//! +---------------------+
//! | Prelude (12 bytes)  |
//! |   total_length: u32 BE  (整条 message 总长, 含自身 + headers + payload + message_crc)
//! |   headers_length: u32 BE
//! |   prelude_crc: u32 BE   (校验前 8 字节)
//! +---------------------+
//! | Headers (headers_length bytes)
//! |   循环:
//! |     name_len: u8
//! |     name: UTF-8 (name_len bytes)
//! |     value_type: u8  (0..9, 见 HeaderValue::parse)
//! |     value: 按 type 解析
//! +---------------------+
//! | Payload (total_length - headers_length - 16 bytes)
//! +---------------------+
//! | Message CRC: u32 BE (校验前 total_length-4 字节)
//! +---------------------+
//! ```
//!
//! ## 使用
//!
//! ```ignore
//! let mut decoder = EventStreamDecoder::new();
//! decoder.feed(&chunk_bytes);
//! while let Some(frame) = decoder.try_pop_frame()? {
//!     let event_type = frame.header_str(":event-type");
//!     // ... 按 event_type 分发
//! }
//! ```
//!
//! 状态机设计: 内部维护单 buffer (Vec<u8>), 每次 `feed` 追加, `try_pop_frame` 在 buffer 头部
//! 尝试解出一帧. 解出后 drain buffer 头部已消费字节. 不依赖 async, 把流式 buffer 管理留给调用方.

use std::collections::HashMap;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventStreamError {
    #[error("event stream prelude CRC mismatch: expected {expected:08x}, got {actual:08x}")]
    PreludeCrcMismatch { expected: u32, actual: u32 },
    #[error("event stream message CRC mismatch: expected {expected:08x}, got {actual:08x}")]
    MessageCrcMismatch { expected: u32, actual: u32 },
    #[error("event stream invalid header value type: {0}")]
    UnknownHeaderType(u8),
    #[error("event stream invalid header: {0}")]
    InvalidHeader(&'static str),
    #[error("event stream prelude total_length {0} smaller than minimum 16 bytes")]
    PreludeTooSmall(u32),
}

/// AWS Event Stream header 值. cc-router 在 Kiro 场景下主要遇到 String (type 7),
/// 但完整解析所有 type 以便兼容 (类似 timestamp 在 S3 Select 中会用).
#[derive(Debug, Clone, PartialEq)]
pub enum HeaderValue {
    BoolTrue,
    BoolFalse,
    Byte(i8),
    Short(i16),
    Integer(i32),
    Long(i64),
    /// 任意字节 (type 6)
    ByteArray(Vec<u8>),
    /// UTF-8 字符串 (type 7)
    String(String),
    /// 毫秒 (type 8)
    Timestamp(i64),
    /// 16 字节 UUID (type 9)
    Uuid([u8; 16]),
}

impl HeaderValue {
    pub fn as_str(&self) -> Option<&str> {
        if let Self::String(s) = self {
            Some(s.as_str())
        } else {
            None
        }
    }
}

/// 解码出的单个 message. payload 是原始字节, 调用方按 :content-type 自行 JSON parse.
#[derive(Debug, Clone)]
pub struct EventStreamFrame {
    pub headers: HashMap<String, HeaderValue>,
    pub payload: Vec<u8>,
}

impl EventStreamFrame {
    /// 取 string 类型的 header 值. 找不到或非 string 返回 None.
    pub fn header_str(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.as_str())
    }

    /// :event-type header, 上游用它区分 assistantResponseEvent / toolUseEvent 等.
    pub fn event_type(&self) -> Option<&str> {
        self.header_str(":event-type")
    }

    /// :message-type header, 上游用 "event" / "exception" / "error" 区分常规事件与错误.
    pub fn message_type(&self) -> Option<&str> {
        self.header_str(":message-type")
    }
}

/// 状态机式 buffer 解码器. 每次 `feed(chunk)` 累积, `try_pop_frame` 在 buffer 头部尝试解出一帧.
pub struct EventStreamDecoder {
    buffer: Vec<u8>,
}

impl EventStreamDecoder {
    pub fn new() -> Self {
        Self { buffer: Vec::with_capacity(8 * 1024) }
    }

    /// 累积更多上游字节. 不触发解码.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// 一次性把 chunk 灌入并把所有完整 frame 解出. 解到一半的 frame 留在内部 buffer.
    pub fn feed_and_drain(&mut self, chunk: &[u8]) -> Result<Vec<EventStreamFrame>, EventStreamError> {
        self.feed(chunk);
        let mut out = Vec::new();
        while let Some(frame) = self.try_pop_frame()? {
            out.push(frame);
        }
        Ok(out)
    }

    /// 尝试从 buffer 头部解一帧. 不足时返回 Ok(None) (等更多字节).
    /// CRC 不一致返回 Err 并把当前 buffer 整体丢弃 (无法恢复对齐).
    pub fn try_pop_frame(&mut self) -> Result<Option<EventStreamFrame>, EventStreamError> {
        if self.buffer.len() < 12 {
            return Ok(None);
        }

        let total_length = u32::from_be_bytes(self.buffer[0..4].try_into().unwrap());
        let headers_length = u32::from_be_bytes(self.buffer[4..8].try_into().unwrap());
        let prelude_crc = u32::from_be_bytes(self.buffer[8..12].try_into().unwrap());

        if total_length < 16 {
            // Prelude (12) + message_crc (4) 是下限, headers 和 payload 可以都为空但 total_length 不能 < 16
            self.buffer.clear();
            return Err(EventStreamError::PreludeTooSmall(total_length));
        }

        // 校验 prelude CRC32 (覆盖前 8 字节)
        let computed_prelude_crc = crc32fast::hash(&self.buffer[0..8]);
        if computed_prelude_crc != prelude_crc {
            self.buffer.clear();
            return Err(EventStreamError::PreludeCrcMismatch {
                expected: prelude_crc,
                actual: computed_prelude_crc,
            });
        }

        let total_len_usize = total_length as usize;
        if self.buffer.len() < total_len_usize {
            // 等更多字节
            return Ok(None);
        }

        // 校验 message CRC32 (覆盖前 total_length-4 字节)
        let message_crc_pos = total_len_usize - 4;
        let computed_msg_crc = crc32fast::hash(&self.buffer[0..message_crc_pos]);
        let stored_msg_crc =
            u32::from_be_bytes(self.buffer[message_crc_pos..total_len_usize].try_into().unwrap());
        if computed_msg_crc != stored_msg_crc {
            self.buffer.clear();
            return Err(EventStreamError::MessageCrcMismatch {
                expected: stored_msg_crc,
                actual: computed_msg_crc,
            });
        }

        let headers_start = 12usize;
        let headers_end = headers_start + headers_length as usize;
        let payload_start = headers_end;
        let payload_end = message_crc_pos;

        let headers = parse_headers(&self.buffer[headers_start..headers_end])?;
        let payload = self.buffer[payload_start..payload_end].to_vec();

        // 消费 buffer 头部 total_length 字节
        self.buffer.drain(0..total_len_usize);

        Ok(Some(EventStreamFrame { headers, payload }))
    }
}

impl Default for EventStreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_headers(bytes: &[u8]) -> Result<HashMap<String, HeaderValue>, EventStreamError> {
    let mut out = HashMap::new();
    let mut i = 0usize;
    while i < bytes.len() {
        // name_len (u8) + name + value_type (u8) + value
        if i + 1 > bytes.len() {
            return Err(EventStreamError::InvalidHeader("expected name length byte"));
        }
        let name_len = bytes[i] as usize;
        i += 1;
        if i + name_len > bytes.len() {
            return Err(EventStreamError::InvalidHeader("header name overruns buffer"));
        }
        let name = std::str::from_utf8(&bytes[i..i + name_len])
            .map_err(|_| EventStreamError::InvalidHeader("header name not utf-8"))?
            .to_string();
        i += name_len;
        if i + 1 > bytes.len() {
            return Err(EventStreamError::InvalidHeader("expected value type byte"));
        }
        let value_type = bytes[i];
        i += 1;
        let value = match value_type {
            0 => HeaderValue::BoolTrue,
            1 => HeaderValue::BoolFalse,
            2 => {
                if i + 1 > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("byte header overruns"));
                }
                let v = bytes[i] as i8;
                i += 1;
                HeaderValue::Byte(v)
            }
            3 => {
                if i + 2 > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("short header overruns"));
                }
                let v = i16::from_be_bytes(bytes[i..i + 2].try_into().unwrap());
                i += 2;
                HeaderValue::Short(v)
            }
            4 => {
                if i + 4 > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("int header overruns"));
                }
                let v = i32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
                i += 4;
                HeaderValue::Integer(v)
            }
            5 => {
                if i + 8 > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("long header overruns"));
                }
                let v = i64::from_be_bytes(bytes[i..i + 8].try_into().unwrap());
                i += 8;
                HeaderValue::Long(v)
            }
            6 => {
                if i + 2 > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("bytes header len overruns"));
                }
                let value_len =
                    u16::from_be_bytes(bytes[i..i + 2].try_into().unwrap()) as usize;
                i += 2;
                if i + value_len > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("bytes header value overruns"));
                }
                let v = bytes[i..i + value_len].to_vec();
                i += value_len;
                HeaderValue::ByteArray(v)
            }
            7 => {
                if i + 2 > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("string header len overruns"));
                }
                let value_len =
                    u16::from_be_bytes(bytes[i..i + 2].try_into().unwrap()) as usize;
                i += 2;
                if i + value_len > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("string header value overruns"));
                }
                let v = std::str::from_utf8(&bytes[i..i + value_len])
                    .map_err(|_| EventStreamError::InvalidHeader("string header not utf-8"))?
                    .to_string();
                i += value_len;
                HeaderValue::String(v)
            }
            8 => {
                if i + 8 > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("timestamp header overruns"));
                }
                let v = i64::from_be_bytes(bytes[i..i + 8].try_into().unwrap());
                i += 8;
                HeaderValue::Timestamp(v)
            }
            9 => {
                if i + 16 > bytes.len() {
                    return Err(EventStreamError::InvalidHeader("uuid header overruns"));
                }
                let mut uuid = [0u8; 16];
                uuid.copy_from_slice(&bytes[i..i + 16]);
                i += 16;
                HeaderValue::Uuid(uuid)
            }
            other => return Err(EventStreamError::UnknownHeaderType(other)),
        };
        out.insert(name, value);
    }
    Ok(out)
}

// ============================================================
// 测试辅助 + 单测
// ============================================================

/// 测试辅助: 用给定 headers + payload 构造一个合法 frame (含 CRC), 用于单测.
#[cfg(test)]
pub fn build_frame(headers: &[(&str, HeaderValue)], payload: &[u8]) -> Vec<u8> {
    let mut header_bytes = Vec::new();
    for (name, val) in headers {
        let name_b = name.as_bytes();
        assert!(name_b.len() <= 255, "header name too long");
        header_bytes.push(name_b.len() as u8);
        header_bytes.extend_from_slice(name_b);
        match val {
            HeaderValue::BoolTrue => header_bytes.push(0),
            HeaderValue::BoolFalse => header_bytes.push(1),
            HeaderValue::Byte(v) => {
                header_bytes.push(2);
                header_bytes.push(*v as u8);
            }
            HeaderValue::Short(v) => {
                header_bytes.push(3);
                header_bytes.extend_from_slice(&v.to_be_bytes());
            }
            HeaderValue::Integer(v) => {
                header_bytes.push(4);
                header_bytes.extend_from_slice(&v.to_be_bytes());
            }
            HeaderValue::Long(v) => {
                header_bytes.push(5);
                header_bytes.extend_from_slice(&v.to_be_bytes());
            }
            HeaderValue::ByteArray(v) => {
                header_bytes.push(6);
                header_bytes.extend_from_slice(&(v.len() as u16).to_be_bytes());
                header_bytes.extend_from_slice(v);
            }
            HeaderValue::String(v) => {
                header_bytes.push(7);
                let v_b = v.as_bytes();
                header_bytes.extend_from_slice(&(v_b.len() as u16).to_be_bytes());
                header_bytes.extend_from_slice(v_b);
            }
            HeaderValue::Timestamp(v) => {
                header_bytes.push(8);
                header_bytes.extend_from_slice(&v.to_be_bytes());
            }
            HeaderValue::Uuid(v) => {
                header_bytes.push(9);
                header_bytes.extend_from_slice(v);
            }
        }
    }
    let headers_length = header_bytes.len() as u32;
    let total_length = 12 + headers_length + payload.len() as u32 + 4;

    let mut out = Vec::with_capacity(total_length as usize);
    out.extend_from_slice(&total_length.to_be_bytes());
    out.extend_from_slice(&headers_length.to_be_bytes());
    let prelude_crc = crc32fast::hash(&out[0..8]);
    out.extend_from_slice(&prelude_crc.to_be_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(payload);
    let msg_crc = crc32fast::hash(&out[..]);
    out.extend_from_slice(&msg_crc.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_single_frame_with_text_payload() {
        let frame_bytes = build_frame(
            &[
                (":event-type", HeaderValue::String("assistantResponseEvent".into())),
                (":content-type", HeaderValue::String("application/json".into())),
            ],
            br#"{"content":"hello"}"#,
        );
        let mut decoder = EventStreamDecoder::new();
        let frames = decoder.feed_and_drain(&frame_bytes).expect("decode ok");
        assert_eq!(frames.len(), 1);
        let f = &frames[0];
        assert_eq!(f.event_type(), Some("assistantResponseEvent"));
        assert_eq!(f.header_str(":content-type"), Some("application/json"));
        assert_eq!(f.payload, br#"{"content":"hello"}"#);
    }

    #[test]
    fn rejects_invalid_prelude_crc() {
        let mut frame_bytes = build_frame(
            &[(":event-type", HeaderValue::String("foo".into()))],
            b"x",
        );
        // 篡改 prelude CRC (字节 8..12)
        frame_bytes[8] ^= 0xFF;
        let mut decoder = EventStreamDecoder::new();
        let err = decoder.feed_and_drain(&frame_bytes).unwrap_err();
        assert!(matches!(err, EventStreamError::PreludeCrcMismatch { .. }));
    }

    #[test]
    fn rejects_invalid_message_crc() {
        let mut frame_bytes = build_frame(
            &[(":event-type", HeaderValue::String("foo".into()))],
            b"hello",
        );
        let last = frame_bytes.len() - 1;
        frame_bytes[last] ^= 0xFF;
        let mut decoder = EventStreamDecoder::new();
        let err = decoder.feed_and_drain(&frame_bytes).unwrap_err();
        assert!(matches!(err, EventStreamError::MessageCrcMismatch { .. }));
    }

    #[test]
    fn handles_multi_frame_chunk() {
        let f1 = build_frame(&[(":event-type", HeaderValue::String("a".into()))], b"1");
        let f2 = build_frame(&[(":event-type", HeaderValue::String("b".into()))], b"22");
        let mut combined = Vec::new();
        combined.extend_from_slice(&f1);
        combined.extend_from_slice(&f2);
        let mut decoder = EventStreamDecoder::new();
        let frames = decoder.feed_and_drain(&combined).expect("decode ok");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].event_type(), Some("a"));
        assert_eq!(frames[0].payload, b"1");
        assert_eq!(frames[1].event_type(), Some("b"));
        assert_eq!(frames[1].payload, b"22");
    }

    #[test]
    fn handles_partial_frame_buffering() {
        let f1 = build_frame(
            &[(":event-type", HeaderValue::String("hello".into()))],
            b"world",
        );
        let mut decoder = EventStreamDecoder::new();
        // 切三段喂
        let mid = f1.len() / 3;
        let mid2 = (f1.len() * 2) / 3;
        assert!(decoder.feed_and_drain(&f1[0..mid]).unwrap().is_empty());
        assert!(decoder.feed_and_drain(&f1[mid..mid2]).unwrap().is_empty());
        let frames = decoder.feed_and_drain(&f1[mid2..]).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event_type(), Some("hello"));
        assert_eq!(frames[0].payload, b"world");
    }

    #[test]
    fn decodes_all_header_value_types() {
        let frame_bytes = build_frame(
            &[
                ("bt", HeaderValue::BoolTrue),
                ("bf", HeaderValue::BoolFalse),
                ("byte", HeaderValue::Byte(-7)),
                ("short", HeaderValue::Short(-12345)),
                ("int", HeaderValue::Integer(123456)),
                ("long", HeaderValue::Long(-9_999_999_999)),
                ("bytes", HeaderValue::ByteArray(vec![1, 2, 3])),
                ("str", HeaderValue::String("hi".into())),
                ("ts", HeaderValue::Timestamp(1_700_000_000_000)),
                ("uuid", HeaderValue::Uuid([1; 16])),
            ],
            b"",
        );
        let mut d = EventStreamDecoder::new();
        let f = d.feed_and_drain(&frame_bytes).unwrap().pop().unwrap();
        assert_eq!(f.headers.get("bt"), Some(&HeaderValue::BoolTrue));
        assert_eq!(f.headers.get("byte"), Some(&HeaderValue::Byte(-7)));
        assert_eq!(f.headers.get("short"), Some(&HeaderValue::Short(-12345)));
        assert_eq!(f.headers.get("int"), Some(&HeaderValue::Integer(123456)));
        assert_eq!(f.headers.get("long"), Some(&HeaderValue::Long(-9_999_999_999)));
        assert_eq!(f.header_str("str"), Some("hi"));
        assert_eq!(
            f.headers.get("ts"),
            Some(&HeaderValue::Timestamp(1_700_000_000_000))
        );
    }
}
