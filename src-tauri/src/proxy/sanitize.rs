//! Anthropic 协议透传分支的请求体 sanitize 工具.
//!
//! 多 provider 轮询场景下, 客户端 (Claude Code) 多轮回灌的 `messages[].content` 里可能
//! 携带上一轮某个 cc-router 翻译层 (openai_responses / gemini) 包装过的 thinking block。
//! 这些 block 的 `signature` 是 cc-router 内部格式, 真正的 Anthropic 协议 provider
//! (xiaomi/deepseek/zhipu/anthropic/minimax/moonshot/alibaba 等) 无法识别, 透传会触发上游 400
//! ("thinking/reasoning_content must be passed back to the API")。
//!
//! 本模块负责在 [`pipeline`](super::pipeline) Anthropic 透传分支序列化 body 之前剥离这些
//! foreign thinking blocks, 保留空 signature 或真 Anthropic 原生 signature 的 block。
//!
//! 本模块同时负责订阅槽位级 forced effort 的写入 ([`force_anthropic_effort`]) —— 透传分支
//! 没有协议翻译层, 用户设定的 effort 只能直接改写出站 body。

use serde_json::Value;

use crate::proxy::transform::responses_common::{
    decode_reasoning_signature_any, looks_like_cc_router_signature, DecodedReasoningSignature,
};

/// 剥离 cc-router 自家翻译层包装过的 thinking blocks. 本函数原地修改 body.
///
/// 判定: 任何 `type == "thinking"` 且 signature 能被 [`looks_like_cc_router_signature`] 识别
/// (即 openai_responses 或 gemini 包装) 的 block → drop。空 signature / Anthropic 原生 UUID
/// signature / 其他无法识别 → 保留 (上游各自校验)。
///
/// `unwrap_plaintext`: deepseek 系上游 (与 placeholder 注入同一判定, 见
/// [`should_inject_thinking_placeholder`]) 传 true —— openai_responses **明文变体** (`pt:1`,
/// DeepSeek Responses 方言产生, 文本本体在 thinking 字段) 不剥离, 而是清空 signature 保留文本:
/// deepseek Anthropic 端点实测接受非空 thinking + 空 signature (thinking 开关均 200), 剥掉
/// 等于白丢已捕获的思维链。加密变体信息不可逆、gemini 包装暂不解包 (2026-08 决定), 均仍剥离;
/// 会校验 signature 的上游 (anthropic 官方等) 传 false 维持全剥。
///
/// 返回值: (dropped, unwrapped), (0, 0) 表示没动 body, 调用方可用于日志。
pub fn strip_foreign_thinking_blocks(body: &mut Value, unwrap_plaintext: bool) -> (usize, usize) {
    let Some(msgs) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return (0, 0);
    };
    let mut dropped = 0usize;
    let mut unwrapped = 0usize;
    for msg in msgs.iter_mut() {
        let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        content.retain_mut(|blk| {
            if blk.get("type").and_then(|t| t.as_str()) != Some("thinking") {
                return true;
            }
            let sig = blk.get("signature").and_then(|v| v.as_str()).unwrap_or("");
            if looks_like_cc_router_signature(sig).is_none() {
                return true;
            }
            if unwrap_plaintext {
                let text = blk.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                if !text.is_empty()
                    && matches!(
                        decode_reasoning_signature_any(sig),
                        Some(DecodedReasoningSignature::Plaintext { .. })
                    )
                {
                    blk["signature"] = Value::String(String::new());
                    unwrapped += 1;
                    return true;
                }
            }
            dropped += 1;
            false
        });
    }
    (dropped, unwrapped)
}

/// 给 messages 数组里每个 `role: assistant` 消息检查: 若 content 数组没有任何 thinking
/// content_block, 则在 content[0] 位置插入 placeholder `{type:"thinking", thinking:"", signature:""}`.
///
/// 用于 provider yaml `inject_missing_thinking_placeholder == true` 的兼容子集 provider
/// (当前只有 DeepSeek): DeepSeek 协议要求每个含 tool_use 的 assistant 消息必须有 thinking
/// block 开头, 否则触发 400 "thinking must be passed back to the API"。多 provider 轮询时
/// 由 GLM/anthropic 等不发 thinking 的 provider 生成的 assistant 消息回灌到 DeepSeek 时会
/// 触发该错误, 本函数补 placeholder 后实测 DeepSeek 200 接受。
///
/// 保守策略: 对所有缺 thinking 的 assistant 消息都补 (不区分是否含 tool_use), 因为空 thinking
/// placeholder 对纯 text assistant 也无副作用 (DeepSeek 实测忽略空 thinking)。
///
/// 返回值: 补充 placeholder 的消息数。
pub fn inject_missing_thinking_placeholders(body: &mut serde_json::Value) -> usize {
    use serde_json::json;

    let Some(msgs) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return 0;
    };
    let mut injected = 0usize;
    for msg in msgs.iter_mut() {
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            // assistant 消息可能 content 是 string (纯文本), 这种情况 DeepSeek 内部接受, 不补
            continue;
        };
        let has_thinking = content
            .iter()
            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"));
        if !has_thinking {
            content.insert(
                0,
                json!({ "type": "thinking", "thinking": "", "signature": "" }),
            );
            injected += 1;
        }
    }
    injected
}

/// 上游是否为 DeepSeek: 订阅 URL 或真实模型名 (slot 改写后) 任一含 "deepseek" (大小写不敏感)。
///
/// 跨协议共用的 DeepSeek 识别谓词 —— Anthropic 透传路径 ([`should_inject_thinking_placeholder`])
/// 与 Responses 翻译路径 ([`crate::proxy::transform::openai::detect_responses_dialect`]) 都靠它:
/// custom provider 没有 yaml 可挂配置, 这两个信号是仅有的稳定来源 (官方域 `api.deepseek.com`
/// 命中前者, one-api/new-api 等泛域名中转代理 deepseek 模型时命中后者)。
pub fn is_deepseek_upstream(url: &str, real_model: &str) -> bool {
    url.to_lowercase().contains("deepseek") || real_model.to_lowercase().contains("deepseek")
}

/// Anthropic 透传路径是否需要 [`inject_missing_thinking_placeholders`]。
///
/// - `yaml_flag = Some(_)`: 内置 provider, yaml `inject_missing_thinking_placeholder` 显式值
///   优先 (zhipu 等显式 false 的不受启发式影响)。
/// - `yaml_flag = None`: custom / 未注册 provider, 落 [`is_deepseek_upstream`] 启发式 ——
///   用户把自定义订阅指向 `api.deepseek.com/anthropic` (provider_id='custom') 时, 混合路由下
///   别家 provider 生成的无 thinking 的 tool_use assistant 消息同样会触发 DeepSeek 400,
///   与内置 deepseek 订阅是同一约束。
pub fn should_inject_thinking_placeholder(
    yaml_flag: Option<bool>,
    url: &str,
    real_model: &str,
) -> bool {
    yaml_flag.unwrap_or_else(|| is_deepseek_upstream(url, real_model))
}

/// 把订阅槽位级 forced effort 写进 Anthropic 透传 body —— **双写** `output_config.effort`
/// 与 `thinking.effort`, 两处同值。本函数原地修改 body。
///
/// 双写理由: Anthropic 兼容中转站实现分裂 —— 有的只认 Claude Code 2.1+ 的
/// `output_config.effort` (Anthropic 官方字段位置), 有的只认更早的 `thinking.effort` 写法。
/// 两个字段填同一个值语义无冲突, 上游认哪个都能拿到用户设定。
///
/// 三条规则:
///
/// 1. **`output_config` 缺失时创建**。它是 Anthropic 官方的 effort 字段位置, 也是 CC 2.1+
///    每个请求都会带的字段, 所以创建它对绝大多数上游无风险。这是"强制生效"语义的必要条件 ——
///    只覆盖已存在字段的话, 客户端没发 effort 时用户的设定就静默失效了。
///    *已知风险*: 极少数严格中转不认 `output_config` 会返回 400 "extra inputs are not
///    permitted"; 届时的解法是给 provider yaml 加一个写入位置开关, 而不是放弃强制语义。
///
/// 2. **`thinking` 缺失时不创建**。凭空造 `thinking` 会把客户端没开的思考打开 (改变行为与
///    计费), 而且 `type` 填什么都不安全: `enabled` 按老规范要求同时给 `budget_tokens`,
///    `adaptive` 不认它的上游直接 400。只在父对象已存在时 upsert `effort` 键。
///
/// 3. **`thinking.type == "disabled"` 时整体跳过 (两个字段都不写)**。客户端显式关掉了思考,
///    而 effort 是思考深度控制 —— 此时强制一个档位是语义矛盾; 且 Anthropic 官方端点对
///    "thinking disabled + effort xhigh/max" 是明确的 400。返回 0 让调用方能记日志。
///
/// `thinking.budget_tokens` 保持原样不删: 认 effort 的上游会以 effort 为准 (budget_tokens
/// 在 Opus 4.7 起已弃用), 而只认 budget_tokens 的老中转被删掉后会因 `type=enabled` 缺
/// budget_tokens 而 400。留着是两边都安全的选择。
///
/// 不碰 `extra_body`: 那是 cc-router 内部约定 (只有自家翻译层读), Anthropic 协议上游不认,
/// 往透传 body 里注入未知顶层字段只会增加严格中转 400 的面。
///
/// 返回实际写入的字段数 (0/1/2), 调用方用于日志。
pub fn force_anthropic_effort(body: &mut Value, effort: &str) -> usize {
    use serde_json::json;

    // 规则 3: 客户端显式关掉思考 → 完全不干预。
    let thinking_disabled = body
        .get("thinking")
        .and_then(|t| t.get("type"))
        .and_then(|t| t.as_str())
        == Some("disabled");
    if thinking_disabled {
        return 0;
    }

    let Some(obj) = body.as_object_mut() else {
        return 0;
    };
    let mut written = 0usize;

    // 规则 1: output_config 缺失时创建 (非 object 时也覆盖成 object)。
    let oc = obj
        .entry("output_config")
        .or_insert_with(|| json!({}));
    if !oc.is_object() {
        *oc = json!({});
    }
    if let Some(oc_obj) = oc.as_object_mut() {
        oc_obj.insert("effort".into(), Value::String(effort.to_string()));
        written += 1;
    }

    // 规则 2: thinking 只在已存在且是 object 时 upsert。
    if let Some(thinking) = obj.get_mut("thinking").and_then(|v| v.as_object_mut()) {
        thinking.insert("effort".into(), Value::String(effort.to_string()));
        written += 1;
    }

    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::transform::gemini::encode_gemini_thought_signature;
    use crate::proxy::transform::responses_common::{
        encode_plaintext_reasoning_signature, encode_reasoning_signature,
    };
    use serde_json::json;

    fn body_with_thinking(signature: &str, thinking_text: &str) -> Value {
        json!({
            "model": "claude-3-5-haiku-20241022",
            "messages": [
                {
                    "role": "user",
                    "content": [{ "type": "text", "text": "hi" }]
                },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": thinking_text, "signature": signature },
                        { "type": "text", "text": "response" }
                    ]
                }
            ]
        })
    }

    #[test]
    fn unwraps_plaintext_thinking_when_enabled() {
        // deepseek 系上游 (unwrap_plaintext=true): pt 明文变体解包保文本, signature 清空
        // (deepseek Anthropic 端点实测接受非空 thinking + 空 signature, thinking 开关均 200)
        let sig = encode_plaintext_reasoning_signature("ds-uuid-1");
        let mut body = body_with_thinking(&sig, "deepseek 的真实思考文本");
        let (dropped, unwrapped) = strip_foreign_thinking_blocks(&mut body, true);
        assert_eq!((dropped, unwrapped), (0, 1));
        let content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "deepseek 的真实思考文本");
        assert_eq!(content[0]["signature"], "");
    }

    #[test]
    fn plaintext_thinking_still_dropped_without_unwrap() {
        // 非 deepseek 的 Anthropic 上游 (anthropic 官方等会校验 signature): 明文块仍剥离
        let sig = encode_plaintext_reasoning_signature("ds-uuid-1");
        let mut body = body_with_thinking(&sig, "text");
        let (dropped, unwrapped) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!((dropped, unwrapped), (1, 0));
        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn empty_plaintext_thinking_dropped_even_with_unwrap() {
        // 空文本没有可保留的内容 → 剥离, 交给 placeholder 注入兜底
        let sig = encode_plaintext_reasoning_signature("ds-uuid-1");
        let mut body = body_with_thinking(&sig, "");
        let (dropped, unwrapped) = strip_foreign_thinking_blocks(&mut body, true);
        assert_eq!((dropped, unwrapped), (1, 0));
    }

    #[test]
    fn encrypted_and_gemini_still_dropped_with_unwrap() {
        // 加密变体信息不可逆, gemini 包装暂不解包 (2026-08 决定) — unwrap 开启时也剥离
        let enc = encode_reasoning_signature("rs_1", "EC");
        let gem = encode_gemini_thought_signature("gemini_thought_sig");
        let mut body = json!({
            "model": "deepseek-v4-pro",
            "messages": [{
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "enc text", "signature": enc },
                    { "type": "thinking", "thinking": "gem text", "signature": gem },
                    { "type": "text", "text": "a" }
                ]
            }]
        });
        let (dropped, unwrapped) = strip_foreign_thinking_blocks(&mut body, true);
        assert_eq!((dropped, unwrapped), (2, 0));
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn drops_openai_responses_wrapped() {
        let sig = encode_reasoning_signature("rs_abc", "encrypted_payload");
        let mut body = body_with_thinking(&sig, "let me think");
        let (dropped, _) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!(dropped, 1);
        let assistant_content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 1);
        assert_eq!(assistant_content[0]["type"], "text");
    }

    #[test]
    fn drops_gemini_wrapped() {
        let sig = encode_gemini_thought_signature("some_gemini_thought_sig_base64");
        let mut body = body_with_thinking(&sig, "");
        let (dropped, _) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!(dropped, 1);
        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn keeps_empty_signature() {
        let mut body = body_with_thinking("", "some thinking");
        let (dropped, _) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!(dropped, 0);
        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn keeps_anthropic_native_uuid_signature() {
        let mut body = body_with_thinking("03ea0953-5ece-4386-afea-31404f331c5f", "thought");
        let (dropped, _) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!(dropped, 0);
        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn keeps_random_base64_that_is_not_cc_router_format() {
        // base64url 解码出非 JSON 内容 → 不被识别为 cc-router signature
        let mut body = body_with_thinking("YWJjZGVmZ2hpams", "x");
        let (dropped, _) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn handles_missing_messages_field() {
        let mut body = json!({ "model": "foo" });
        let (dropped, _) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn handles_string_content_messages() {
        // 历史消息 content 可能是 string (Anthropic 协议允许), 不应 panic
        let mut body = json!({
            "model": "foo",
            "messages": [
                { "role": "user", "content": "plain text user message" }
            ]
        });
        let (dropped, _) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn drops_multiple_across_multiple_messages() {
        let sig = encode_reasoning_signature("rs_1", "ec_1");
        let mut body = json!({
            "model": "foo",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "q1" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "", "signature": sig.clone() },
                        { "type": "text", "text": "a1" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "q2" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "", "signature": sig.clone() },
                        { "type": "thinking", "thinking": "", "signature": "" },
                        { "type": "text", "text": "a2" }
                    ]
                }
            ]
        });
        let (dropped, _) = strip_foreign_thinking_blocks(&mut body, false);
        assert_eq!(dropped, 2);
        // 第二条 assistant 应该还剩 thinking(空 sig) + text 两块
        assert_eq!(body["messages"][3]["content"].as_array().unwrap().len(), 2);
    }

    // ============================================================
    // inject_missing_thinking_placeholders 测试 (修 DeepSeek 400)
    // ============================================================

    #[test]
    fn inject_adds_placeholder_to_assistant_missing_thinking() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "q" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "calling tool" },
                        { "type": "tool_use", "id": "t1", "name": "echo", "input": {} }
                    ]
                },
                { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 1);
        let assistant = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(assistant.len(), 3);
        assert_eq!(assistant[0]["type"], "thinking");
        assert_eq!(assistant[0]["thinking"], "");
        assert_eq!(assistant[0]["signature"], "");
        assert_eq!(assistant[1]["type"], "text");
        assert_eq!(assistant[2]["type"], "tool_use");
    }

    #[test]
    fn inject_skips_assistant_with_existing_thinking() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "I'm thinking", "signature": "abc" },
                        { "type": "text", "text": "hi" }
                    ]
                }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 0);
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn inject_ignores_user_messages() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "q" }] }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 0);
        // user 消息不应被加 thinking
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn inject_skips_assistant_with_string_content() {
        // assistant content 可以是 string (Anthropic 协议允许), DeepSeek 内部接受, 不补
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "assistant", "content": "plain text reply" }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 0);
    }

    #[test]
    fn inject_handles_multiple_assistant_messages() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "no thinking 1" }]
                },
                { "role": "user", "content": [{ "type": "text", "text": "q" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "yes", "signature": "" },
                        { "type": "text", "text": "has thinking" }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [{ "type": "tool_use", "id": "t1", "name": "x", "input": {} }]
                }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 2); // msg[0] 和 msg[3] 缺 thinking, msg[2] 不动
        assert_eq!(body["messages"][0]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][2]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][2]["content"][0]["thinking"], "yes"); // 已存在的不被覆盖
        assert_eq!(body["messages"][3]["content"][0]["type"], "thinking");
    }

    #[test]
    fn inject_handles_missing_messages_field() {
        let mut body = json!({ "model": "deepseek-v4-flash" });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 0);
    }

    // ---------- should_inject_thinking_placeholder ----------

    #[test]
    fn placeholder_decision_yaml_flag_wins_over_heuristic() {
        // 内置 provider (yaml 有显式值): 显式值优先, URL/模型无关
        assert!(should_inject_thinking_placeholder(Some(true), "https://x.example.com", "m"));
        assert!(!should_inject_thinking_placeholder(
            Some(false),
            "https://api.deepseek.com/anthropic",
            "deepseek-v4-pro"
        ));
    }

    #[test]
    fn placeholder_decision_custom_provider_falls_back_to_deepseek_heuristic() {
        // custom / 未注册 provider (yaml_flag=None): 按 URL 或真实模型名识别 deepseek
        // (用户场景: 「自定义dp」订阅指向 api.deepseek.com/anthropic, provider_id='custom')
        assert!(should_inject_thinking_placeholder(
            None,
            "https://api.deepseek.com/anthropic/v1/messages",
            "deepseek-v4-pro"
        ));
        // 泛域名中转代理 deepseek 模型: 按模型名命中 (大小写不敏感)
        assert!(should_inject_thinking_placeholder(
            None,
            "https://relay.example.com/v1/messages",
            "DeepSeek-V4-Flash"
        ));
        // 非 deepseek 的自定义订阅: 不注入
        assert!(!should_inject_thinking_placeholder(
            None,
            "https://relay.example.com/v1/messages",
            "glm-5"
        ));
    }

    // ---------- force_anthropic_effort ----------

    /// 「双写」需求的直接断言。
    #[test]
    fn force_effort_dual_writes_when_both_present() {
        let mut body = json!({
            "output_config": {"effort": "low"},
            "thinking": {"type": "adaptive", "effort": "low"},
        });
        assert_eq!(force_anthropic_effort(&mut body, "max"), 2);
        assert_eq!(body["output_config"]["effort"], "max");
        assert_eq!(body["thinking"]["effort"], "max");
    }

    #[test]
    fn force_effort_creates_output_config_when_absent() {
        // 强制语义的必要条件: 客户端没发 effort 时用户设定也要生效。
        let mut body = json!({ "model": "kimi-k3" });
        assert_eq!(force_anthropic_effort(&mut body, "max"), 1);
        assert_eq!(body["output_config"]["effort"], "max");
    }

    #[test]
    fn force_effort_preserves_other_output_config_keys() {
        let mut body = json!({ "output_config": {"format": {"type": "json_schema"}} });
        force_anthropic_effort(&mut body, "high");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    }

    /// 回归哨兵: 凭空造 thinking 会把客户端没开的思考打开 (改变行为与计费)。
    #[test]
    fn force_effort_does_not_create_thinking_when_absent() {
        let mut body = json!({ "model": "kimi-k3" });
        force_anthropic_effort(&mut body, "max");
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn force_effort_keeps_thinking_type_and_budget_tokens() {
        let mut body = json!({ "thinking": {"type": "enabled", "budget_tokens": 8192} });
        force_anthropic_effort(&mut body, "high");
        assert_eq!(body["thinking"]["effort"], "high");
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 8192);
    }

    /// 客户端显式关思考 → 完全不干预 (Anthropic 官方对 disabled + xhigh/max 是 400)。
    #[test]
    fn force_effort_skips_entirely_when_thinking_disabled() {
        let mut body = json!({ "thinking": {"type": "disabled"} });
        assert_eq!(force_anthropic_effort(&mut body, "max"), 0);
        assert!(body.get("output_config").is_none());
        assert!(body["thinking"].get("effort").is_none());
    }

    #[test]
    fn force_effort_never_touches_extra_body() {
        let mut body = json!({
            "extra_body": {"reasoning_effort": "low"},
            "output_config": {},
        });
        force_anthropic_effort(&mut body, "max");
        assert_eq!(body["extra_body"]["reasoning_effort"], "low");
    }

    #[test]
    fn force_effort_leaves_messages_untouched() {
        let mut body = json!({
            "model": "kimi-k3",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "yo"}]},
            ],
        });
        let before = body["messages"].clone();
        force_anthropic_effort(&mut body, "max");
        assert_eq!(body["messages"], before);
    }

    #[test]
    fn force_effort_overwrites_non_object_output_config() {
        // 防御性: 客户端送了个畸形的 output_config, 不 panic 也不静默丢掉 effort。
        let mut body = json!({ "output_config": 42 });
        assert_eq!(force_anthropic_effort(&mut body, "low"), 1);
        assert_eq!(body["output_config"]["effort"], "low");
    }
}
