#!/usr/bin/env python3
"""cc-router-tui 的伪终端冒烟测试 (仅 macOS / Linux)。

单测用 TestBackend, 测不到「真的进备用屏幕、真的读键盘、真的退得出来」这一段。这个脚本:
  1. 起一个假的 cc-router 后端 (11 个 command + 事件流), 在临时目录写一份 runtime.json;
  2. 在 80x24 的伪终端里跑 TUI, 依次按 2 / j / t / e / ? / Esc / ⏎⏎glm⏎ / s / 3 / l / J / s / 4 / 1 / q
     (2 = 订阅页; j 选中第二条 "Kimi 备用"; t = 测试连接, e = 就地启停; ⏎⏎glm⏎ + s = 改 fable 槽模型
     再保存——Task 4/5 加的就地操作里挑几个真的打一次假后端; 3 = 虚拟模型页, Task 6 起是真页面:
     l 从 Models 进 Members、J 把第一条订阅下移一位造一个草稿、s 保存——真的打一次假后端的
     `update_virtual_model`; 4 = 实时路由, 仍是占位, 顶掉原来 3 占的那个断言);
  3. 断言: 退出码 0、进出过备用屏幕、几个页面的关键文字 (含就地操作的 toast 文案) 都出现过、
     空闲 2 秒几乎不输出 (按需重绘)。

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


def sub(sid, name, state, dispatchable, usage=None, cooldown=None):
    # provider_id / base_url / auth_type / model_slots 是 Task 2 加的订阅详情字段, dto::Subscription
    # 里没有 #[serde(default)], 缺了任何一个都会让 list_subscriptions 反序列化失败, 订阅页 (Task 3
    # 起是真页面) 整页转圈圈。
    return {"id": sid, "display_name": name, "provider_display_name": "p", "enabled": True, "state": state,
            "cooldown_until": cooldown, "last_error_message": None, "is_dispatchable": dispatchable,
            "quota_usage": [usage] if usage else [],
            "provider_id": "p", "base_url": "https://example.invalid", "auth_type": "api_key",
            "model_slots": {"fable": "d", "opus": "a", "sonnet": "b", "haiku": "c"}}


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
        sub("2", "Kimi 备用", "rate_limited", False, quota(100, 91), NOW_MS + 42000),
        sub("3", "示例中转", "auth_failed", False),
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
    # Task 6: 5 个虚拟模型, 后端固定顺序; subscription_ids 引用上面三条假订阅。
    "list_virtual_models": [
        {"name": "model-fable", "mode": "sequential", "subscription_ids": ["1", "2"]},
        {"name": "model-opus", "mode": "round_robin", "subscription_ids": ["1"]},
        {"name": "model-sonnet", "mode": "sticky", "subscription_ids": ["1", "2", "3"]},
        {"name": "model-haiku", "mode": "sequential", "subscription_ids": []},
        {"name": "model-fallback", "mode": "sequential", "subscription_ids": ["3"]},
    ],
}

# 去掉转义序列之后必须出现过的文字
EXPECT = [
    "总览", "1,284", "98.6%", "智谱主号", "已连接", "订阅 (3)", "备注名", "此页面将在后续版本提供", "键位",
    "连接正常",  # test_connection 成功的 toast
    "已停用",  # set_subscription_enabled 的 toast (Kimi 备用被 e 停用)
    "槽位已保存",  # update_subscription 成功的 toast (Task 5: ⏎⏎ 改模型 ⏎ s 保存)
    "虚拟模型",  # Task 6: 虚拟模型页标题 (标签栏一直可见, 这里顺带确认真进过这一页)
    "已保存",  # update_virtual_model 成功的 toast (Task 6: l 进 Members、J 重排、s 保存)
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
        elif name == "update_virtual_model":
            # 同样真的改一下内存里的条目, 好让 s 之后的 `Cmd::Fetch(VirtualModels)` 补拉看到新顺序 /
            # 新模式。`input` 是 `{mode, subscription_ids}` (`runtime.rs::call_mutation` 拼的形状)。
            # 返回 JSON null (对应 Rust 端的 `()`, 与 `set_subscription_enabled` 同一套约定)。
            req = json.loads(raw or b"{}")
            input_ = req.get("input", {})
            for vm in DATA["list_virtual_models"]:
                if vm["name"] == req.get("name"):
                    vm["mode"] = input_.get("mode", vm["mode"])
                    vm["subscription_ids"] = input_.get("subscription_ids", vm["subscription_ids"])
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

    pump(3.0)  # 启动动效 + 首次加载 + 1.5s 时的状态变更事件
    # 2 = 订阅页 (真页面, 期待「订阅 (3)」「备注名」); j = 选中第二条 "Kimi 备用";
    # t = 测试连接 (等够 0.8s 让假后端的响应 + toast + 重拉列表都跑完), e = 就地启停 (同样等 0.8s);
    # ? / Esc = 帮助弹窗开关; 此时仍在订阅页且选中 "Kimi 备用":
    #   ⏎ 进详情 (焦点落在 fable 槽) → ⏎ 打开 fable 槽的模型 picker (输入框预填当前值 "d") →
    #   输入 "glm" (追加在预填值后面, 变成 "dglm", 够用了——这条冒烟不关心具体模型名) →
    #   ⏎ 选中「使用「dglm」」这一行, 写进草稿 → s 保存 (真的打一次假后端的 update_subscription,
    #   等够 0.8s 让响应 + toast + 重拉列表跑完);
    # 3 = 虚拟模型页 (Task 6 起是真页面, 默认选中 model-fable / Models 焦点):
    #   l 切到 Members 焦点 (选中 model-fable 的成员列表) → J 把第一条订阅下移一位 (造一个草稿) →
    #   s 保存 (真的打一次假后端的 update_virtual_model, 等够 0.8s 让响应 + toast + 重拉都跑完);
    # 4 = 实时路由 (仍占位, 期待「此页面将在后续版本提供」, 顶掉原来 3 占的那个断言); 1 = 回总览。
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
        (b"s", 0.8),
        (b"3", 0.6),
        (b"l", 0.4),
        (b"J", 0.4),
        (b"s", 0.8),
        (b"4", 0.6),
        (b"1", 0.6),
    ):
        os.write(fd, keys)
        pump(wait)
    # 让四条 toast (t 的「连接正常」、e 的「已停用」、订阅页 s 的「槽位已保存」、虚拟模型页 s 的
    # 「已保存」) 彻底放完再量「空闲」: toast 一条只能显示 3s (`toast::LIFETIME_MS`) + 300ms 消散
    # 动效, 后一条还要等前一条弹出队列才轮到它显示 (`MAX_TOASTS=4`, 四条都不会被挤掉)——四条顺序
    # 显示大约要 13s 才能全部放完 (从第一条 t 的 toast 大约 t≈5s 开始展示算起), 不等够的话空闲窗口
    # 会撞上消散动效的 60fps 重画, 把「按需重绘」误判成一直在跑 (Task 4 时曾经在两条 toast 上踩过
    # 这个坑, 从 260 字节涨到 1291 字节; Task 6 加了第四条 toast, 这里把 settle pump 从 5s 延到 9s
    # 留够余量)。
    pump(9.0)
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

    raw = out.decode("utf-8", "replace")
    text = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", raw)
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
    # 空闲时只有 250ms tick 带来的零星重绘 (冷却倒计时每秒变一格)。几 KB 以上说明在持续全速重画。
    if idle_bytes > 4000:
        failures.append(f"空闲 2 秒输出了 {idle_bytes} 字节, 按需重绘失效")

    print(f"输出 {len(out)} 字节, 空闲 2 秒 {idle_bytes} 字节")
    if failures:
        sys.exit("冒烟失败:\n  " + "\n  ".join(failures))
    print("冒烟通过")


if __name__ == "__main__":
    main()
