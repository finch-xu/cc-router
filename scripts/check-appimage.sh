#!/usr/bin/env bash
# 以「root 解包、换一个普通用户启动」的方式检查 AppImage, 模拟 firejail --appimage 与
# AppImageHub 的收录测试。正常双击运行走 FUSE 挂载, 只要有任意执行位就放行, 所以
# 权限问题在开发机上看不出来 (tauri-apps/tauri#16155: 启动器 AppRun.wrapped 被打成 0770)。
# 另外拦住两类白屏: 包里自带 libwayland (新系统 Mesa 加载失败), 以及窗口出现后渲染进程退出。
#
# 用法: scripts/check-appimage.sh <path/to/app.AppImage>
# 依赖: sudo, squashfs-tools (unsquashfs, 需 4.5+ 支持 -o), xvfb, x11-utils (xwininfo)
# 只在 CI 的 Linux runner 上跑; 会创建系统用户 ccr-appimage-check 并占用 DISPLAY :99。

set -euo pipefail

appimage="$(readlink -f "${1:?usage: check-appimage.sh <AppImage>}")"
window_title="cc-router"
wait_secs=45
check_user="ccr-appimage-check"
# 解到 /opt 而不是 mktemp: mktemp 目录是 0700, 换了用户连目录都进不去, 测不出真问题
root="/opt/ccr-appimage-check"

# squashfs 紧跟在 ELF 运行时之后, 起点 = 节头表末尾 = e_shoff + e_shentsize * e_shnum。
# 不用 `--appimage-offset`: 那要求能执行 AppImage 本身, qemu-user 认不出运行时头里的 AI 魔数。
read_le() { od -An -t "u$2" -j "$1" -N "$2" "$appimage" | tr -d ' '; }
offset=$(( $(read_le 40 8) + $(read_le 58 2) * $(read_le 60 2) ))
if [ "$(head -c $((offset + 4)) "$appimage" | tail -c 4)" != "hsqs" ]; then
  echo "::error::在偏移 $offset 处没有找到 squashfs 魔数, 不是预期的 type 2 AppImage"
  exit 1
fi
sudo rm -rf "$root"
# 以 root 解包: 保留镜像里的 root 属主与原始权限位, 与 firejail 的 root 挂载等价
sudo unsquashfs -q -no-progress -d "$root" -o "$offset" "$appimage" >/dev/null

echo "== 1/3 不得自带 libwayland (新系统 Mesa 需要更新的 libwayland, 自带旧版会导致白屏)"
bundled="$(sudo find "$root" -name 'libwayland-*.so*' -printf '%P\n')"
if [ -n "$bundled" ]; then
  echo "::error::AppImage 自带了 libwayland, 检查 Build 步骤的 LINUXDEPLOY_EXCLUDED_LIBRARIES:"
  echo "$bundled"
  exit 1
fi
echo "ok"

echo "== 2/3 权限扫描: 所有文件须对其他用户可读, 可执行文件须对其他用户可执行"
bad="$(sudo find "$root" -type f \( ! -perm -o=r -o \( -perm -u=x ! -perm -o=x \) \) -printf '%M %u %P\n')"
if [ -n "$bad" ]; then
  echo "::error::AppImage 内有文件对其他用户不可读或不可执行, firejail / AppImageHub 下会 Permission denied:"
  echo "$bad"
  exit 1
fi
echo "ok"

echo "== 3/3 以用户 $check_user 启动, ${wait_secs}s 内须出现标题为 \"$window_title\" 的窗口, 且渲染进程存活"
id "$check_user" >/dev/null 2>&1 || sudo useradd --create-home "$check_user"
# -ac: 关闭 X 访问控制, 让另一个用户能连上这个 display
xvfb_log="$(mktemp)"
Xvfb :99 -screen 0 1280x800x24 -ac >"$xvfb_log" 2>&1 &
xvfb_pid=$!
cleanup() {
  sudo pkill -u "$check_user" >/dev/null 2>&1 || true
  kill "$xvfb_pid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

log="$(mktemp)"

# 失败时打印 Xvfb 的状态与输出: GTK 只会说 "Failed to initialize GTK", 连不上 display 的原因在这里
dump_xvfb() {
  if kill -0 "$xvfb_pid" 2>/dev/null; then
    echo "---- Xvfb 仍在运行 (pid $xvfb_pid) ----"
  else
    echo "---- Xvfb 已退出 ----"
  fi
  tail -n 30 "$xvfb_log"
}

# 先等 Xvfb 就绪, 再以检查用户的身份连一次: 区分「X 没起来」与「换了用户连不上」
for _ in $(seq 1 20); do
  DISPLAY=:99 xwininfo -root >/dev/null 2>&1 && break
  sleep 0.5
done
if ! sudo -u "$check_user" env -i DISPLAY=:99 xwininfo -root >/dev/null 2>&1; then
  echo "::error::用户 $check_user 连不上 Xvfb :99"
  dump_xvfb
  exit 1
fi

started=$SECONDS
# 日志由当前 shell 重定向写入, 应用进程本身不需要对它有权限
# shellcheck disable=SC2024
# 与 AppImageHub 的测试环境一致: Xvfb 没有 GPU, WebKitGTK 的 DMABUF / 合成模式会因 EGL 报错退出
sudo -u "$check_user" env -i \
  HOME="/home/$check_user" PATH=/usr/local/bin:/usr/bin:/bin DISPLAY=:99 LANG=C.UTF-8 \
  WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 \
  "$root/AppRun" >"$log" 2>&1 &
app_pid=$!

result="timeout"
for _ in $(seq 1 "$wait_secs"); do
  sleep 1
  if ! kill -0 "$app_pid" 2>/dev/null; then
    result="exited"
    break
  fi
  if DISPLAY=:99 xwininfo -root -tree 2>/dev/null | grep -qF "\"$window_title\""; then
    result="window"
    break
  fi
done

if [ "$result" != "window" ]; then
  if [ "$result" = "exited" ]; then
    echo "::error::应用在出现窗口之前就退出了 (启动后 $((SECONDS - started))s)"
  else
    echo "::error::${wait_secs}s 内没有出现 \"$window_title\" 窗口"
  fi
  echo "---- 应用输出 (末尾 60 行) ----"
  tail -n 60 "$log"
  dump_xvfb
  exit 1
fi

# 白屏时窗口照样会出现, 区别是 WebKitWebProcess 已退出 (典型输出: EGL_BAD_PARAMETER. Aborting...)
sleep 5
if ! pgrep -u "$check_user" -f WebKitWebProcess >/dev/null; then
  echo "::error::窗口出现了, 但 WebKitWebProcess 已退出, 用户看到的会是白屏"
  echo "---- 应用输出 (末尾 60 行) ----"
  tail -n 60 "$log"
  exit 1
fi
echo "ok: 窗口已出现, 渲染进程存活"
