#!/usr/bin/env python3
"""
独立探测脚本: 测试 deepseek / whatai / xiaomi 三家 Anthropic 兼容端点
在多轮对话 + thinking + tool_use + 跨厂商轮询场景下的兼容性。

用法:
  DEEPSEEK_KEY=... WHATAI_KEY=... XIAOMI_KEY=... \
    python3 scripts/probe_anthropic_compat.py

输出:
  - scripts/probe-out/<scenario>--<provider>.req.json   请求体
  - scripts/probe-out/<scenario>--<provider>.resp.json  响应体或错误体
  - scripts/probe-out/<scenario>--<provider>.meta.json  status + headers
  - scripts/probe-out/<scenario>--<provider>.raw.txt    流式原始字节
  - stdout: markdown 总结
"""
import json
import os
import sys
import time
from pathlib import Path
from typing import Any, Optional

import httpx

OUT_DIR = Path(__file__).resolve().parent / "probe-out"
OUT_DIR.mkdir(exist_ok=True)

PROVIDERS = {
    "deepseek": {
        "url": "https://api.deepseek.com/anthropic/v1/messages",
        "key_env": "DEEPSEEK_KEY",
        "default_model": "deepseek-v4-pro",
    },
    "whatai": {
        "url": "https://api.whatai.cc/v1/messages",
        "key_env": "WHATAI_KEY",
        "default_model": "claude-opus-4-6",
    },
    "xiaomi": {
        "url": "https://api.xiaomimimo.com/anthropic/v1/messages",
        "key_env": "XIAOMI_KEY",
        "default_model": "mimo-v2.5-pro",
    },
}

# 运行期填充：每家选定的 model id（discover_models 之后修正）
SELECTED_MODEL: dict[str, str] = {p: PROVIDERS[p]["default_model"] for p in PROVIDERS}


# ------------- HTTP 基础工具 -------------

def get_key(provider: str) -> str:
    env = PROVIDERS[provider]["key_env"]
    val = os.environ.get(env)
    if not val:
        sys.exit(f"ERROR: 环境变量 {env} 未设置")
    return val


def auth_headers(provider: str) -> dict:
    return {
        "Authorization": f"Bearer {get_key(provider)}",
        "anthropic-version": "2023-06-01",
        "content-type": "application/json",
        "accept": "application/json, text/event-stream",
    }


def write_artifact(case_name: str, provider: str, suffix: str, content: str) -> None:
    path = OUT_DIR / f"{case_name}--{provider}.{suffix}"
    path.write_text(content)


# ------------- SSE 解析 -------------

def parse_sse_event(text: str) -> Optional[dict]:
    event_name = None
    data_lines = []
    for line in text.splitlines():
        if line.startswith("event:"):
            event_name = line[len("event:"):].strip()
        elif line.startswith("data:"):
            data_lines.append(line[len("data:"):].lstrip())
    if not data_lines:
        return None
    try:
        data = json.loads("".join(data_lines))
    except json.JSONDecodeError:
        return None
    return {"event": event_name or data.get("type"), "data": data}


def apply_sse_event(message: dict, blocks: dict, ev: dict) -> None:
    name = ev["event"]
    data = ev["data"]
    if name == "message_start":
        msg = data.get("message", {})
        message["id"] = msg.get("id")
        message["model"] = msg.get("model")
        message["role"] = msg.get("role", "assistant")
        message["usage"] = msg.get("usage")
    elif name == "content_block_start":
        idx = data.get("index", 0)
        block = dict(data.get("content_block", {}))
        if block.get("type") == "thinking":
            block.setdefault("thinking", "")
            block.setdefault("signature", "")
        elif block.get("type") == "text":
            block.setdefault("text", "")
        elif block.get("type") == "tool_use":
            block.setdefault("input", {})
            block["_input_json"] = ""
        blocks[idx] = block
    elif name == "content_block_delta":
        idx = data.get("index", 0)
        if idx not in blocks:
            return
        delta = data.get("delta", {})
        dt = delta.get("type")
        b = blocks[idx]
        if dt == "thinking_delta":
            b["thinking"] = b.get("thinking", "") + delta.get("thinking", "")
        elif dt == "signature_delta":
            b["signature"] = b.get("signature", "") + delta.get("signature", "")
        elif dt == "text_delta":
            b["text"] = b.get("text", "") + delta.get("text", "")
        elif dt == "input_json_delta":
            b["_input_json"] = b.get("_input_json", "") + delta.get("partial_json", "")
    elif name == "content_block_stop":
        idx = data.get("index", 0)
        if idx in blocks:
            b = blocks.pop(idx)
            if b.get("type") == "tool_use":
                ij = b.pop("_input_json", "")
                if ij:
                    try:
                        b["input"] = json.loads(ij)
                    except json.JSONDecodeError:
                        pass
            message["content"].append(b)
    elif name == "message_delta":
        delta = data.get("delta", {})
        if "stop_reason" in delta:
            message["stop_reason"] = delta["stop_reason"]
        if data.get("usage"):
            existing = message.get("usage") or {}
            existing.update(data["usage"])
            message["usage"] = existing


# ------------- 调用上游 -------------

def call_upstream(
    provider: str,
    body: dict,
    case_name: str,
    stream: bool,
) -> tuple[int, Optional[dict], str]:
    """
    返回 (status, parsed_or_None, raw_text)
    parsed: 流式时是聚合后的 final_message; 非流式时是 r.json()
    """
    info = PROVIDERS[provider]
    body_to_send = dict(body)
    if stream:
        body_to_send["stream"] = True
    base = f"{case_name}--{provider}"
    write_artifact(case_name, provider, "req.json",
                   json.dumps(body_to_send, ensure_ascii=False, indent=2))

    headers = auth_headers(provider)

    if not stream:
        try:
            r = httpx.post(info["url"], headers=headers, json=body_to_send, timeout=180)
        except Exception as e:
            write_artifact(case_name, provider, "meta.json",
                           json.dumps({"error": str(e), "type": type(e).__name__}, indent=2))
            return -1, None, ""
        meta = {"status": r.status_code, "headers": dict(r.headers), "stream": False}
        write_artifact(case_name, provider, "meta.json",
                       json.dumps(meta, ensure_ascii=False, indent=2))
        raw = r.text
        write_artifact(case_name, provider, "resp.json", raw)
        try:
            parsed = r.json()
        except Exception:
            parsed = None
        return r.status_code, parsed, raw

    # stream
    final_message: dict = {
        "role": "assistant", "content": [], "id": None, "model": None
    }
    blocks: dict = {}
    raw_chunks: list[str] = []
    try:
        with httpx.stream("POST", info["url"], headers=headers, json=body_to_send,
                          timeout=180) as r:
            status = r.status_code
            meta = {"status": status, "headers": dict(r.headers), "stream": True}
            if status != 200:
                err = r.read().decode(errors="replace")
                write_artifact(case_name, provider, "meta.json",
                               json.dumps(meta, ensure_ascii=False, indent=2))
                write_artifact(case_name, provider, "resp.json", err)
                try:
                    return status, json.loads(err), err
                except json.JSONDecodeError:
                    return status, None, err
            buf = ""
            for chunk in r.iter_text():
                raw_chunks.append(chunk)
                buf += chunk
                while "\n\n" in buf:
                    ev_text, buf = buf.split("\n\n", 1)
                    parsed = parse_sse_event(ev_text)
                    if parsed:
                        apply_sse_event(final_message, blocks, parsed)
        write_artifact(case_name, provider, "meta.json",
                       json.dumps(meta, ensure_ascii=False, indent=2))
        write_artifact(case_name, provider, "raw.txt", "".join(raw_chunks))
        write_artifact(case_name, provider, "resp.json",
                       json.dumps(final_message, ensure_ascii=False, indent=2))
        return status, final_message, "".join(raw_chunks)
    except Exception as e:
        write_artifact(case_name, provider, "meta.json",
                       json.dumps({"error": str(e), "type": type(e).__name__}, indent=2))
        return -1, None, ""


# ------------- 历史构造工具 -------------

def assistant_msg_from_response(final_msg: dict) -> dict:
    """从 streaming final_message 抽出可塞回 messages 数组的 assistant message."""
    content = final_msg.get("content", [])
    return {"role": "assistant", "content": content}


def find_block(content: list, btype: str) -> Optional[dict]:
    for b in content:
        if isinstance(b, dict) and b.get("type") == btype:
            return b
    return None


def summarize_content(content: list) -> str:
    parts = []
    for b in content:
        if not isinstance(b, dict):
            continue
        t = b.get("type")
        if t == "thinking":
            parts.append(f"thinking(len={len(b.get('thinking', ''))},sig={'Y' if b.get('signature') else 'N'})")
        elif t == "text":
            parts.append(f"text(len={len(b.get('text', ''))})")
        elif t == "tool_use":
            parts.append(f"tool_use({b.get('name')})")
        elif t == "tool_result":
            parts.append(f"tool_result(id={b.get('tool_use_id', '?')[:8]})")
        else:
            parts.append(t or "?")
    return "[" + ",".join(parts) + "]"


def short_err(parsed: Optional[dict], raw: str) -> str:
    if parsed is None:
        return raw[:300]
    if isinstance(parsed, dict):
        if "error" in parsed:
            err = parsed["error"]
            if isinstance(err, dict):
                msg = err.get("message", "")
                etype = err.get("type", "")
                code = err.get("code", "")
                return f"{etype}/{code}: {msg}"
            return str(err)
        return json.dumps(parsed, ensure_ascii=False)[:300]
    return str(parsed)[:300]


# ------------- 工具定义（A5/B4 用） -------------

WEATHER_TOOL = {
    "name": "get_weather",
    "description": "Get current weather for a city.",
    "input_schema": {
        "type": "object",
        "properties": {
            "city": {"type": "string", "description": "City name"},
        },
        "required": ["city"],
    },
}


# ------------- 场景实现 -------------

REPORT_ROWS: list[dict] = []  # 每行 {scenario, provider, status, summary}


def record(scenario: str, provider: str, status: int, summary: str) -> None:
    REPORT_ROWS.append({
        "scenario": scenario, "provider": provider,
        "status": status, "summary": summary,
    })
    print(f"  [{scenario}] {provider} → {status} {summary}", flush=True)


def make_body(provider: str, messages: list, stream: bool, max_tokens: int = 1024,
              tools: Optional[list] = None) -> dict:
    body = {
        "model": SELECTED_MODEL[provider],
        "max_tokens": max_tokens,
        "messages": messages,
    }
    if tools:
        body["tools"] = tools
    return body


# ----- A 组 -----

def scenario_A1(provider: str) -> None:
    """A1 单轮 user 非流式"""
    name = "A1-simple-nonstream"
    body = make_body(provider, [{"role": "user", "content": "Reply with 'pong' only."}], stream=False)
    status, parsed, raw = call_upstream(provider, body, name, stream=False)
    if status == 200:
        content = (parsed or {}).get("content", [])
        record(name, provider, status, f"content={summarize_content(content)}")
    else:
        record(name, provider, status, short_err(parsed, raw))


def scenario_A2(provider: str) -> dict:
    """A2 单轮 user 流式 → 返回 final_message 给 A4 复用."""
    name = "A2-simple-stream"
    body = make_body(provider,
                     [{"role": "user", "content": "Think step by step about 7+5, then say the answer."}],
                     stream=True, max_tokens=2048)
    status, parsed, _ = call_upstream(provider, body, name, stream=True)
    if status == 200:
        content = (parsed or {}).get("content", [])
        record(name, provider, status, f"content={summarize_content(content)}")
        return parsed or {}
    record(name, provider, status, short_err(parsed, ""))
    return {}


def scenario_A3(provider: str) -> None:
    """A3 多轮纯文本(无 thinking 历史)"""
    name = "A3-multiturn-text"
    messages = [
        {"role": "user", "content": "Hi! Just say 'hello'."},
        {"role": "assistant", "content": "hello"},
        {"role": "user", "content": "Now say 'bye'."},
    ]
    body = make_body(provider, messages, stream=False)
    status, parsed, raw = call_upstream(provider, body, name, stream=False)
    if status == 200:
        record(name, provider, status,
               f"content={summarize_content((parsed or {}).get('content', []))}")
    else:
        record(name, provider, status, short_err(parsed, raw))


def scenario_A4(provider: str, prior_assistant: dict) -> None:
    """A4 多轮，把 A2 抓到的 thinking 块原样回传."""
    name = "A4-multiturn-thinking"
    if not prior_assistant or not prior_assistant.get("content"):
        record(name, provider, -1, "skipped: no prior assistant message")
        return
    asst_msg = assistant_msg_from_response(prior_assistant)
    messages = [
        {"role": "user", "content": "Think step by step about 7+5, then say the answer."},
        asst_msg,
        {"role": "user", "content": "Now also tell me what 8+9 is, briefly."},
    ]
    body = make_body(provider, messages, stream=False, max_tokens=2048)
    status, parsed, raw = call_upstream(provider, body, name, stream=False)
    if status == 200:
        record(name, provider, status,
               f"content={summarize_content((parsed or {}).get('content', []))}")
    else:
        record(name, provider, status, short_err(parsed, raw))


def scenario_A5(provider: str) -> None:
    """A5 工具调用 + thinking 回传."""
    name = "A5-tool-thinking-roundtrip"
    # round 1: ask weather, expect tool_use
    body1 = make_body(
        provider,
        [{"role": "user", "content": "What's the weather in Tokyo? Use the get_weather tool."}],
        stream=True, tools=[WEATHER_TOOL], max_tokens=2048,
    )
    status1, parsed1, _ = call_upstream(provider, body1, f"{name}-r1", stream=True)
    if status1 != 200 or not parsed1:
        record(name, provider, status1, f"r1 failed: {short_err(parsed1, '')}")
        return
    asst_content = parsed1.get("content", [])
    tool_use = find_block(asst_content, "tool_use")
    if not tool_use:
        record(name, provider, 200,
               f"r1 no tool_use returned, content={summarize_content(asst_content)}")
        return
    # round 2: provide tool_result
    messages = [
        {"role": "user", "content": "What's the weather in Tokyo? Use the get_weather tool."},
        {"role": "assistant", "content": asst_content},
        {"role": "user", "content": [{
            "type": "tool_result",
            "tool_use_id": tool_use.get("id", "?"),
            "content": "Sunny, 22 C in Tokyo right now.",
        }]},
    ]
    body2 = make_body(provider, messages, stream=False, tools=[WEATHER_TOOL], max_tokens=2048)
    status2, parsed2, raw2 = call_upstream(provider, body2, f"{name}-r2", stream=False)
    if status2 == 200:
        record(name, provider, status2,
               f"r2 OK content={summarize_content((parsed2 or {}).get('content', []))}")
    else:
        record(name, provider, status2, f"r2 fail: {short_err(parsed2, raw2)}")


def scenario_A6(provider: str) -> None:
    """A6 控制实验：故意改 thinking → think 看上游报错文案."""
    name = "A6-rename-think-control"
    # 构造一个 *本来* 会带 thinking 块的多轮 body，把 type 与字段都改成 think
    bad_msg = {
        "role": "assistant",
        "content": [
            {"type": "think", "think": "let me think...", "signature": "abc"},
            {"type": "text", "text": "ok"},
        ],
    }
    body = make_body(
        provider,
        [{"role": "user", "content": "hi"}, bad_msg, {"role": "user", "content": "ok continue"}],
        stream=False, max_tokens=512,
    )
    # 同时把顶层也用错误的 think 字段
    body["think"] = {"type": "enabled", "budget_tokens": 256}
    status, parsed, raw = call_upstream(provider, body, name, stream=False)
    record(name, provider, status, short_err(parsed, raw))


# ----- B 组 跨厂商轮询 -----

def run_chain(scenario: str, chain: list[str], use_tools: bool = False) -> None:
    """
    模拟 cc-router round_robin: 每一轮换一家上游, 把上一轮返回原样塞回历史
    """
    print(f"\n=== {scenario} chain={'->'.join(chain)} tools={use_tools} ===", flush=True)
    user_first = "What is 17 plus 25? Think step by step."
    if use_tools:
        user_first = "What's the weather in Paris? Use the tool."
    messages: list[dict] = [{"role": "user", "content": user_first}]
    tools = [WEATHER_TOOL] if use_tools else None
    last_tool_use_id: Optional[str] = None

    for i, provider in enumerate(chain, start=1):
        round_name = f"{scenario}-r{i}"
        # build body: stream first round to grab thinking; non-stream subsequent for clarity
        # actually grab thinking via stream every round (more realistic to cc-router behavior)
        body = make_body(provider, messages, stream=True, tools=tools, max_tokens=2048)
        print(f"  [{round_name}] target={provider} msgs={len(messages)} "
              f"history_summary=" + " | ".join(
                  f"{m['role']}:" + (
                      summarize_content(m['content']) if isinstance(m.get('content'), list)
                      else f"text(len={len(m.get('content', ''))})"
                  )
                  for m in messages
              ), flush=True)
        status, parsed, raw = call_upstream(provider, body, round_name, stream=True)
        if status != 200 or not parsed:
            record(round_name, provider, status, short_err(parsed, raw))
            return
        asst_content = parsed.get("content", [])
        record(round_name, provider, status, f"content={summarize_content(asst_content)}")
        # append assistant message
        messages.append({"role": "assistant", "content": asst_content})
        # find tool_use to follow up
        tu = find_block(asst_content, "tool_use") if use_tools else None
        if tu:
            last_tool_use_id = tu.get("id")
            # provide a tool result, then continue
            messages.append({"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": last_tool_use_id,
                "content": "Sunny, 18C in Paris.",
            }]})
        else:
            # next-turn user message to continue the thread
            if i < len(chain):
                messages.append({"role": "user", "content": "And what about 30 minus 7?"})


# ------------- main -------------

def main() -> int:
    # 用户可通过 env 覆盖默认 model
    for p in PROVIDERS:
        env = f"{p.upper()}_MODEL"
        v = os.environ.get(env)
        if v:
            SELECTED_MODEL[p] = v
    print(f"selected_models = {SELECTED_MODEL}\n", flush=True)

    print("\nStep 1: A 组场景（单家行为）", flush=True)
    print("=" * 60, flush=True)
    for p in PROVIDERS:
        print(f"\n--- A 组 / provider={p} ---", flush=True)
        scenario_A1(p)
        prior = scenario_A2(p)
        scenario_A3(p)
        scenario_A4(p, prior)
        scenario_A5(p)
        scenario_A6(p)

    print("\n\nStep 3: B 组场景（跨厂商轮询）", flush=True)
    print("=" * 60, flush=True)
    run_chain("B1-ds-whatai-xiaomi", ["deepseek", "whatai", "xiaomi"], use_tools=False)
    run_chain("B2-whatai-ds-xiaomi", ["whatai", "deepseek", "xiaomi"], use_tools=False)
    run_chain("B3-xiaomi-ds-whatai", ["xiaomi", "deepseek", "whatai"], use_tools=False)
    run_chain("B4-ds-xiaomi-whatai-tools", ["deepseek", "xiaomi", "whatai"], use_tools=True)

    # 最终报告
    print("\n\n# 调查报告", flush=True)
    print("=" * 60, flush=True)
    print(f"\n## 选用模型\n")
    for p, m in SELECTED_MODEL.items():
        print(f"- **{p}**: `{m}`")
    print(f"\n## 场景结果汇总\n")
    print(f"| scenario | provider | status | summary |")
    print(f"|---|---|---|---|")
    for row in REPORT_ROWS:
        s = row["summary"].replace("|", "\\|").replace("\n", " ")
        print(f"| {row['scenario']} | {row['provider']} | {row['status']} | {s} |")
    print(f"\n所有 raw req/resp 落在 `scripts/probe-out/`，共 {len(list(OUT_DIR.iterdir()))} 个文件")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
