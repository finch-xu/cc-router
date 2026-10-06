#!/usr/bin/env python3
"""cc-router-tui 的伪终端冒烟测试 (仅 macOS / Linux)。

单测用 TestBackend, 测不到「真的进备用屏幕、真的读键盘、真的退得出来」这一段。这个脚本:
  1. 起一个假的 cc-router 后端 (16 个 command + 事件流, 含 `list_providers` /
     `create_subscription` / `refresh_model_list` / `probe_custom_models` / `delete_subscription`),
     在临时目录写一份 runtime.json, 记录 `update_subscription` / `update_virtual_model` /
     `list_requests` / `create_subscription` / `delete_subscription` 收到的原始请求体供事后断言;
  2. 在 80x24 的伪终端里跑 TUI, 依次按
     2 / j / t / e / ? / Esc / ⏎⏎glm⏎ / q / n / Esc / y / ⏎⏎glm⏎ / s / 3 / l / J / m / s / 4 /
     空格 / 空格 / ⏎ / ⏎ / jj / Esc / 1 /
     2 / Esc / n / ⏎ / chatgpt / ⏎ / ⏎ / ⏎ / sk-test / Tab×3 / ⏎ / Tab×6 / ⏎ / 2 / g / d / y / q
     (2 = 订阅页; j 选中第二条 "Kimi 备用"; t = 测试连接, e = 就地启停;
     ⏎⏎glm⏎ 造一份草稿 (改 fable 槽模型) 之后先走一遍 fix round final (M9c) 加的放弃流程练习——
     q (脏页面上 q 会先问「确定放弃」) → n (选否, 草稿原样保留) → Esc (再问一次) → y (这次选是,
     草稿真的被丢弃, 焦点回到列表)——验证确认放弃这条路径在真终端上也能走通, 而不是只在
     `TestBackend` 单测里测过; 之后重新走一遍 ⏎⏎glm⏎ 造草稿、这次真的按 s 保存 (打一次假后端的
     `update_subscription`); 3 = 虚拟模型页, 真页面: l 从 Models 进 Members、J 把第一条订阅下移
     一位、m 切换调度模式 (Sequential -> RoundRobin, 同一份草稿), s 保存 (打一次假后端的
     `update_virtual_model`); 4 = 实时路由页 (真页面): 空格暂停、再按一次空格继续
     (`Live::toggle_pause` 是纯页面内状态, 不经 `Action`, 只能靠画面文字断言走过这条路径);
     ⏎ 跟随最新时目标是最后一个尝试 (haiku/示例中转, 没发 finished, 订阅 "3") ——跳到日志页
     并带着这条订阅的过滤条件重新发起 `list_requests`; 日志页里再按 ⏎ 打开第一条 (成功, 带
     effort/工具字段) 的详情弹窗、jj 往下滚两格 (露出「工具调用」小节)、Esc 关掉弹窗、1 = 回总览;
     接着走一遍新建 + 删除的完整向导流程——2 回订阅页 (脚本前面进过一次详情, `Esc` 确保焦点回到
     列表, `n` 只在 `Focus::List` 下生效) → n 打开向导 (等厂商列表回来) → ⏎ 打开厂商 picker →
     输入 "chatgpt" 过滤到 OAuth 厂商 → ⏎ 选中它 (不落值, 只弹「请在桌面端添加」的提示) → ⏎ 再开
     一次 picker (未过滤, 默认选中第一项 = 智谱) → ⏎ 选中它 (`choose_provider` 自动把焦点跳到
     API Key, 不需要额外导航) → 打 "sk-test" → Tab 三次 (只需要两次到「下一步」按钮, 多按的会被
     `FormState::step` 在边界夹住, 不会越界) → ⏎ 提交 (等 `create_subscription` +
     `refresh_model_list` 都跑完, 进第二步; 自动发现预填了四个核心槽) → Tab 六次 (同样只需要五次
     到「保存」, 多按的被夹住) → ⏎ 保存槽位 (向导关闭并重新拉一次订阅列表) → 2 回订阅页 → g 回到
     第一条 (脚本前面用 j 选过第二条) → d 删除它 → y 确认);
  3. 断言: 退出码 0、进出过备用屏幕、几个页面的关键文字 (含就地操作的 toast 文案、确认放弃提示、
     实时路由页的面板标题与暂停态、日志页的总数与详情弹窗内容、OAuth 厂商的桌面端提示、创建 /
     删除成功的 toast) 都出现过、假后端真的收到了 `update_subscription`(手动改槽位那次, fable 槽
     模型是选中的 "glm-4.6", 见下面 "Kimi 备用" 的 `model_cache`)、`update_virtual_model` (调度
     模式已经从 sequential 切到 round_robin、订阅顺序被重排)、`list_requests` (pageSize=50、按
     订阅 "3" 过滤)、`create_subscription` (`input.model_slots.fable == "(pending)"` 第二步之前
     还没绑真实模型、`input.source.kind == "from_template"`、`input.api_key == "sk-test"`)、
     向导那次的 `update_subscription` (patch 只有 `model_slots` 一个键——向导不设置 effort) 与
     `delete_subscription` (`id == "1"`, 对应删除时选中的第一条) 的请求体、空闲 2 秒几乎不输出
     (按需重绘, 且这个窗口不撞上任何 toast 的消散动效)。

  假后端的 SSE 在 1.5 秒发出状态变更事件之后, 紧接着发三组 `route_attempt_*` 事件
  (started(model-sonnet, "1") + finished(true); started(model-opus, "2") + finished(false);
  started(model-haiku, "3"), 不发 finished——留一条常驻的「进行中」行), 给实时路由页 (键 `4`)
  一份看得见内容的假数据。切进这一页之后按两次空格验证暂停 / 继续 (`Live::toggle_pause` 不产出
  `Cmd`, 纯页面内状态, 只能靠画面文字断言)。

  假后端的 `list_requests` 固定返回三条记录 (成功, 带 effort 与工具字段 / 失败 429, 带
  error_message / 超时) 、`total: 3`, 不管请求体里的过滤条件是什么都返回同一份——与其它假
  command 同一套「忽略参数, 返回固定数据」写法, 过滤条件本身只靠断言收到的请求体来验证。

用法 (仓库根目录):
  cd src-tauri && cargo build -p cc-router-tui && cd ..
  python3 src-tauri/crates/cc-router-tui/dev/pty_smoke.py src-tauri/target/debug/cc-router-tui-bin

不进 CI (依赖伪终端与时序); 改了 runtime.rs / main.rs 之后手动跑。后续阶段加页面时, 在 DATA 里补 command、在 EXPECT 里补文字。
"""
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import sys
import tempfile
import termios
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

NOW_MS = int(time.time() * 1000)


def quota(limit, used):
    return {"period": "daily", "limit": limit, "input": used, "output": 0, "cache_creation": 0,
            "cache_read": 0, "period_start_ms": 0, "exceeded": used >= limit}


def sub(sid, name, state, dispatchable, usage=None, cooldown=None, model_cache=None):
    # provider_id / base_url / auth_type / model_slots 是 Task 2 加的订阅详情字段, dto::Subscription
    # 里没有 #[serde(default)], 缺了任何一个都会让 list_subscriptions 反序列化失败, 订阅页 (Task 3
    # 起是真页面) 整页转圈圈。model_cache 有 #[serde(default)], 缺省 (None) 时干脆不带这个 key。
    d = {"id": sid, "display_name": name, "provider_display_name": "p", "enabled": True, "state": state,
         "cooldown_until": cooldown, "last_error_message": None, "is_dispatchable": dispatchable,
         "quota_usage": [usage] if usage else [],
         "provider_id": "p", "base_url": "https://example.invalid", "auth_type": "api_key",
         "model_slots": {"fable": "d", "opus": "a", "sonnet": "b", "haiku": "c"}}
    if model_cache is not None:
        d["model_cache"] = model_cache
    return d


DATA = {
    "proxy_status": {"port": 23456, "running": True, "mode": "http", "http_port": 23456, "https_port": None,
                     "listen_all": False, "base_url": "http://127.0.0.1:23456"},
    "get_settings": {"preferred_language": "zh", "tui_enabled": True, "auth_enabled": True},
    "get_overall_stats": {"total_requests": 1284, "success_rate_pct": 98.6, "total_input_tokens": 3000000,
                          "total_output_tokens": 200000, "total_cache_creation_tokens": 0,
                          "total_cache_read_tokens": 0},
    "get_daily_series": [{"day": "2026-01-01", "hour": h, "request_count": abs(h - 12) * 3 + 1} for h in range(24)],
    "list_subscriptions": [
        sub("1", "智谱主号", "healthy", True, quota(100, 62)),
        # Task 5: 补上 model_cache, 好让脚本覆盖 picker「匹配项排在自定义行前面」这条 (以前 Kimi
        # 备用没有缓存过模型, 输入 "glm" 只会出现「使用「glm」」一个候选, 测不出排序)。
        sub("2", "Kimi 备用", "rate_limited", False, quota(100, 91), NOW_MS + 42000, model_cache={
            "fetched_at": NOW_MS,
            "models": [{"id": "glm-4.6", "display_name": None}, {"id": "glm-4.5-air", "display_name": None}],
        }),
        sub("3", "示例中转", "auth_failed", False),
        # System One 订阅: 只有 Jev 槽, 只出现在 model-jev 里。
        dict(sub("4", "Ollama 决策", "healthy", True), endpoint_protocol="systemone",
             model_slots={"fable": "", "opus": "", "sonnet": "", "haiku": "", "jev": "clef-flash"}),
    ],
    # Task 4 的四个就地操作: 这里只挑 test_connection / set_subscription_enabled 两个真的按一遍
    # (另外两个 refresh_model_list / refresh_subscription_balance 走同一条 command 分派逻辑,
    # 不重复验证), 响应形状照抄 dto.rs 里对应的反序列化目标。
    "test_connection": {"ok": True, "message": "连接正常", "http_status": 200, "model_used": "moonshot-v1-8k", "state_reset": True},
    "refresh_model_list": {"kind": "auto", "models": [{"id": "moonshot-v1-8k", "display_name": None}], "fetched_at": NOW_MS},
    "refresh_subscription_balance": {"kind": "unsupported"},
    # Task 5: `Mutation::UpdateSlots` 落地成 `update_subscription`; `runtime.rs::call_mutation` 把
    # 返回值反序列化成 `serde_json::Value` 就直接丢弃 (权威值等 refetch 的 list_subscriptions 拿),
    # 所以随便一个合法 JSON 都够, 空对象最省事。
    "update_subscription": {},
    # Task 6: 6 个虚拟模型, 后端固定顺序 (model-jev 最后); subscription_ids 引用上面的假订阅。
    "list_virtual_models": [
        {"name": "model-fable", "mode": "sequential", "subscription_ids": ["1", "2"]},
        {"name": "model-opus", "mode": "round_robin", "subscription_ids": ["1"]},
        {"name": "model-sonnet", "mode": "sticky", "subscription_ids": ["1", "2", "3"]},
        {"name": "model-haiku", "mode": "sequential", "subscription_ids": []},
        {"name": "model-fallback", "mode": "sequential", "subscription_ids": ["3"]},
        {"name": "model-jev", "mode": "sequential", "subscription_ids": ["4"]},
    ],
    # Task 8: 日志页 (键 4 → ⏎ 跳过去) 的假数据。不管请求体里的 `filters` 是什么都返回同一份
    # (与其它假 command 一致); 过滤条件本身只靠 `RECORDED["list_requests"]` 断言。三条分别是
    # 成功 (带 effort + 工具字段, 供详情弹窗展示「工具调用」小节) / 失败 429 (带 error_message) /
    # 超时。
    # 新建订阅向导用到的四个 command (契约锁在 `src-tauri/src/tui_contract.rs`); 脚本后段真的
    # 按键走一遍向导, 并对 create_subscription / update_subscription 的请求体做断言。
    # 两个内置厂商: 一个 api_key 带两个 endpoint (给「选厂商 -> 选 endpoint」这条路径用),
    # 一个 chatgpt_oauth (没有 endpoints, 用来验证厂商选择器里的置灰判断)。
    "list_providers": [
        {
            "id": "zhipu", "display_name": "智谱", "description": None,
            "endpoints": [
                {"id": "default", "label": "默认", "base_url": "https://open.bigmodel.cn/api/anthropic"},
                {"id": "intl", "label": "国际版", "base_url": "https://intl.bigmodel.cn/api/anthropic"},
            ],
            "default_endpoint": "default", "auth": {"type": "api_key"},
            "model_discovery": {"enabled": True, "example_models": ["glm-4.6"]},
            "translations": {
                lang: {"display_name": "Zhipu GLM", "description": None, "endpoints": {
                    "default": {"label": "Default"}, "intl": {"label": "International"},
                }} for lang in ("en", "ja")
            },
        },
        {
            "id": "chatgpt", "display_name": "ChatGPT", "description": None,
            "endpoints": [], "default_endpoint": None, "auth": {"type": "chatgpt_oauth"},
            "model_discovery": {"enabled": False, "example_models": []},
            "translations": {lang: {"display_name": "ChatGPT", "description": None, "endpoints": {}} for lang in ("en", "ja")},
        },
        # 带 url_params 的厂商 (照 cloudflare 精简): 只保证新字段不会让 TUI 崩, 脚本不走它。
        {
            "id": "cloudflare", "display_name": "Cloudflare", "description": None,
            "endpoints": [
                {"id": "direct", "label": "直连", "base_url": "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1",
                 "url_params_used": ["account_id"]},
            ],
            "default_endpoint": "direct", "auth": {"type": "openai_chat_completions_api_key"},
            "model_discovery": {"enabled": True, "example_models": []},
            "url_params": [
                {"id": "account_id", "label": "账户 ID", "placeholder": "32 位十六进制", "pattern": "^[0-9a-f]{32}$"},
            ],
            "translations": {
                lang: {"display_name": "Cloudflare", "description": None,
                       "endpoints": {"direct": {"label": "Direct"}},
                       "url_params": {"account_id": {"label": "Account ID", "placeholder": "32 hex chars"}}}
                for lang in ("en", "ja")
            },
        },
    ],
    "probe_custom_models": {
        "kind": "auto",
        "models": [{"id": "glm-4.6", "display_name": None}],
        "models_url": "https://relay.example.invalid/v1/models",
    },
    "list_requests": {
        "items": [
            {
                "id": "req-1", "timestamp": NOW_MS - 60000, "virtual_model_name": "model-haiku",
                "subscription_id": "3", "provider_id": "p", "endpoint_id": "default",
                "real_model_name": "c", "is_streaming": True, "status": "success", "http_status": 200,
                "total_latency_ms": 1800, "input_tokens": 12300, "output_tokens": 3400,
                "client_effort": "high", "effective_effort": "high", "effort_source": "slot",
                "upstream_effort": "high", "stop_reason": "end_turn", "tools_offered_count": 2,
                "tool_result_count": 1, "tool_use_count": 2, "tool_use_names": "[\"Read\",\"Bash\"]",
            },
            {
                "id": "req-2", "timestamp": NOW_MS - 120000, "virtual_model_name": "model-opus",
                "subscription_id": "2", "provider_id": "p", "endpoint_id": "default",
                "real_model_name": "b", "is_streaming": False, "status": "error", "http_status": 429,
                "error_message": "触发限流",
            },
            {
                "id": "req-3", "timestamp": NOW_MS - 180000, "virtual_model_name": "model-haiku",
                "subscription_id": "3", "provider_id": "p", "endpoint_id": "default",
                "real_model_name": "c", "is_streaming": False, "status": "timeout",
            },
        ],
        "total": 3,
    },
}

# M9(b) (fix round final): 假后端把 `update_subscription` / `update_virtual_model` 真正收到的请求体
# 记下来, 脚本在按完全部键之后据此断言"保存的值就是这次操作里选中的值", 而不是只看 toast 文案
# (toast 文案对了不代表发给后端的 payload 也对——旧版这里只断言过文案)。
RECORDED = {}

# 去掉转义序列之后必须出现过的文字
EXPECT = [
    "总览", "1,284", "98.6%", "智谱主号", "已连接", "订阅 (4)", "备注名", "键位",
    "连接正常",  # test_connection 成功的 toast
    "已停用",  # set_subscription_enabled 的 toast (Kimi 备用被 e 停用)
    "确定放弃",  # M9(c): 脏页面上 q / Esc 弹出的确认放弃提示 (confirm_discard 的子串)
    "槽位已保存",  # update_subscription 成功的 toast (改 fable 槽模型再保存)
    # M9(a): 旧版这里断言的是 "虚拟模型" (标签栏文字, 不管有没有真的进过那一页都会显示, 测不出
    # "真的切换到了这一页" 这件事), 换成只有真进了虚拟模型页才会出现的页面内文字。
    "model-fable",
    "model-jev",  # 第 6 个虚拟模型出现在左栏
    # update_virtual_model 成功的 toast (Task 6: l 进 Members、J 重排、m 切模式、s 保存)。
    # `toast_vm_saved` 的实际文案是 `{vm}：已保存` (`i18n.rs`), 用 model-fable 专属的完整文案,
    # 不是 "槽位已保存" (订阅页保存的 toast) 的子串。
    "model-fable：已保存",
    # Task 7: 实时路由页 (键 4) 不再是占位——面板标题、暂停态提示、常驻「进行中」行 (haiku/示例中转,
    # 没发 finished) 都要真的出现在画面上。
    "最近 60 秒",
    "已暂停",
    "→ 示例中转",
    # 从实时路由页 ⏎ 跳到日志页 (带着订阅 "3" 的过滤, 假后端固定返回 3 条) + 打开第一条
    # (成功, 带工具字段) 的详情弹窗。
    "共 3 条",
    "请求详情",
    "工具调用",
    # 「请在桌面端添加」(OAuth 厂商选中提示) 故意不放在这个"只要出现过就算过"的列表里, 也不用
    # 它的出现次数当判据——厂商 picker 里 OAuth 厂商那一行的 label 本身就带这串字, 而且这一格
    # 跟 toast 的坐标在 80×24 下有真实重叠, `BufferDiff` 可能不会重新发送它 (fix round 1/2 的
    # 报告有详细坐标推导)。改用「选中之后 picker 还能不能再打开一次」这个状态信号, 见下方按键
    # 序列与断言。
    # 创建 + 保存槽位成功的 toast (`wiz_created`); 备注名跟着厂商显示名 "智谱" 自动生成, 与已有的
    # 四条假订阅都不重名, 不会被追加序号。
    "已创建「智谱」",
    # 删除第一条订阅 ("智谱主号", id "1") 成功的 toast (`toast_deleted`)。
    "已删除「智谱主号」",
]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("content-length", 0)))
        name = self.path.rsplit("/", 1)[-1]
        if name == "set_subscription_enabled":
            # 真的改一下内存里的 enabled, 好让后续 e 触发的 Cmd::Fetch(Subscriptions) 刷新时
            # 订阅页能看到状态真的变了 (不只是 toast 文案对了)。返回 JSON null (对应 Rust 端的 `()`)。
            req = json.loads(raw or b"{}")
            for s in DATA["list_subscriptions"]:
                if s["id"] == req.get("id"):
                    s["enabled"] = req.get("enabled", s["enabled"])
            body = json.dumps(None).encode()
        elif name == "update_subscription":
            # 记下收到的 patch, 脚本事后断言 fable 槽真的是这次选中的模型。
            # 这个 command 会被打两次 (订阅页手动改槽位一次, 向导第二步保存一次),
            # `RECORDED["update_subscription"]` 只留最后一次 (给向导那次的「patch 只有
            # model_slots 一个键」断言用), 完整历史另存一份供订阅页那次的断言引用。
            req = json.loads(raw or b"{}")
            RECORDED["update_subscription"] = req
            RECORDED.setdefault("update_subscription_history", []).append(req)
            body = json.dumps(DATA[name]).encode()
        elif name == "update_virtual_model":
            # 同样真的改一下内存里的条目, 好让 s 之后的 `Cmd::Fetch(VirtualModels)` 补拉看到新顺序 /
            # 新模式。`input` 是 `{mode, subscription_ids}` (`runtime.rs::call_mutation` 拼的形状)。
            # 返回 JSON null (对应 Rust 端的 `()`, 与 `set_subscription_enabled` 同一套约定)。M9(b):
            # 同样记下收到的请求体。
            req = json.loads(raw or b"{}")
            RECORDED["update_virtual_model"] = req
            input_ = req.get("input", {})
            for vm in DATA["list_virtual_models"]:
                if vm["name"] == req.get("name"):
                    vm["mode"] = input_.get("mode", vm["mode"])
                    vm["subscription_ids"] = input_.get("subscription_ids", vm["subscription_ids"])
            body = json.dumps(None).encode()
        elif name == "list_requests":
            # Task 8: 记下收到的查询 (分页大小 + 过滤条件), 脚本事后断言它们真的是日志页发出的那份
            # (只保留最后一次, 与 `update_subscription`/`update_virtual_model` 同一套约定)。不管
            # 参数是什么都返回固定的三条假数据——过滤条件本身不在假后端里真的生效。
            req = json.loads(raw or b"{}")
            RECORDED["list_requests"] = req
            body = json.dumps(DATA[name]).encode()
        elif name == "create_subscription":
            # 记下请求体供事后断言, 返回一个固定的新 id。
            req = json.loads(raw or b"{}")
            RECORDED["create_subscription"] = req
            body = json.dumps({"id": "9"}).encode()
        elif name == "delete_subscription":
            # 只记请求体供事后断言 (删除时选中的是哪一条), 不真的从 DATA["list_subscriptions"] 里删。
            # 真后端签名是 AppResult<()>, serde_json::to_value(()) 是 null, 所以这里回 null 而不是
            # {}——客户端按 `()` 反序列化, 回 {} 会报 "invalid type: map, expected unit"
            # (与 set_subscription_enabled / update_virtual_model 的假实现同一约定)。
            req = json.loads(raw or b"{}")
            RECORDED["delete_subscription"] = req
            body = json.dumps(None).encode()
        else:
            body = json.dumps(DATA[name]).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # /ui/api/events
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()
        try:
            time.sleep(1.5)
            DATA["list_subscriptions"][0].update(state="rate_limited", is_dispatchable=False)
            self.wfile.write(b'event: subscription_state_changed\ndata: "1"\n\n')
            self.wfile.flush()
            # Task 7: 紧接着发三组路由尝试事件, 给实时路由页 (键 4) 一份看得见内容的假数据——
            # 一对成功 / 一对失败 / 一条只有 started 没有 finished 的「进行中」常驻行。
            for name, sub_id, success in (
                ("model-sonnet", "1", True),
                ("model-opus", "2", False),
            ):
                started = json.dumps({"subscription_id": sub_id, "virtual_model": name})
                self.wfile.write(f"event: route_attempt_started\ndata: {started}\n\n".encode())
                finished = json.dumps({"subscription_id": sub_id, "virtual_model": name, "success": success})
                self.wfile.write(f"event: route_attempt_finished\ndata: {finished}\n\n".encode())
            pending = json.dumps({"subscription_id": "3", "virtual_model": "model-haiku"})
            self.wfile.write(f"event: route_attempt_started\ndata: {pending}\n\n".encode())
            self.wfile.flush()
            while True:
                time.sleep(5)
                self.wfile.write(b": keepalive\n\n")
                self.wfile.flush()
        except OSError:
            pass


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    binary = os.path.abspath(sys.argv[1])

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    data_dir = tempfile.mkdtemp(prefix="ccr-tui-smoke-")
    with open(os.path.join(data_dir, "runtime.json"), "w") as f:
        json.dump({"pid": 1, "app_version": "0.0.0-smoke", "http_port": server.server_address[1],
                   "https_port": None, "ca_pem_path": None, "local_secret": "smoke"}, f)

    pid, fd = pty.fork()
    if pid == 0:
        os.environ.update(COLORTERM="truecolor", TERM="xterm-256color")
        os.environ.pop("NO_COLOR", None)
        os.execv(binary, [binary, "--data-dir", data_dir])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

    out = bytearray()

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            if select.select([fd], [], [], 0.05)[0]:
                try:
                    out.extend(os.read(fd, 65536))
                except OSError:
                    return

    def visible_text(limit=None):
        # 去掉转义序列之后的可见文字。**必须从字节 0 开始切**, 不能取 `out` 中间任意一段
        # (`out[a:b]`) 再单独去转义——终端全屏重绘时几乎每个格子前面都带一段样式转义序列, 任意
        # 字节偏移量都可能落在某个序列中间, 序列开头 (`\x1b[`) 被切掉的那一截会被正则漏判、原样
        # 混进"文字"里, 把旁边本来连续的真实文字拆碎 (fix round 1 复现过这个问题: 窄窗口切片让
        # "请在桌面端添加" 这几个字被拆散, 断言拿不到完整子串)。从 0 开始切的话, 中途顶多在切口
        # 处留一小段没切干净的转义码尾巴 (或者一个被截断的宽字符), 不会污染前面已经完整闭合的
        # 那些序列, 所以这个函数只接受"到第几个字节为止"的前缀, 不接受任意区间。
        raw = bytes(out) if limit is None else bytes(out[:limit])
        return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", raw.decode("utf-8", "replace"))

    def wait_until_count_increases(needle, baseline, timeout=3.0):
        # fix round 2: 换 `pump(固定时长)` + 单次采样为轮询——固定时长本身可能不够 (机器负载 /
        # 调度延迟), 用短切片反复采样直到计数真的变多, 或者等够 `timeout` 还没变多就放弃。
        # 返回 (是否等到, 超时时最后一次采到的计数) 供调用方拼失败信息。
        deadline = time.time() + timeout
        while True:
            count = visible_text(len(out)).count(needle)
            if count > baseline:
                return True, count
            if time.time() >= deadline:
                return False, count
            pump(0.1)

    pump(3.0)  # 启动动效 + 首次加载 + 1.5s 时的状态变更事件
    # 2 = 订阅页 (真页面, 期待「订阅 (4)」「备注名」); j = 选中第二条 "Kimi 备用";
    # t = 测试连接 (等够 0.8s 让假后端的响应 + toast + 重拉列表都跑完), e = 就地启停 (同样等 0.8s);
    # ? / Esc = 帮助弹窗开关; 此时仍在订阅页且选中 "Kimi 备用":
    #   ⏎ 进详情 (焦点落在 fable 槽) → ⏎ 打开 fable 槽的模型 picker (I2 起输入框不再预填当前值,
    #   打开时是空的) → 输入 "glm" (Task 5 起 Kimi 备用缓存了两个模型 glm-4.6/glm-4.5-air, "glm"
    #   两个都能匹配上, 匹配到的项排在「使用「glm」」自定义行前面) → ⏎ 选中排在最前的匹配项,
    #   写进草稿 (fable 槽 = "glm-4.6")。
    #
    # M9(c): 造完草稿先不急着保存, 练一遍放弃流程 (旧版这条脚本从没走过这条路径, 只在
    # `TestBackend` 单测里测过): q (脏页面按 q 会先问「确定放弃」, 不立即退出) → n (选否, 草稿
    # 原样保留, 弹窗关掉) → Esc (在 Detail 焦点上脏着按 Esc 同样会问一遍「确定放弃」) → y (这次
    # 选是, `Action::Confirmed(DiscardDraft)` 真的丢弃草稿, 焦点退回列表)。
    #
    # 放弃之后重新走一遍 ⏎⏎glm⏎ 造一份新草稿 (同样落在 "glm-4.6"), 这次真的按 s 保存 (打一次
    # 假后端的 `update_subscription`, 等够 0.8s 让响应 + toast + 重拉列表都跑完)。
    #
    # 3 = 虚拟模型页 (Task 6 起是真页面, 默认选中 model-fable / Models 焦点):
    #   l 切到 Members 焦点 (选中 model-fable 的成员列表) → J 把第一条订阅下移一位 (造一个草稿,
    #   ids 从 ["1","2"] 变成 ["2","1"]) → m 切换调度模式 (Sequential -> RoundRobin, 同一份草稿)
    #   → s 保存 (真的打一次假后端的 `update_virtual_model`, 等够 0.8s 让响应 + toast + 重拉都跑完);
    # 4 = 实时路由页 (Task 7 起是真页面, 期待「最近 60 秒」面板标题、haiku/示例中转 那条常驻的
    #   「进行中」行): 空格暂停 (期待「已暂停」) → 空格再继续。
    #
    # Task 8: 仍在实时路由页, 跟随最新 (没有选中任何一行) → ⏎ 应该取最后一个可见尝试 (haiku/
    #   示例中转, 订阅 "3") 的订阅, 跳到日志页并带着这条过滤重新发起 `list_requests` (等够 0.8s
    #   让响应落地、期待「共 3 条」) → ⏎ 打开第一条 (成功, 带 effort/工具字段) 的详情弹窗 (期待
    #   「请求详情」「工具调用」) → jj 往下滚两格 (露出「工具调用」小节, 单格还不够) → Esc 关掉弹窗; 1 = 回总览。
    for keys, wait in (
        (b"2", 0.6),
        (b"j", 0.6),
        (b"t", 0.8),
        (b"e", 0.8),
        (b"?", 0.6),
        (b"\x1b", 0.6),
        (b"\r", 0.4),
        (b"\r", 0.4),
        (b"glm", 0.3),
        (b"\r", 0.4),
        (b"q", 0.4),
        (b"n", 0.4),
        (b"\x1b", 0.4),
        (b"y", 0.4),
        (b"\r", 0.4),
        (b"\r", 0.4),
        (b"glm", 0.3),
        (b"\r", 0.4),
        (b"s", 0.8),
        (b"3", 0.6),
        (b"l", 0.4),
        (b"J", 0.4),
        (b"m", 0.4),
        (b"s", 0.8),
        (b"4", 0.6),
        (b" ", 0.4),  # Task 7: 暂停实时路由页
        (b" ", 0.4),  # 再按一次继续
        (b"\r", 0.8),  # Task 8: 跟随最新时 ⏎ 跳到日志页 (目标订阅 "3"), 等响应落地
        (b"\r", 0.4),  # 打开第一条的详情
        # req-1 的「基本信息」+「思考强度」两节加起来就有 16 行, 正好填满 80×24 下详情弹窗一屏
        # (last_rows=16)——「工具调用」小节的标题在这之后, 单按一次 j (scroll=1) 还看不到, 两次
        # (scroll=2) 才够, 与 `widgets::detail::area`/`draw` 的行高计算对齐, 不是随手选的数字。
        (b"jj", 0.3),
        (b"\x1b", 0.4),  # 关掉详情弹窗
        (b"1", 0.6),
        # 完整走一遍新建 + 删除。2 回订阅页: 脚本前面进过一次详情且成功保存后焦点仍停在那里
        # (`Focus::Detail`, 不脏), `n` 只在 `Focus::List` 下生效, 先 Esc 一次确保焦点回到列表。
        (b"2", 0.6),
        (b"\x1b", 0.3),
        (b"n", 0.8),  # 打开向导, 等厂商列表回来
        (b"\r", 0.4),  # Provider 行 ⏎ 打开厂商 picker
        (b"chatgpt", 0.3),  # 按 id/label 过滤到 OAuth 厂商——过滤出的这一行本身就带着
        # `wiz_desktop_only` 的文案 (`basics.rs::provider_picker`: label 是
        # `"{显示名} · {desktop_only}"`), 所以光是走到这一步, 屏幕上已经出现过
        # "请在桌面端添加" 这串字——不能拿它证明"选中之后弹了提示"。
    ):
        os.write(fd, keys)
        pump(wait)

    # 选中 OAuth 厂商: **不**拿 toast 文案 "请在桌面端添加" 当判据 (fix round 1 里按出现次数的
    # 增量判断过, 但 fix round 2 复测发现这个判据本身不稳定, 约 40% 概率误报失败)。改用状态信号:
    # `apply_provider_choice` 的 OAuth 分支只弹一次提示就直接 `return`, **不**调
    # `BasicsForm::choose_provider`, 焦点仍停在 `Provider` 行 (`FieldKind::Pick`); 所以「picker
    # 关掉之后再按一次 ⏎, 应该还能再打开一次 picker (标题「选择厂商」再出现一次)」这件事本身就
    # 证明了焦点没有被挪走、`choose_provider` 没有被调用——如果 mutation 让 OAuth 走了
    # `choose_provider` 那条路, 焦点会直接跳到 `ApiKey` (Secret 行), 这里的 ⏎ 就变成"移到下一行"
    # 而不是"打开 picker", 标题不会再出现, 断言照样能抓到 (mutation 证明见 fix round 2 报告)。
    #
    # 不用 toast 的原因是真的量过坐标, 不是猜的: 80×24 下厂商 picker 居中 box 是
    # y∈[4,20) x∈[10,70) (`picker::area`), toast 贴右上角是 y∈[content.y, content.y+3)
    # x∈[61,79) (`toast::area`, `content.y` 是标签栏底下那一行, 也在 [4,20) 范围内)——两者的
    # y∈[4, content.y+3) x∈[61,70) 有真实重叠。选中厂商那一刻是「filtered picker 正显示这一行」
    # 直接过渡到「picker 关掉 + toast 出现」, 中间没有一帧先清空这块区域——如果 toast 恰好把
    # "请在桌面端添加" 画在跟 picker 过滤出的这一行完全相同的格子、完全相同的字符上,
    # ratatui 的 `BufferDiff` 只发送变化的格子, 这几格因为内容没变就不会被重新发送, 断言即使代码
    # 完全正确也会在这种坐标巧合下偶发失败, 加大超时或轮询都救不了 (量过的重叠区域, 不是猜测)。
    # 上面「重开 picker」的信号在选中前后隔着一次真正的关闭 (内容先换成向导表单再换回标题),
    # 不存在同一批格子内容从头到尾没变过这回事, 不受这个问题影响。
    picker_title_count_before = visible_text(len(out)).count("选择厂商")
    os.write(fd, b"\r")  # 选中 OAuth 厂商
    pump(0.4)
    os.write(fd, b"\r")  # 再开一次 picker (未过滤, initial 不匹配任何项时默认选中第一项 = 智谱)
    picker_reopened, picker_title_count_after = wait_until_count_increases("选择厂商", picker_title_count_before)

    for keys, wait in (
        (b"\r", 0.4),  # 选中智谱: `choose_provider` 自动把焦点跳到 API Key
        (b"sk-test", 0.2),
        # 只需要两次 Tab 到「下一步」按钮 (ApiKey -> DisplayName -> Submit), 多按的会在
        # `FormState::step` 的边界夹住, 不会越界吞掉本该给按钮的 ⏎。
        (b"\t\t\t", 0.3),
        (b"\r", 0.8),  # 提交: 等 create_subscription + refresh_model_list 都跑完, 进第二步
        # 第二步同理只需要五次 Tab 到「保存」(五个槽位行 + 保存按钮), 多按的同样被夹住。
        (b"\t\t\t\t\t\t", 0.3),
        (b"\r", 0.8),  # 保存槽位 (向导关闭并重新拉一次订阅列表)
        (b"2", 0.6),  # 回订阅页
        (b"g", 0.3),  # 选中第一条 (脚本前面用 j 选过第二条 "Kimi 备用")
        (b"d", 0.4),  # 删除它 (弹确认)
        (b"y", 0.4),
    ):
        os.write(fd, keys)
        pump(wait)
    # 让七条 toast (t 的「连接正常」、e 的「已停用」、订阅页 s 的「槽位已保存」、虚拟模型页 s 的
    # 「已保存」、厂商 picker 选中 OAuth 厂商的「请在桌面端添加」、向导创建成功的「已创建」、
    # 删除成功的「已删除」) 彻底放完再量「空闲」: toast 一条只能显示 3s (`toast::LIFETIME_MS`) +
    # 300ms 消散动效, 后一条还要等前一条弹出队列才轮到它显示 (`MAX_TOASTS=4`, 不会被挤掉但会排队)。
    # 放弃流程练习 (q/n/Esc/y) 本身不产生任何 toast, 只是往后推迟了几秒; 向导 + 删除流程把最后
    # 三条 toast 排在了整个按键序列的尾巴上, 所以 settle pump 要留到 20s。
    pump(20.0)
    before_idle = len(out)
    pump(2.0)
    idle_bytes = len(out) - before_idle
    os.write(fd, b"q")

    # 轮询退出而不是阻塞 waitpid: 如果 UI 没接住 q (或者卡死), 阻塞 waitpid 会让脚本永远
    # 挂着。边等边继续把 pty 的输出读走 (不读会堵住子进程写终端)。
    exited = False
    status = 0
    deadline = time.time() + 3.0
    while time.time() < deadline:
        if select.select([fd], [], [], 0.05)[0]:
            try:
                out.extend(os.read(fd, 65536))
            except OSError:
                pass
        reaped, status = os.waitpid(pid, os.WNOHANG)
        if reaped == pid:
            exited = True
            break
    if not exited:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        _, status = os.waitpid(pid, 0)

    raw = out.decode("utf-8", "replace")  # 备用屏幕进出的转义序列检查要看未去码的原始字符串
    text = visible_text()
    failures = []
    if not exited:
        failures.append("按 q 之后 3 秒内没有退出")
    elif not os.WIFEXITED(status):
        failures.append(f"被信号 {os.WTERMSIG(status)} 终止")
    elif os.WEXITSTATUS(status) != 0:
        failures.append(f"退出码 {os.WEXITSTATUS(status)}")
    if "\x1b[?1049h" not in raw or "\x1b[?1049l" not in raw:
        failures.append("没有成对地进入 / 离开备用屏幕")
    failures += [f"没出现过: {needle}" for needle in EXPECT if needle not in text]

    # M9(b): 光 toast 文案对了不能证明发给后端的 payload 也对——直接断言假后端真正收到的请求体。
    # `update_subscription` 这个 command 这次脚本里被打了两次 (这里断言的是订阅页手动改槽位那次,
    # 也就是历史记录里第一条; 向导保存槽位那次见下面 `wizard_slots_payload`), 所以取
    # `update_subscription_history[0]` 而不是只留最后一次的 `RECORDED["update_subscription"]`。
    sub_history = RECORDED.get("update_subscription_history", [])
    sub_payload = sub_history[0] if sub_history else None
    if not sub_payload:
        failures.append("假后端没有收到 update_subscription 请求")
    else:
        fable = sub_payload.get("patch", {}).get("model_slots", {}).get("fable")
        if fable != "glm-4.6":
            failures.append(f"update_subscription 的 fable 槽应该是 \"glm-4.6\" (放弃流程练习之后重新选的, 缓存里 "
                             f"「glm」匹配到的第一项), 实际 {fable!r}")

    vm_payload = RECORDED.get("update_virtual_model")
    if not vm_payload:
        failures.append("假后端没有收到 update_virtual_model 请求")
    else:
        input_ = vm_payload.get("input", {})
        if input_.get("mode") != "round_robin":
            failures.append(f"update_virtual_model 的 mode 应该是 \"round_robin\" (m 切换过一次), 实际 {input_.get('mode')!r}")
        if input_.get("subscription_ids") != ["2", "1"]:
            failures.append(f"update_virtual_model 的 subscription_ids 应该是 [\"2\", \"1\"] (J 下移过一次), 实际 {input_.get('subscription_ids')!r}")

    # Task 8: 实时路由页 ⏎ 跳到日志页应该带着目标订阅的过滤重新发起 `list_requests` (page 1,
    # pageSize 固定 50) ——同样直接断言收到的请求体, 不只看 toast/画面文字。
    requests_payload = RECORDED.get("list_requests")
    if not requests_payload:
        failures.append("假后端没有收到 list_requests 请求")
    else:
        if requests_payload.get("pageSize") != 50:
            failures.append(f"list_requests 的 pageSize 应该是 50, 实际 {requests_payload.get('pageSize')!r}")
        sub_filter = requests_payload.get("filters", {}).get("subscription_id")
        if sub_filter != "3":
            failures.append(f"list_requests 的 filters.subscription_id 应该是 \"3\" (⏎ 跳转日志页时带的过滤), 实际 {sub_filter!r}")

    # 新建订阅向导: 第一步提交时槽位还是占位符、来源是内置厂商模板、API Key 就是打的那串——同样
    # 直接查请求体, 不只看「进了第二步」这个间接信号。
    create_payload = RECORDED.get("create_subscription")
    if not create_payload:
        failures.append("假后端没有收到 create_subscription 请求")
    else:
        input_ = create_payload.get("input", {})
        fable = input_.get("model_slots", {}).get("fable")
        if fable != "(pending)":
            failures.append(f"create_subscription 的 model_slots.fable 应该是 \"(pending)\" (第二步之前的占位), 实际 {fable!r}")
        source = input_.get("source", {})
        if source.get("kind") != "from_template":
            failures.append(f"create_subscription 的 source.kind 应该是 \"from_template\", 实际 {source.get('kind')!r}")
        # fix round 1: 选中 OAuth 厂商那一步不该落值——这里从结果反向核实: 最终提交的厂商真的是
        # 后来选的智谱 (`default` 接入点), 不是先选中的 ChatGPT (它没有 endpoints, `endpoint_id`
        # 会是空串) 也不是两者混出来的半吊子状态。
        if source.get("provider_id") != "zhipu":
            failures.append(f"create_subscription 的 source.provider_id 应该是 \"zhipu\" (选 OAuth 厂商不该落值), 实际 {source.get('provider_id')!r}")
        if source.get("endpoint_id") != "default":
            failures.append(f"create_subscription 的 source.endpoint_id 应该是 \"default\", 实际 {source.get('endpoint_id')!r}")
        if input_.get("api_key") != "sk-test":
            failures.append(f"create_subscription 的 api_key 应该是 \"sk-test\", 实际 {input_.get('api_key')!r}")

    # fix round 2: 改用「picker 关掉之后还能不能再打开一次」这个状态信号判断选中 OAuth 厂商有没有
    # 落值/挪焦点 (见上面按键序列那段大注释解释为什么不用 toast 文案)。
    if not picker_reopened:
        failures.append(
            f"选中 OAuth 厂商 (ChatGPT) 之后, ⏎ 应该还能再打开一次厂商 picker (标题「选择厂商」应该"
            f"再出现一次, 说明焦点还停在 Provider 行、没有调 choose_provider), 3 秒内出现次数没有从 "
            f"{picker_title_count_before} 变多, 最后采到 {picker_title_count_after}"
        )

    # 向导第二步保存槽位打的是同一个 `update_subscription` command, 这里是历史记录里最后一条
    # (`RECORDED["update_subscription"]` 只留最后一次, 与上面订阅页那次的 `sub_history[0]` 是同一
    # 份历史的两端)。向导不设置 effort, patch 应该只有 model_slots 这一个键。
    wizard_slots_payload = RECORDED.get("update_subscription")
    if not wizard_slots_payload or wizard_slots_payload is sub_payload:
        failures.append("假后端没有收到向导保存槽位那次 update_subscription 请求")
    else:
        patch_keys = sorted(wizard_slots_payload.get("patch", {}).keys())
        if patch_keys != ["model_slots"]:
            failures.append(f"向导保存槽位的 patch 应该只有 model_slots 一个键, 实际 {patch_keys}")

    delete_payload = RECORDED.get("delete_subscription")
    if not delete_payload:
        failures.append("假后端没有收到 delete_subscription 请求")
    elif delete_payload.get("id") != "1":
        failures.append(f"delete_subscription 的 id 应该是 \"1\" (g 选中的第一条 \"智谱主号\"), 实际 {delete_payload.get('id')!r}")

    # 空闲时只有 250ms tick 带来的零星重绘 (冷却倒计时每秒变一格)。几 KB 以上说明在持续全速重画
    # (含撞上了某条 toast 还没放完的消散动效)。
    if idle_bytes > 4000:
        failures.append(f"空闲 2 秒输出了 {idle_bytes} 字节, 按需重绘失效")

    print(f"输出 {len(out)} 字节, 空闲 2 秒 {idle_bytes} 字节")
    print(f"记录的请求体: update_subscription={sub_payload}")
    print(f"记录的请求体: update_virtual_model={vm_payload}")
    print(f"记录的请求体: list_requests={requests_payload}")
    print(f"记录的请求体: create_subscription={create_payload}")
    print(f"记录的请求体: update_subscription(向导保存槽位)={wizard_slots_payload}")
    print(f"记录的请求体: delete_subscription={delete_payload}")
    if failures:
        sys.exit("冒烟失败:\n  " + "\n  ".join(failures))
    print("冒烟通过")


if __name__ == "__main__":
    main()
