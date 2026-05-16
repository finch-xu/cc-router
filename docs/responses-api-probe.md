# OpenAI Responses API 协议实测报告（P0）

> 实测时间: 2026-05-16
> 实测端点: 第三方 new-api 系中转 (域名脱敏), 非 OpenAI 官方
> 测试模型: gpt-5.5 (中转用 `<vendor>/<model>` 前缀命名, 实际请求时被 strip)
> 用途: cc-router 新增「OpenAI Responses API provider」(自定义 + 官方内置) 前置实测

---

## 1. 中转层 vs 官方 OpenAI 行为差异（重要）

实测端点是 **new-api** 系中转 (probe 9 错误体 `type: "new_api_error"` 确认), 不是 OpenAI 官方端点。**部分 quirks 是中转层特有**, 与 `api.openai.com/v1/responses` 可能不同。本报告里**🟢=与官方一致**, **🟡=中转层有偏差, 设计按官方走**, **🔴=阻断设计**。

---

## 2. 11 个事实清单

| # | 探针 | 信号 | 关键发现 | 设计影响 |
|---|------|------|----------|----------|
| 1 | stream=true 简单 text | 🟢 | HTTP 200, Content-Type `text/event-stream`, SSE 事件序列与 codex chatgpt 反代完全一致 (`response.created` → `response.in_progress` → `response.output_item.added` → `response.content_part.added` → `response.output_text.delta` × N → `response.output_text.done` → `response.content_part.done` → `response.output_item.done` → `response.completed`) | `ResponsesSseConverter` 可直接复用, 不需要 fork |
| 2 | stream=false 简单 text | 🟢 | HTTP 200, Content-Type `application/json`, 返完整 JSON `{id, status, model, output: [{type, content, ...}], usage}` | 必须实现 `responses_json_to_anthropic` JSON-to-JSON 翻译, 不能复用 codex 的 NonStreamingCollector |
| 3 | stream=true with tools | 🟢 | tools schema 字段名 `parameters` (与 codex 一致), 中转层自动补 `strict: true` 和 `additionalProperties: false`; function_call SSE 序列: `output_item.added(function_call)` → `function_call_arguments.delta` × N → `function_call_arguments.done` → `output_item.done` | `convert_tool` 抽到 `responses_common` 直接共用 |
| 4 | stream=true reasoning 复杂题 (训练动力学) | 🟡 | **关键颠覆假设**: gpt-5.5 reasoning 流程只有 `output_item.added(reasoning)` + `output_item.done(reasoning)` 各 1 次, **没有 `reasoning_summary_text.delta` 或 `reasoning_summary_part.added/done` 事件**, **`summary` 字段始终是空数组 `[]`**; encrypted_content 在 added 时是部分长度 (~1.7KB), done 时是完整版 (~3.5KB) | SSE 状态机简化: emit `content_block_start(thinking, "")` on added → emit `content_block_delta(signature_delta)` + `content_block_stop` on done; **保留 summary_text.delta 处理代码作为 o1/o3 等可能暴露 summary 的模型 fallback** |
| 5 | multi-turn round 2 回灌 reasoning encrypted_content | 🟢 | HTTP 200, 服务端接受 round 1 拿到的完整 reasoning item (`{id, type:reasoning, encrypted_content, summary:[]}`) 作为 round 2 input items 元素; round 2 输出新的 reasoning item, 推理继续 | 多轮回灌设计**完全可行**; signature 编码方案 `base64url(JSON{v, id, ec})` 推进 |
| 6 | max_output_tokens=50 | 🟡 | 中转层 **silent drop**: HTTP 200, 响应 `max_output_tokens: null`, 实际生成 624 tokens (远超 50), `incomplete_details: null`; **官方 OpenAI 应正常截断, 中转层不支持** | 翻译层仍按官方走: `max_tokens` → `max_output_tokens`; 中转 中转层用户不会得到截断行为 (在 README compatibility_notes 标注) |
| 7 | 不传 instructions 字段 | 🟢 | HTTP 200, output 正常含 message item; 与 codex 反代「强制 instructions present」不同 | `ResponsesTransformConfig::force_instructions_present` codex=true / openai=false |
| 8 | reasoning effort 4 档 (minimal / high) | 🟢 | minimal: reasoning_tokens=0, 服务端响应里 effort 显示为 "none" (gpt-5.5 把 minimal 内部转为 none); high: reasoning_tokens=12; effort 字段在 request body 顶层 `reasoning.effort` | effort 透传到 `body.reasoning.effort`; 默认值由 yaml `default_reasoning_effort` 提供 |
| 9 | 401 错误 (无效 key) | 🟢 | HTTP 401, Content-Type `application/json`, body `{"error":{"code":"","message":"无效的令牌 (request id: ...)","type":"new_api_error"}}` | 错误透传容错: 翻译层 emit `error_response(401, "auth_error", error.message)`; **能识别 `error.type == "new_api_error"` 当作 OpenAI 错误处理 (中转层产物, 但 shape 兼容)** |
| 10 | max_tokens (Anthropic 字段) on Responses endpoint | 🟡 | 中转层 silent drop, HTTP 200 不报错, 实际不截断; **官方 OpenAI 应该 400 拒绝** | 翻译层 **strip max_tokens** (与 codex 一致), 同时映射为 `max_output_tokens` 兼容官方 |
| 11 | stream=true reasoning without `include` | 🟢 | encrypted_content **默认出现**, 即使没传 `include: ["reasoning.encrypted_content"]`; 中转层和 gpt-5.5 默认行为 | 仍传 `include: ["reasoning.encrypted_content"]` 与官方对齐; **expose_reasoning=false 时仍传 store=false 但不传 include** |

---

## 3. 事件清单 (gpt-5.5 实测)

### stream=true SSE 事件

```
event: response.created
event: response.in_progress
event: response.output_item.added       ← item.type: message | function_call | reasoning
event: response.content_part.added       ← (仅 message item)
event: response.output_text.delta        ← (仅 message text)
event: response.output_text.done
event: response.function_call_arguments.delta   ← (仅 function_call)
event: response.function_call_arguments.done
event: response.content_part.done        ← (仅 message item)
event: response.output_item.done
event: response.completed
```

**gpt-5.5 实测中未出现的事件 (其他模型可能出现)**:
- `response.reasoning_summary_part.added`
- `response.reasoning_summary_part.done`
- `response.reasoning_summary_text.delta`
- `response.reasoning_summary_text.done`
- `response.reasoning_text.delta` (历史 API beta 字段)

**奇怪字段**:
- `output_text.delta` 含 `obfuscation: "<random string>"` —— OpenAI 风控混淆字段, 翻译层 ignore
- `output_item` 含 `phase: "final_answer"` —— 中转层加的元数据, 翻译层 ignore

### stream=false JSON shape

```json
{
  "id": "resp_xxx",
  "object": "response",
  "status": "completed",
  "model": "gpt-5.5",
  "output": [
    {"id": "rs_xxx", "type": "reasoning", "encrypted_content": "gAAAAA...", "summary": []},
    {"id": "msg_xxx", "type": "message", "status": "completed",
     "content": [{"type": "output_text", "annotations": [], "logprobs": [], "text": "..."}],
     "phase": "final_answer", "role": "assistant"}
  ],
  "usage": {
    "input_tokens": 12,
    "input_tokens_details": {"cached_tokens": 0},
    "output_tokens": 6,
    "output_tokens_details": {"reasoning_tokens": 0},
    "total_tokens": 18
  },
  "reasoning": {"effort": "medium", "summary": null},
  "store": false,
  "tool_choice": "auto",
  "tools": [],
  ...
}
```

---

## 4. signature 编码方案（推荐）

基于 probe 4/5 实测, encrypted_content 长度量级 ~3.5KB (medium effort 简单数学题), 高 effort 复杂任务预估 10-20KB。

**推荐方案** (P2 落地):

```javascript
signature_v1 = base64url(JSON.stringify({
  v: 1,
  id: "rs_xxx",           // OpenAI 返回的 reasoning item id (必带, OpenAI input items 要求)
  ec: "<encrypted_content>"  // 原样字节串
}))
```

**Claude Code signature 上限实测 (待完成)**: P2 实施前需用 cc-router dev 模式喂一个 20KB signature 进 Claude Code 多轮, 确认未被截断。如截断, 改 fallback 为 dispatch 层 in-memory LRU cache (per `conversation_id`, TTL 30min)。

---

## 5. P2 SSE 状态机调整 (基于 probe 4 实测)

**原计划**:
```
[OpenAI SSE]                                  [Anthropic SSE]
response.output_item.added(reasoning)      →  content_block_start { type:"thinking", thinking:"" }
response.reasoning_summary_text.delta(d)   →  content_block_delta { thinking_delta: d }
response.reasoning_summary_text.done       →  (skip)
response.output_item.done(reasoning)       →  content_block_delta { signature_delta: <encoded> }
                                              + content_block_stop
```

**实测调整后**:
```
[OpenAI SSE (gpt-5.5 实测)]                          [Anthropic SSE]
response.output_item.added(reasoning, summary=[])  →  content_block_start { type:"thinking", thinking:"" }
                                                      (注: encrypted_content 在 added 时是部分长度, ignore)
response.output_item.done(reasoning, summary=...)  →  if summary 非空:
                                                        emit content_block_delta { thinking_delta: <summary text> }
                                                      emit content_block_delta { signature_delta: <encoded(ec from done event)> }
                                                      emit content_block_stop
```

**保留代码**: `reasoning_summary_text.delta` 和 `reasoning_summary_part.added/done` 的处理逻辑仍然写, 作为 o1 / o3 / 未来其他模型可能暴露 summary 的兼容路径。

---

## 6. ResponsesTransformConfig 实测对齐表

| 配置项 | codex (chatgpt 反代) | openai (官方/中转) | 实测依据 |
|--------|----------------------|----------------------|----------|
| `force_upstream_streaming` | true | false | probe 2 验证 stream=false 上游返 JSON, 不需要强制 SSE |
| `force_store_false` | true | true | probe 默认 store=false (gpt-5.5 默认), 与 cc-router 不持久化语义一致 |
| `default_include` | `["reasoning.encrypted_content"]` | `["reasoning.encrypted_content"]` (当 expose_reasoning=true) / `[]` | probe 11 显示 include 冗余但不报错; 与官方对齐 |
| `force_instructions_present` | true | false | probe 7 不传 instructions 200 OK |
| `drop_max_tokens` | true | false (映射为 max_output_tokens) | probe 6, 10 |
| `emit_reasoning` | false (默认) | true (默认) | probe 4 reasoning 事件实测 |
| `roundtrip_reasoning` | false (默认, 与 emit_reasoning 同步) | true (默认) | probe 5 多轮回灌 200 OK |

---

## 7. 设计决策影响表

| 设计点 | 原假设 | 实测结果 | 是否调整 |
|--------|--------|----------|----------|
| 翻译层需要 fork SSE converter | 可能 | 不需要, 序列一致 | ✅ 维持共享 helper |
| stream=false 走 JSON-to-JSON 还是 SSE collect | JSON-to-JSON | JSON-to-JSON | ✅ 维持 P1 计划 |
| reasoning SSE summary delta 处理 | 有 delta 事件 | gpt-5.5 没有 | ⚠️ 简化主路径, 保留 delta 处理作为 fallback |
| encrypted_content 在 SSE 出现时机 | output_item.done | added 部分长度 + done 完整长度 | ✅ 只用 done 时的 |
| max_tokens 处理 | 映射 max_output_tokens | 同 (中转 中转吞两个) | ✅ 仍按官方走 |
| instructions 强制注入 | openai 路径 false | openai 路径 false | ✅ |
| include 字段 | 必传 | 冗余但官方对齐 | ✅ expose_reasoning=true 时传 |
| 多轮回灌可行性 | 不确定 | ✅ 200 OK | ✅ 推进 P2 |
| signature 上限 | 4KB-64KB 区间 | encrypted_content ~3.5KB (medium 简单题) | 🟡 P2 实施前用 cc-router 实测 Claude Code 透传 |

---

## 8. Fixture 清单

存放路径: `src-tauri/tests/fixtures/openai_responses/` (本地保留, `.gitignore` 已排除)

| 文件 | 内容 | 用途 |
|------|------|------|
| `probe1_stream_true_text.sse` | stream=true 简单 text 完整 SSE | P3 e2e 流式 text 测试 |
| `probe2_stream_false_text.json` | stream=false JSON 含 reasoning + message | P3 e2e 非流式测试 + 验证 reasoning_json_to_anthropic |
| `probe3_stream_true_tools.sse` | stream=true with tools SSE | P3 e2e tool_use 测试 |
| `probe4_stream_true_reasoning.sse` | stream=true 复杂题 reasoning SSE (56KB) | P2 reasoning SSE 状态机黄金路径测试 |
| `probe4_reasoning_item.json` | probe 4 抽出来的 reasoning item (1.4KB) | P2 anthropic_messages_to_input 多轮回灌测试 |
| `probe5_request.json` + `probe5_multiturn_reasoning.sse` | 多轮回灌成功 round 2 SSE | P2 e2e 多轮回灌测试 |
| `probe6_max_output_tokens.json` | max_output_tokens 被中转吞掉的反例 | 中转层兼容性记录 |
| `probe7_no_instructions.json` | 不传 instructions 200 OK | P3 instructions 可选回归测试 |
| `probe8a_effort_minimal.json` / `probe8b_effort_high.json` | effort 4 档差异 | P3 effort 透传测试 |
| `probe9_401.json` | 401 错误体 | P3 401 透传测试 |
| `probe10_max_tokens.json` | max_tokens (Anthropic 字段) 被吞 | 中转层兼容性记录 |
| `probe11_stream_reasoning_no_include.sse` | 不传 include 仍有 encrypted_content | 中转层默认行为记录 |

---

## 9. 退出条件（P0 Gate）

- [x] 11 个事实全部记录, 无 🔴 阻断信号
- [x] SSE 状态机 (probe 4) 调整方案明确 (Section 5)
- [x] 多轮回灌可行性 (probe 5) ✅ HTTP 200
- [x] Fixture 全部落到 `src-tauri/tests/fixtures/openai_responses/`
- [ ] **Claude Code signature 上限实测** —— 留到 P2 实施前的最后 gate, 当前推荐用 base64url JSON 内嵌方案

---

## 10. 中转层 quirks 备忘 (给 README compatibility 表用)

| 中转层行为 | 影响 | README 表达 |
|------------|------|-------------|
| `max_output_tokens` / `max_tokens` 都 silent drop, 不截断 | 用户无法控制最大输出长度 | "max_tokens 仅官方 OpenAI 生效, 中转层未实现" |
| `obfuscation` 字段加在 delta 里 | 翻译层 ignore | 无 |
| `phase` 字段加在 output item 里 | 翻译层 ignore | 无 |
| `prompt_cache_key` 自动生成 | 安全但与官方不同 | 无 |
| error.type 是 `new_api_error` 而非 `invalid_request_error` | 错误分类需识别两者 | 无 (内部处理) |
| reasoning encrypted_content 默认开 (不需 include) | 不强制开始 include | "支持 reasoning 多轮回灌" |
| service_tier 默认 "auto"/"default" | 与官方一致 | 无 |
