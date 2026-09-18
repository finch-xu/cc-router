//! 把 `cc-router-tui` 放到用户的 PATH 上 (设置页「添加到 PATH」按钮的后端, spec §7.2)。
//!
//! 三条原则: **不改 shell 配置文件**; **不自己处理密码** (macOS 提权交给系统授权对话框);
//! **只动自己创建的东西** (目标位置上不是我们的文件就不覆盖、不删除)。
//!
//! 每个平台一种做法:
//!   macOS          符号链接 `/usr/local/bin/cc-router-tui` → `.app` 里的 sidecar (app 更新后自动跟上)
//!   Windows        把安装目录追加进**用户级** PATH (直接读写注册表值, 保留 REG_EXPAND_SZ 类型)
//!   Linux AppImage 复制到 `~/.local/bin/` (挂载点路径每次启动都变, 链接没有意义)
//!   Linux deb      sidecar 本来就在 `/usr/bin`, 什么都不用做
//!
//! 文件的上半部分是纯函数 (全平台编译、全平台可测); 真正碰系统的部分在最下面按平台分开。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 一键安装在这台机器上是哪种做法。前端据此选说明文字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// macOS: 符号链接, 目录不可写时会弹系统授权框
    Symlink,
    /// Windows: 用户级 PATH, 已打开的终端要重开
    UserPath,
    /// Linux AppImage / 其它非系统位置: 复制到 ~/.local/bin, 不随 app 更新
    Copy,
    /// 已经在系统 PATH 上 (deb 装到 /usr/bin), 不需要按钮
    System,
    /// 此构建没有 sidecar, 无从安装
    Unavailable,
}

/// 为什么现在不能安装。前端负责翻译成一句话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallBlocked {
    /// macOS Gatekeeper 的 App Translocation: app 从下载目录直接运行, 路径是临时的
    Translocated,
    /// app 还在 DMG (`/Volumes/...`) 里
    OnDiskImage,
    /// 目标位置上已经有一个不是我们创建的同名文件
    Occupied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallStatus {
    pub kind: InstallKind,
    pub in_path: bool,
    /// 已安装时的位置 (链接路径 / PATH 条目 / 复制目标)
    pub installed_at: Option<String>,
    pub blocked: Option<InstallBlocked>,
    /// 仅 `Copy`: `~/.local/bin` 不在当前 PATH 里, 前端要提示用户自己加一行 export
    pub local_bin_off_path: bool,
}

impl InstallStatus {
    pub fn unavailable() -> Self {
        Self { kind: InstallKind::Unavailable, in_path: false, installed_at: None, blocked: None, local_bin_off_path: false }
    }

    pub fn can_install(&self) -> bool {
        matches!(self.kind, InstallKind::Symlink | InstallKind::UserPath | InstallKind::Copy) && self.blocked.is_none()
    }
}

/// 操作到底做没做: `Cancelled` 只在 macOS 提权对话框被用户点了「取消」时出现, 其余成功路径都是 `Done`。
/// command 层据此决定要不要给前端一条「已取消」的中性提示, 而不是把取消误报成失败或悄无声息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Cancelled,
}

pub type InstallResult = Result<Outcome, String>;

// ───────────────────────── 纯函数: macOS ─────────────────────────

/// `.app` 是否直接坐在某个卷的根目录上 (`/Volumes/<vol>/<X>.app/...`, 典型的「还没拖出 DMG」)。
/// 挂载的外置硬盘上装了 `Applications` 目录再放 `.app` (`/Volumes/SSD/Applications/x.app/...`)
/// 是合法安装, 不算 —— 判断标准是 `.app` 前面只隔着一层卷名。
/// 用 `to_string_lossy` 而不是 `to_str`: 后者对非 UTF-8 分量返回 `None`, `filter_map` 会把它整段
/// 丢掉, 后面所有分量的下标就全部往前移一位——这个判断全靠下标, 移位会让好端端的路径判断错
/// (fix-2 F7)。`.app` 后缀按 ASCII 大小写不敏感比较 (`cc-router.APP` 也算)。
fn is_on_disk_image_at_volume_root(sidecar: &Path) -> bool {
    let comps: Vec<std::borrow::Cow<'_, str>> =
        sidecar.components().map(|c| c.as_os_str().to_string_lossy()).collect();
    // comps[0] 是根 "/"; [1] 应为 "Volumes"; [3] (卷名之后紧跟的一段) 若以 ".app" 结尾就是卷根。
    comps.get(1).is_some_and(|c| c.as_ref() == "Volumes")
        && comps.get(3).is_some_and(|c| c.to_ascii_lowercase().ends_with(".app"))
}

pub fn macos_blocked(sidecar: &Path) -> Option<InstallBlocked> {
    let s = sidecar.to_string_lossy();
    if s.contains("/AppTranslocation/") {
        Some(InstallBlocked::Translocated)
    } else if is_on_disk_image_at_volume_root(sidecar) {
        Some(InstallBlocked::OnDiskImage)
    } else {
        None
    }
}

/// 目标位置上现在是什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    Absent,
    /// 指向当前 sidecar 的符号链接
    Ours,
    /// 指向**别处**的 `cc-router-tui` 的符号链接 (app 挪过位置留下的旧链接): 可以安全替换 / 删除
    StaleOurs,
    /// 别的东西: 不碰
    Foreign,
}

pub fn link_state(link: &Path, sidecar: &Path) -> LinkState {
    match std::fs::read_link(link) {
        Ok(target) if target == sidecar => LinkState::Ours,
        // 有意放宽: 只要符号链接指向的路径**文件名**是 cc-router-tui, 不管目标目录是哪, 就认成
        // StaleOurs——不要求非得在 `.app/Contents/MacOS/` 下面。替换一个符号链接不会丢用户数据
        // (跟 Linux 的整份复制不一样, 那边错判的代价是真的覆盖/删掉一个文件, 所以才要 marker 那套
        // 更严格的校验); 反过来如果要求必须匹配 `.app` 内部路径, 同一台机器上一个 dev 构建
        // (target/debug/cc-router-tui) 和一次正式安装会互相把对方的链接当成「别人的」, 谁也点不动
        // 「添加到 PATH」(fix-2 F8)。
        Ok(target) if target.file_name() == sidecar.file_name() => LinkState::StaleOurs,
        Ok(_) => LinkState::Foreign,
        // 不是符号链接: 存在即是别人的文件
        Err(_) if link.symlink_metadata().is_ok() => LinkState::Foreign,
        Err(_) => LinkState::Absent,
    }
}

/// 安装时对目标位置做什么。把决策从平台实现里拆出来, 单独测试——`LinkState::Foreign` 那一支
/// 「不碰」曾经只在平台 `install`/`uninstall` 里硬编码, 删掉那一行整个测试套件照样绿, 见 fix-1 R3。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPlan {
    AlreadyDone,
    Create,
    Refuse,
}

pub fn install_plan(state: &LinkState) -> LinkPlan {
    match state {
        LinkState::Ours => LinkPlan::AlreadyDone,
        LinkState::Absent | LinkState::StaleOurs => LinkPlan::Create,
        LinkState::Foreign => LinkPlan::Refuse,
    }
}

/// true = 该删; 只有 `Ours` / `StaleOurs` (我们自己放的东西) 才删。
pub fn uninstall_plan(state: &LinkState) -> bool {
    matches!(state, LinkState::Ours | LinkState::StaleOurs)
}

/// POSIX shell 单引号转义: 整体包一层 `'…'`, 内部的 `'` 写成 `'\''`。
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// 放进 AppleScript 字符串字面量 (`"…"`) 之前的转义: 只有反斜杠和双引号有特殊含义。
pub fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// `osascript -e <这个>`: 以管理员权限跑一段 shell。弹的是**系统**授权框, app 不接触密码。
pub fn admin_script(shell: &str) -> String {
    format!("do shell script {} with administrator privileges", applescript_quote(shell))
}

/// 提权脚本用绝对路径调 `mkdir`/`ln`/`rm`, 不吃 `PATH`——这段是要跑在 root 下的, 不能让
/// 一个被篡改的 `PATH` 决定实际执行的是哪个二进制 (fix-2 F4)。
/// 授权框可能停留好几分钟, 这段时间里目标位置可能被别的东西占了: 脚本自己在真正动手前
/// **重新检查一次** (check-then-act 的窗口不能只靠 Rust 侧在弹框之前判断一次的 `LinkState`)。
/// `[ ! -e L ]`(不存在) 或 `[ -L L ]`(是符号链接, 不管指向哪, 都可以安全地 `ln -sfn` 覆盖) 才继续,
/// 否则 `exit 1` 拒绝——注意 `-e` 对失效的悬空符号链接是 false, 所以必须显式再判一次 `-L`。
pub fn symlink_shell(sidecar: &Path, link: &Path) -> String {
    let dir = link.parent().unwrap_or(Path::new("/"));
    let l = sh_quote(&link.to_string_lossy());
    let d = sh_quote(&dir.to_string_lossy());
    let s = sh_quote(&sidecar.to_string_lossy());
    format!("[ ! -e {l} ] || [ -L {l} ] || exit 1; /bin/mkdir -p {d} && /bin/ln -sfn {s} {l}")
}

/// 同样重新检查一次: 只有此刻仍是符号链接才删, 避免授权框开着的这段时间里目标被换成了别人的文件。
pub fn unlink_shell(link: &Path) -> String {
    let l = sh_quote(&link.to_string_lossy());
    format!("[ -L {l} ] || exit 0; /bin/rm -f {l}")
}

/// osascript 在用户点了「取消」时以非零退出, stderr 里带 `(-128)` (userCanceledErr) 作为**尾巴**。
/// 只匹配带括号的完整错误码——裸的 `-128` 可能出现在别的地方 (路径名、别的错误码如 `-12800`),
/// 那样会把真错误误判成取消, 见 fix-1 R5。
pub fn is_user_cancel(stderr: &str) -> bool {
    stderr.trim().ends_with("(-128)")
}

// ───────────────────────── 纯函数: Windows PATH ─────────────────────────

/// 比较用的规范形式: 去首尾空白与引号、去尾部 `\` `/`、转小写 (Windows 路径大小写不敏感)。
fn norm_dir(s: &str) -> String {
    s.trim().trim_matches('"').trim_end_matches(['\\', '/']).to_lowercase()
}

pub fn path_entries(path: &str) -> Vec<&str> {
    path.split(';').filter(|e| !e.trim().is_empty()).collect()
}

pub fn path_contains(path: &str, dir: &str) -> bool {
    let want = norm_dir(dir);
    path_entries(path).iter().any(|e| norm_dir(e) == want)
}

/// 追加 `dir`; 已经有了就原样返回 (不重排、不去重别人的条目 —— 只动自己的那一项)。
pub fn path_with(path: &str, dir: &str) -> String {
    if path_contains(path, dir) {
        return path.to_string();
    }
    if path_entries(path).is_empty() {
        return dir.to_string();
    }
    format!("{};{dir}", path.trim_end_matches(';'))
}

/// 去掉与 `dir` 相等的条目 (可能不止一条), 其余条目保持原文与顺序。
pub fn path_without(path: &str, dir: &str) -> String {
    let want = norm_dir(dir);
    path_entries(path).into_iter().filter(|e| norm_dir(e) != want).collect::<Vec<_>>().join(";")
}

/// `powershell.exe` 的绝对路径: 不靠 PATH 解析——PATH 顺序可能被篡改, 或者压根没把
/// System32 放进去。用 `%SystemRoot%` 拼绝对路径, 拿不到就退回 `C:\Windows`。
/// 用字符串拼接而不是 `PathBuf::join`: 后者按**编译目标**的原生分隔符插入 (这个纯函数全平台编译、
/// 全平台跑单测, 在 unix 宿主机上 join 会插 `/` 而不是 `\`), 显式拼字符串才能保证结果总是 Windows 路径。
pub fn powershell_path(system_root: Option<&std::ffi::OsStr>) -> PathBuf {
    let root = system_root
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| r"C:\Windows".to_string());
    let root = root.trim_end_matches(['\\', '/']);
    PathBuf::from(format!(r"{root}\System32\WindowsPowerShell\v1.0\powershell.exe"))
}

// `powershell -Command "<script>"` 默认的 `$ErrorActionPreference` 是 `Continue`: 一条语句抛错,
// 后面的语句照样跑, 退出码只看最后一条语句成不成功。这会让「读注册表失败」悄悄变成「读到空 PATH」,
// 「写注册表失败」悄悄变成「整体成功」——必须每段脚本第一句就把它改成 `Stop` (fix-2 F1)。
// **但 `$ErrorActionPreference='Stop'` 只管**「非终止性」错误 (cmdlet 走 `Write-Error` 报的那类);
// .NET 方法调用 (`$k.GetValue(...)`、`$k.SetValue(...)`) 抛的是 CLR 异常, 是不是被当前引擎版本
// 升级成终止性错误并不是稳定契约——真出现「没被升级」的情况, `$v` 会是 `$null`, 后面的语句照样
// 跑完、`CCR1:`+空 base64 正常打印、退出码 0, install 会拿着一个「看起来是空 PATH」的假值把用户
// PATH 整个覆盖成我们这一条 (fix-3 B1)。修法是显式 `try{…}catch{…;exit 1}` 兜底, 不依赖
// `$ErrorActionPreference` 的隐式升级行为。`try` 块外层仍然先设 `$ErrorActionPreference='Stop'`——
// 双保险, 也让 cmdlet 类的非终止性错误照样被升级进 catch。
// 两段脚本都不能含 `"` 字符: Rust 侧用 `-Command <script>` 传参, Windows 的参数拼接/转义规则
// 会把内嵌的 `"` 搞复杂, 干脆全程只用单引号写 PowerShell 字符串字面量, 从根源上绕开。

/// 读用户级 `HKCU\Environment\Path`。`DoNotExpandEnvironmentNames` 保留 `%USERPROFILE%\…` 原文,
/// 不展开也不改类型。`OpenSubKey` 拿不到 `Environment` 键是真正的错误 (`throw`, 走 `catch` 非 0
/// 退出) —— 与「有键但没有 Path 值」(`GetValue` 的默认值 `''`, 合法的空 PATH) 是两件不同的事,
/// 不能混。**不碰 `[Console]::OutputEncoding`**: 我们用 `CREATE_NO_WINDOW` + 管道起的进程没有
/// 真正的控制台, 读/写这个属性在这类「无控制台」进程里可能直接抛 `IOException: The handle is
/// invalid`——曾经把它当第一条可能失败的语句, 一旦抛错整个 Windows 功能直接不可用 (fix-3 B2)。
/// 改成把值编码成 **base64**: `[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($v))`
/// 只产出 ASCII 字符, 天然不受控制台代码页影响, 也不需要碰 `OutputEncoding`。
/// 输出前缀固定标记 `CCR1:`, 供 `decode_ps_read` 校验「这确实是我们的脚本跑完的」而不是半截输出。
pub const PS_READ: &str = "$ErrorActionPreference='Stop';\
    try{\
    $k=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment');\
    if(-not $k){throw 'Environment key missing'};\
    $v=[string]$k.GetValue('Path','',[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames);\
    [Console]::Out.Write('CCR1:'+[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($v)))\
    }catch{[Console]::Error.Write($_.ToString());exit 1}";

/// 写回 `HKCU\Environment\Path`, 保留原有的值类型 (没有旧值时按 `ExpandString` 新建, 与
/// Windows 自己新建这个值时用的类型一致)。新值经环境变量 `CCR_NEW_PATH` 传入而不是拼进脚本文本,
/// 但 PowerShell 里读一个不存在/空的环境变量得到的是 `$null`, `SetValue('Path',$null,…)` 会直接
/// 抛错——`[string]$env:CCR_NEW_PATH` 强制转换成空字符串, 才能把「PATH 被清空」当成合法值写回,
/// 而不是让 WRITE 在这种边界情况下失败。最后广播一次 `WM_SETTINGCHANGE`(对不存在的变量
/// `SetEnvironmentVariable`), 新开的终端立即看到新 PATH。同 `PS_READ`: 全部包进
/// `try{…}catch{…;exit 1}`——`SetValue` 抛错不能被最后一条 `SetEnvironmentVariable` 的成功
/// 掩盖掉, 那样会汇报「已完成」但其实什么都没写进去 (fix-3 B1)。
pub const PS_WRITE: &str = "$ErrorActionPreference='Stop';\
    try{\
    $k=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment');\
    $kind=if($k.GetValueNames() -contains 'Path'){$k.GetValueKind('Path')}else{[Microsoft.Win32.RegistryValueKind]::ExpandString};\
    $v=[string]$env:CCR_NEW_PATH;\
    $k.SetValue('Path',$v,$kind);\
    [Environment]::SetEnvironmentVariable('CCR_TUI_PATH_REFRESH',$null,'User')\
    }catch{[Console]::Error.Write($_.ToString());exit 1}";

/// `PS_READ` 的输出解码。严格 UTF-8 (无效字节直接拒绝, 不用 `from_utf8_lossy` 悄悄换成 U+FFFD),
/// 去掉最多一个开头的 BOM, 再 `trim()`——这一步现在是安全的: 标记之后的内容是纯 ASCII 的 base64,
/// PowerShell 输出可能带的尾部换行不会混进真正的 PATH 值 (旧版本在这里特意不 trim, 因为那时候
/// 标记后面直接是原始 PATH 文本, trim 会啃掉用户 PATH 里故意留的尾部空格——现在这层保护移到了
/// base64 内部, 外层的 ASCII 包装可以放心 trim)。`CCR1:` 标记缺失说明脚本没跑完 / 跑的不是我们
/// 期望的脚本, 当错误处理。标记之后的内容按标准 base64 解码、再做一次严格 UTF-8 校验, 两步任一失败
/// 都是 `Err`, 不猜测、不用损坏的字节拼出一个「看起来对」的字符串。
pub fn decode_ps_read(stdout: &[u8]) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let s = String::from_utf8(stdout.to_vec()).map_err(|_| "读取用户 PATH 失败: 输出不是合法 UTF-8".to_string())?;
    let s = s.strip_prefix('\u{FEFF}').unwrap_or(s.as_str()).trim();
    let encoded = s.strip_prefix("CCR1:").ok_or_else(|| "读取用户 PATH 失败: 输出缺少标记".to_string())?;
    let bytes = STANDARD.decode(encoded).map_err(|e| format!("读取用户 PATH 失败: base64 解码失败: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "读取用户 PATH 失败: base64 内容不是合法 UTF-8".to_string())
}

// ───────────────────────── 纯函数: Linux ─────────────────────────

/// sidecar 在这些目录里 = 系统包管理器装的, 已经在 PATH 上。
pub fn is_system_bin(sidecar: &Path) -> bool {
    matches!(sidecar.parent().and_then(Path::to_str), Some("/usr/bin" | "/usr/local/bin" | "/bin"))
}

pub fn local_bin(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

/// `$PATH` (冒号分隔) 里有没有这个目录。
pub fn unix_path_contains(path_var: &str, dir: &Path) -> bool {
    path_var.split(':').any(|e| !e.is_empty() && Path::new(e.trim_end_matches('/')) == dir)
}

/// 两个文件字节是否相同; 任一读不到或长度不同直接 false (先比长度再读全部字节——sidecar 就几 MB, 可以接受)。
pub fn same_file_content(a: &Path, b: &Path) -> bool {
    let (ma, mb) = match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => (ma, mb),
        _ => return false,
    };
    if ma.len() != mb.len() {
        return false;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(ba), Ok(bb)) => ba == bb,
        _ => false,
    }
}

/// 复制安装 (Linux) 目标位置现在是什么, 按内容判定归属——不像 macOS 有符号链接可以直接问「指向谁」。
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyState {
    Absent,
    /// 内容与当前 sidecar 字节相同
    Ours,
    /// 内容不同, marker 也没有 / 对不上: 不是我们的东西, 不碰
    Foreign,
    /// 内容跟当前 sidecar 不一致, 但 marker 记录的 (长度, mtime) 与它现在的 metadata 完全对得上——
    /// app 更新后 sidecar 换了字节, 这其实是自家旧副本。覆盖 / 删除都安全。
    StaleOurs,
}

/// 与 `target` 同目录的标记文件: 证明 `target` 是 `copy_install` 放的, 不是用户自己的同名文件。
/// 内容是 `copy_install` 写完之后立刻读回的 `"<字节数> <mtime 秒>\n"`——只留一个空文件 (v1 做法)
/// 有个漏洞: 用户手动删掉了我们的副本、自己放了同名文件, 空 marker 还留在原地, 会把这份**别人的**
/// 文件误判成「自家旧副本」从而允许覆盖 / 删除 (fix-2 F5 修的就是这个)。记录长度 + mtime 之后,
/// 只有当前文件的 metadata 跟 marker 里记的完全一致才认——用户自己放的文件几乎不可能凑巧撞上。
/// 残余风险: marker 写失败 (磁盘满 / 权限问题, 极端情况) 时这次复制仍然成功 (`copy_install` 不因此
/// 报错), 但下一次会把刚装好的这份也误判成 `Foreign`, 需要用户手动处理——比反过来 (把用户文件
/// 误判成自家的从而覆盖/删除) 安全得多, 是有意的取舍。marker 读写都是 best-effort, `status()` 只读
/// 不写不删。
#[cfg(unix)]
pub const COPY_MARKER: &str = ".cc-router-tui.installed-by-cc-router";

/// 文件的 (字节数, mtime 的 unix 秒数)。拿不到 metadata / mtime 早于 unix epoch (几乎不可能, 但
/// 保守处理) 都返回 `None`——调用方据此把 `copy_state` 判成 `Foreign`, 不是 panic 或者假装匹配。
#[cfg(unix)]
fn file_len_and_mtime(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some((meta.len(), mtime))
}

#[cfg(unix)]
fn marker_content(len: u64, mtime: u64) -> String {
    format!("{len} {mtime}\n")
}

/// 解析 marker 文件内容; 格式不对 (字段数不对 / 不是数字) 都是 `None`, 而不是 panic 或凑一半数据。
#[cfg(unix)]
fn parse_marker(content: &str) -> Option<(u64, u64)> {
    let mut parts = content.split_whitespace();
    let len = parts.next()?.parse().ok()?;
    let mtime = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // 多出字段: 当成格式不认识的垃圾, 不猜。
    }
    Some((len, mtime))
}

#[cfg(unix)]
pub fn copy_state(target: &Path, sidecar: &Path) -> CopyState {
    let meta = match target.symlink_metadata() {
        Ok(m) => m,
        Err(_) => return CopyState::Absent,
    };
    if !meta.is_file() {
        // 符号链接 / 目录: 都不是 copy_install 会放的东西, 一律当外人的, 不碰。
        return CopyState::Foreign;
    }
    if same_file_content(target, sidecar) {
        return CopyState::Ours;
    }
    let current = match file_len_and_mtime(target) {
        Some(v) => v,
        None => return CopyState::Foreign,
    };
    let marker_matches = target
        .parent()
        .and_then(|d| std::fs::read_to_string(d.join(COPY_MARKER)).ok())
        .and_then(|s| parse_marker(&s))
        .is_some_and(|recorded| recorded == current);
    if marker_matches {
        CopyState::StaleOurs
    } else {
        CopyState::Foreign
    }
}

/// `CopyState` → `(in_path, blocked)` 的映射, 从 Linux `platform::status` 里拆出来单独测试
/// (原来内嵌在平台代码里, 删掉 `Foreign` 那一支照样能过全套测试——同 fix-1 R3 对 macOS 的整改)。
#[cfg(unix)]
pub fn copy_status_fields(state: &CopyState) -> (bool, Option<InstallBlocked>) {
    match state {
        CopyState::Ours => (true, None),
        CopyState::Foreign => (false, Some(InstallBlocked::Occupied)),
        CopyState::StaleOurs | CopyState::Absent => (false, None),
    }
}

// ───────────────────────── 文件操作 (unix 通用, 目标路径由调用方给, 所以能用临时目录测) ─────────────────────────

/// 不提权建符号链接; 目标位置上是我们的旧链接就先删掉。`Foreign` 由调用方事先挡掉。
#[cfg(unix)]
pub fn link_direct(sidecar: &Path, link: &Path) -> std::io::Result<()> {
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(link)?;
    }
    std::os::unix::fs::symlink(sidecar, link)
}

/// 复制 sidecar 到 `target` 并给可执行位。`Foreign` 由 `copy_state` 挡掉——拒绝覆盖不是我们放的文件。
#[cfg(unix)]
pub fn copy_install(sidecar: &Path, target: &Path) -> InstallResult {
    use std::os::unix::fs::PermissionsExt;
    if copy_state(target, sidecar) == CopyState::Foreign {
        return Err(format!("{} 已存在且不是 cc-router 创建的", target.display()));
    }
    let dir = target.parent().ok_or("无法确定目标目录")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {}: {e}", dir.display()))?;
    // 先删再拷: 覆盖已存在的文件会沿用它的权限, 而且正在运行的旧副本不能被原地改写 (ETXTBSY)。
    let _ = std::fs::remove_file(target);
    std::fs::copy(sidecar, target).map_err(|e| format!("复制到 {}: {e}", target.display()))?;
    std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    // 写 marker: 记录**刚写完这一份**的 (长度, mtime), 之后 app 更新、sidecar 字节变了, 仍能凭这两个
    // 数字认出这是自家旧副本 (CopyState::StaleOurs)。best-effort——写失败不能把一次成功的复制变成
    // Err (旧行为), 代价是下次可能被误判成 Foreign, 见 COPY_MARKER 文档注释里的取舍说明。
    if let Some((len, mtime)) = file_len_and_mtime(target) {
        let _ = std::fs::write(dir.join(COPY_MARKER), marker_content(len, mtime));
    }
    Ok(Outcome::Done)
}

/// 只删 `Ours` / `StaleOurs` (我们放的); `Foreign` / `Absent` 原样不动。
#[cfg(unix)]
pub fn copy_uninstall(target: &Path, sidecar: &Path) -> InstallResult {
    match copy_state(target, sidecar) {
        CopyState::Ours | CopyState::StaleOurs => {
            std::fs::remove_file(target).map_err(|e| format!("删除 {}: {e}", target.display()))?;
            if let Some(dir) = target.parent() {
                let _ = std::fs::remove_file(dir.join(COPY_MARKER));
            }
            Ok(Outcome::Done)
        }
        CopyState::Foreign | CopyState::Absent => Ok(Outcome::Done),
    }
}

// ───────────────────────── 碰系统的部分 ─────────────────────────

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::process::Command;

    const LINK: &str = "/usr/local/bin/cc-router-tui";

    pub fn status(sidecar: &Path) -> InstallStatus {
        let link = Path::new(LINK);
        let state = link_state(link, sidecar);
        let blocked = macos_blocked(sidecar).or((state == LinkState::Foreign).then_some(InstallBlocked::Occupied));
        InstallStatus {
            kind: InstallKind::Symlink,
            in_path: state == LinkState::Ours,
            installed_at: (state == LinkState::Ours).then(|| LINK.to_string()),
            blocked,
            local_bin_off_path: false,
        }
    }

    /// 以管理员权限跑 shell。用户取消 → `Cancelled` (不算错误, 调用方重新读状态即可)。
    fn run_as_admin(shell: &str) -> InstallResult {
        let out = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(admin_script(shell))
            .output()
            .map_err(|e| format!("无法调用 osascript: {e}"))?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            Ok(Outcome::Done)
        } else if is_user_cancel(&stderr) {
            Ok(Outcome::Cancelled)
        } else {
            Err(stderr.trim().to_string())
        }
    }

    pub fn install(sidecar: &Path) -> InstallResult {
        // sh_quote 只转义单引号, 挡不住一个以 "-" 开头的参数被 ln 当成选项解析——sidecar 永远应该是
        // current_exe() 边上拼出来的绝对路径, 不是绝对路径本身就说明调用方传错了, 直接拒绝 (fix-2 F4)。
        if !sidecar.is_absolute() {
            return Err("sidecar 路径不是绝对路径, 拒绝安装".to_string());
        }
        // Translocation / DMG 里跑的临时路径: 装了也没用 (下次启动路径就变了), 这里就近拦掉,
        // 不能只指望调用方 (command 层) 检查一次——直接调这个函数的路径也要经过同一道闸门。
        if macos_blocked(sidecar).is_some() {
            return Err("app 位于临时位置或磁盘映像里, 无法添加到 PATH".to_string());
        }
        let link = Path::new(LINK);
        match install_plan(&link_state(link, sidecar)) {
            LinkPlan::AlreadyDone => return Ok(Outcome::Done),
            LinkPlan::Refuse => return Err(format!("{LINK} 已存在且不是 cc-router 创建的")),
            LinkPlan::Create => {}
        }
        // 先不提权试一次: /usr/local/bin 对当前用户可写的机器 (装过 Homebrew 的 Intel Mac) 不用弹框。
        match link_direct(sidecar, link) {
            Ok(()) => Ok(Outcome::Done),
            Err(_) => {
                // 提权分支要把 sidecar 路径拼进 shell 字符串; 非 UTF-8 路径经 to_string_lossy 会被
                // 悄悄改写, 生成的命令可能对着错误路径操作, 必须在这里就拒绝, 不能让它混进 shell 字符串。
                sidecar.to_str().ok_or("sidecar 路径包含非 UTF-8 字符, 无法提权安装")?;
                run_as_admin(&symlink_shell(sidecar, link))
            }
        }
    }

    pub fn uninstall(sidecar: &Path) -> InstallResult {
        let link = Path::new(LINK);
        if !uninstall_plan(&link_state(link, sidecar)) {
            // 不是我们的: 不删
            return Ok(Outcome::Done);
        }
        match std::fs::remove_file(link) {
            Ok(()) => Ok(Outcome::Done),
            Err(_) => run_as_admin(&unlink_shell(link)),
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// 跑一段脚本, 原始 stdout 字节 (不在这里假设编码——`PS_READ` 自己把值编成 ASCII 的 base64
    /// 并加了 `CCR1:` 标记, 不依赖控制台代码页, 解码交给 `decode_ps_read`)。
    fn powershell(script: &str, new_path: Option<&str>) -> Result<Vec<u8>, String> {
        let exe = powershell_path(std::env::var_os("SystemRoot").as_deref());
        let mut cmd = Command::new(exe);
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]).creation_flags(CREATE_NO_WINDOW);
        if let Some(p) = new_path {
            // 新值走环境变量传进去, 不拼进脚本文本: 路径里的引号 / 分号 / $ 都不需要转义。
            cmd.env("CCR_NEW_PATH", p);
        }
        let out = cmd.output().map_err(|e| format!("无法调用 PowerShell: {e}"))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    /// `PS_READ` + 解码一步到位; READ 失败或输出解不出标记都直接 `Err`, 调用方 (`install`/`uninstall`)
    /// 据此绝不会带着一个「看起来是空 PATH 但其实是读失败」的值去算 `next` 再写回去 (fix-2 F1)。
    fn read_user_path() -> Result<String, String> {
        let raw = powershell(PS_READ, None)?;
        decode_ps_read(&raw)
    }

    fn install_dir(sidecar: &Path) -> Option<String> {
        sidecar.parent().map(|d| d.to_string_lossy().into_owned())
    }

    pub fn status(sidecar: &Path) -> InstallStatus {
        let dir = install_dir(sidecar);
        let user_path = match read_user_path() {
            Ok(p) => p,
            Err(e) => {
                // 读失败不能悄悄当成「没装」——那样用户已经装过的状态会在偶发的 PowerShell 抽风
                // 后被前端展示成「未添加」, 诱导用户再点一次「添加」。保底展示未添加 (不确定就不
                // 显示已装), 但把原因记进日志, 方便排查是不是宿主机 PowerShell 环境有问题。
                tracing::warn!(error = %e, "读取用户 PATH 失败, 状态展示为未添加");
                return InstallStatus {
                    kind: InstallKind::UserPath,
                    in_path: false,
                    installed_at: None,
                    blocked: None,
                    local_bin_off_path: false,
                };
            }
        };
        let in_path = dir.as_deref().is_some_and(|d| path_contains(&user_path, d));
        InstallStatus {
            kind: InstallKind::UserPath,
            in_path,
            installed_at: if in_path { dir } else { None },
            blocked: None,
            local_bin_off_path: false,
        }
    }

    pub fn install(sidecar: &Path) -> InstallResult {
        let dir = install_dir(sidecar).ok_or("无法确定安装目录")?;
        let current = read_user_path()?;
        let next = path_with(&current, &dir);
        if next == current {
            return Ok(Outcome::Done);
        }
        // 写之前把原值记进日志: WRITE 是覆盖式写整个 PATH 字符串, 一旦哪里算错, 这是用户唯一能
        // 从 app 日志里手动抄回去的记录。
        tracing::info!(old = %current, "修改用户 PATH 前的原值");
        powershell(PS_WRITE, Some(&next)).map(|_| Outcome::Done)
    }

    pub fn uninstall(sidecar: &Path) -> InstallResult {
        let dir = install_dir(sidecar).ok_or("无法确定安装目录")?;
        let current = read_user_path()?;
        let next = path_without(&current, &dir);
        if next == current {
            return Ok(Outcome::Done);
        }
        tracing::info!(old = %current, "修改用户 PATH 前的原值");
        powershell(PS_WRITE, Some(&next)).map(|_| Outcome::Done)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::*;

    fn home() -> Option<PathBuf> {
        std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
    }

    fn target() -> Option<PathBuf> {
        home().map(|h| local_bin(&h).join("cc-router-tui"))
    }

    pub fn status(sidecar: &Path) -> InstallStatus {
        if is_system_bin(sidecar) {
            return InstallStatus {
                kind: InstallKind::System,
                in_path: true,
                installed_at: Some(sidecar.to_string_lossy().into_owned()),
                blocked: None,
                local_bin_off_path: false,
            };
        }
        let target = target();
        let state = target.as_deref().map(|t| copy_state(t, sidecar)).unwrap_or(CopyState::Absent);
        let (in_path, blocked) = copy_status_fields(&state);
        let off_path = match (&target, std::env::var("PATH")) {
            (Some(t), Ok(p)) => !t.parent().is_some_and(|d| unix_path_contains(&p, d)),
            _ => false,
        };
        InstallStatus {
            kind: InstallKind::Copy,
            in_path,
            installed_at: target.filter(|_| in_path).map(|t| t.to_string_lossy().into_owned()),
            blocked,
            local_bin_off_path: off_path,
        }
    }

    pub fn install(sidecar: &Path) -> InstallResult {
        copy_install(sidecar, &target().ok_or("无法确定 HOME 目录")?)
    }

    pub fn uninstall(sidecar: &Path) -> InstallResult {
        match target() {
            Some(t) => copy_uninstall(&t, sidecar),
            None => Ok(Outcome::Done),
        }
    }
}

pub use platform::{install, status, uninstall};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translocated_and_disk_image_apps_are_blocked() {
        let t = Path::new("/private/var/folders/ab/T/AppTranslocation/1234/d/cc-router.app/Contents/MacOS/cc-router-tui");
        assert_eq!(macos_blocked(t), Some(InstallBlocked::Translocated));
        let v = Path::new("/Volumes/cc-router/cc-router.app/Contents/MacOS/cc-router-tui");
        assert_eq!(macos_blocked(v), Some(InstallBlocked::OnDiskImage));
        let ok = Path::new("/Applications/cc-router.app/Contents/MacOS/cc-router-tui");
        assert_eq!(macos_blocked(ok), None);
        // 外置硬盘上装了 Applications 目录: .app 不直接坐在卷根上, 是合法安装, 不拦。
        let external = Path::new("/Volumes/SSD/Applications/cc-router.app/Contents/MacOS/cc-router-tui");
        assert_eq!(macos_blocked(external), None);
        // 大小写不敏感: cc-router.APP 也算 .app 后缀 (fix-2 F7)。
        let upper = Path::new("/Volumes/X/cc-router.APP/Contents/MacOS/cc-router-tui");
        assert_eq!(macos_blocked(upper), Some(InstallBlocked::OnDiskImage));
    }

    // 唯一一个真的调用 `install`/`uninstall` 的测试: 只有在能确认要测的那条 return 排在**第一条
    // 语句**、之前没有任何 syscall 时才安全 (上面 `pub fn install` 的源码已经确认: 非绝对路径检查
    // 是函数体的第一行, 在 `macos_blocked`(纯字符串比较) 和 `link_state`(读, 不写) 之前就返回)。
    // 不要再加别的调用 `install`/`uninstall` 的测试——这两个函数在其它分支会碰真实的
    // `/usr/local/bin`、可能弹系统授权框, 不是 tempdir 能兜住的。
    #[cfg(target_os = "macos")]
    #[test]
    fn install_rejects_a_non_absolute_sidecar_path_before_touching_anything() {
        assert!(install(Path::new("relative/cc-router-tui")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn link_state_tells_ours_from_stale_from_foreign() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("app").join("cc-router-tui");
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar, b"bin").unwrap();
        let link = dir.path().join("cc-router-tui");

        assert_eq!(link_state(&link, &sidecar), LinkState::Absent);

        std::os::unix::fs::symlink(&sidecar, &link).unwrap();
        assert_eq!(link_state(&link, &sidecar), LinkState::Ours);

        // app 挪了位置: 链接还指着旧路径 (已不存在) —— 名字是我们的, 可以替换
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(dir.path().join("old").join("cc-router-tui"), &link).unwrap();
        assert_eq!(link_state(&link, &sidecar), LinkState::StaleOurs);

        // 指向别的程序的链接 / 普通文件: 不是我们的
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("/bin/ls", &link).unwrap();
        assert_eq!(link_state(&link, &sidecar), LinkState::Foreign);
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, b"someone else's").unwrap();
        assert_eq!(link_state(&link, &sidecar), LinkState::Foreign);
    }

    #[test]
    fn install_plan_matches_link_state() {
        assert_eq!(install_plan(&LinkState::Ours), LinkPlan::AlreadyDone);
        assert_eq!(install_plan(&LinkState::Absent), LinkPlan::Create);
        assert_eq!(install_plan(&LinkState::StaleOurs), LinkPlan::Create);
        assert_eq!(install_plan(&LinkState::Foreign), LinkPlan::Refuse);
    }

    #[test]
    fn uninstall_plan_only_deletes_links_that_are_ours() {
        assert!(uninstall_plan(&LinkState::Ours));
        assert!(uninstall_plan(&LinkState::StaleOurs));
        assert!(!uninstall_plan(&LinkState::Absent));
        assert!(!uninstall_plan(&LinkState::Foreign));
    }

    #[cfg(unix)]
    #[test]
    fn link_direct_replaces_a_stale_link_of_ours() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("cc-router-tui-real");
        std::fs::write(&sidecar, b"bin").unwrap();
        let link = dir.path().join("bin").join("cc-router-tui");
        assert!(link_direct(&sidecar, &link).is_err(), "目录不存在: 失败, 调用方据此走提权分支");

        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), &link).unwrap();
        link_direct(&sidecar, &link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), sidecar);
        link_direct(&sidecar, &link).unwrap(); // 再来一次也没事
        assert_eq!(std::fs::read(&link).unwrap(), b"bin");
    }

    #[cfg(unix)]
    #[test]
    fn copy_state_distinguishes_absent_ours_foreign_and_stale_ours() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("sidecar");
        std::fs::write(&sidecar, b"payload-v2").unwrap();
        let target = dir.path().join("target");

        assert_eq!(copy_state(&target, &sidecar), CopyState::Absent);

        std::fs::write(&target, b"payload-v2").unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::Ours);

        // 内容不同, 没有 marker: 用户自己的同名文件, 不是我们的
        std::fs::write(&target, b"someone else's binary").unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::Foreign);

        // marker 记录的 (长度, mtime) 与 target 现在的 metadata 完全对得上: 自家旧副本
        let (len, mtime) = file_len_and_mtime(&target).unwrap();
        std::fs::write(dir.path().join(COPY_MARKER), marker_content(len, mtime)).unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::StaleOurs);

        // marker 内容是垃圾 (解析不出两个数字): 不信它, 当 Foreign
        std::fs::write(dir.path().join(COPY_MARKER), b"not a marker\n").unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::Foreign);

        // 孤儿 marker: 数字能解析, 但跟 target 现在的 metadata 对不上 (target 内容换过): 不信它
        std::fs::write(dir.path().join(COPY_MARKER), marker_content(len + 1, mtime)).unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::Foreign);

        std::fs::remove_file(dir.path().join(COPY_MARKER)).unwrap();

        // 符号链接: 哪怕指向 sidecar 本身, 也不算 Ours (copy_install 只会放普通文件)
        std::fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(&sidecar, &target).unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::Foreign);

        // 目录也不碰
        std::fs::remove_file(&target).unwrap();
        std::fs::create_dir(&target).unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::Foreign);
    }

    #[cfg(unix)]
    #[test]
    fn orphan_marker_does_not_promote_a_foreign_file_to_stale_ours() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("sidecar");
        std::fs::write(&sidecar, b"v2").unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, b"user's own binary, unrelated to us").unwrap();
        // 孤儿 marker: 用户手动删过我们的旧副本、自己放了别的文件, marker 还留在原地,
        // 但记录的数字跟这份新文件的 metadata 对不上——fix-2 F5 修的就是这种情况。
        std::fs::write(dir.path().join(COPY_MARKER), marker_content(999_999, 1)).unwrap();

        assert_eq!(copy_state(&target, &sidecar), CopyState::Foreign);
        assert!(copy_install(&sidecar, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"user's own binary, unrelated to us");
    }

    #[cfg(unix)]
    #[test]
    fn copy_status_fields_matches_each_state() {
        assert_eq!(copy_status_fields(&CopyState::Ours), (true, None));
        assert_eq!(copy_status_fields(&CopyState::Foreign), (false, Some(InstallBlocked::Occupied)));
        assert_eq!(copy_status_fields(&CopyState::StaleOurs), (false, None));
        assert_eq!(copy_status_fields(&CopyState::Absent), (false, None));
    }

    #[cfg(unix)]
    #[test]
    fn copy_install_refuses_foreign_and_leaves_it_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("sidecar");
        std::fs::write(&sidecar, b"v2").unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, b"someone else's binary").unwrap();

        assert!(copy_install(&sidecar, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"someone else's binary");
        assert!(!dir.path().join(COPY_MARKER).exists(), "拒绝时不应该留下 marker");
    }

    #[cfg(unix)]
    #[test]
    fn copy_uninstall_only_touches_ours_and_stale_ours() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("sidecar");
        std::fs::write(&sidecar, b"v2").unwrap();

        // Foreign: 不碰
        let target = dir.path().join("target");
        std::fs::write(&target, b"foreign content").unwrap();
        copy_uninstall(&target, &sidecar).unwrap();
        assert!(target.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"foreign content");

        // Absent: 无事发生, 不报错
        std::fs::remove_file(&target).unwrap();
        copy_uninstall(&target, &sidecar).unwrap();
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn copy_install_creates_the_dir_sets_the_exec_bit_and_overwrites() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("src-bin");
        std::fs::write(&sidecar, b"v2").unwrap();
        let target = local_bin(dir.path()).join("cc-router-tui");

        copy_uninstall(&target, &sidecar).unwrap(); // 没装过: 无事发生

        // 旧副本是我们自己放的 v1 (marker 记录跟它的 (长度, mtime) 匹配 —— StaleOurs): 覆盖后
        // 必须是 0755, marker 必须更新成新副本的数字 (不能留着 v1 的旧值)。
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"v1").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (v1_len, v1_mtime) = file_len_and_mtime(&target).unwrap();
        std::fs::write(target.parent().unwrap().join(COPY_MARKER), marker_content(v1_len, v1_mtime)).unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::StaleOurs);

        copy_install(&sidecar, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"v2");
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
        let marker_path = target.parent().unwrap().join(COPY_MARKER);
        assert!(marker_path.is_file(), "marker 保留, 供下次识别");
        let (v2_len, v2_mtime) = file_len_and_mtime(&target).unwrap();
        assert_eq!(
            parse_marker(&std::fs::read_to_string(&marker_path).unwrap()),
            Some((v2_len, v2_mtime)),
            "marker 必须更新到 v2 的 (长度, mtime), 不能留着 v1 的旧值"
        );

        copy_uninstall(&target, &sidecar).unwrap();
        assert!(!target.exists());
        assert!(!marker_path.exists(), "marker 跟着一起删");
        assert!(target.parent().unwrap().is_dir(), "只删自己的文件, 不删 ~/.local/bin");
    }

    #[test]
    fn shell_quoting_survives_spaces_and_single_quotes() {
        assert_eq!(sh_quote("/Applications/cc-router.app"), "'/Applications/cc-router.app'");
        assert_eq!(sh_quote("/Users/o'brien/My Apps/x"), r"'/Users/o'\''brien/My Apps/x'");
    }

    #[test]
    fn applescript_quoting_escapes_backslash_before_quote() {
        assert_eq!(applescript_quote(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
    }

    #[test]
    fn admin_script_nests_both_quotings() {
        let shell = symlink_shell(Path::new("/Users/o'brien/cc-router.app/Contents/MacOS/cc-router-tui"), Path::new("/usr/local/bin/cc-router-tui"));
        assert_eq!(
            shell,
            r"[ ! -e '/usr/local/bin/cc-router-tui' ] || [ -L '/usr/local/bin/cc-router-tui' ] || exit 1; /bin/mkdir -p '/usr/local/bin' && /bin/ln -sfn '/Users/o'\''brien/cc-router.app/Contents/MacOS/cc-router-tui' '/usr/local/bin/cc-router-tui'"
        );
        let script = admin_script(&shell);
        assert!(script.starts_with("do shell script \"[ ! -e "));
        assert!(script.ends_with("\" with administrator privileges"));
        // shell 里的反斜杠在 AppleScript 字面量里必须成对
        assert!(script.contains(r"o'\\''brien"), "{script}");
        assert_eq!(
            unlink_shell(Path::new("/usr/local/bin/cc-router-tui")),
            "[ -L '/usr/local/bin/cc-router-tui' ] || exit 0; /bin/rm -f '/usr/local/bin/cc-router-tui'"
        );
    }

    #[test]
    fn user_cancel_is_recognised() {
        assert!(is_user_cancel("execution error: User canceled. (-128)"));
        assert!(is_user_cancel("execution error: 用户已取消。 (-128)"));
        assert!(!is_user_cancel("execution error: ln: /usr/local/bin: Read-only file system (1)"));
        // 假阳性防回归: 路径里含 "-128"、错误码是 "-12800" 都不该被当成取消
        assert!(!is_user_cancel("execution error: /Users/build-128/x: Permission denied (1)"));
        assert!(!is_user_cancel("execution error: something failed (-12800)"));
    }

    #[test]
    fn windows_path_membership_ignores_case_trailing_slash_and_quotes() {
        let path = r#"C:\Tools;"C:\Users\Me\AppData\Local\cc-router\";;%USERPROFILE%\bin"#;
        assert!(path_contains(path, r"c:\users\me\appdata\local\CC-ROUTER"));
        assert!(path_contains(path, r"C:\Tools\"));
        assert!(!path_contains(path, r"C:\Users\Me\AppData\Local"));
        assert!(!path_contains("", r"C:\x"));
    }

    #[test]
    fn windows_path_add_is_idempotent_and_keeps_other_entries_verbatim() {
        let path = r"C:\Tools;%USERPROFILE%\bin;";
        let added = path_with(path, r"C:\App");
        assert_eq!(added, r"C:\Tools;%USERPROFILE%\bin;C:\App");
        assert_eq!(path_with(&added, r"c:\app\"), added, "已存在 (大小写 / 尾部斜杠不同) 不再追加");
        assert_eq!(path_with("", r"C:\App"), r"C:\App");
        assert_eq!(path_with(" ; ", r"C:\App"), r"C:\App");
    }

    #[test]
    fn windows_path_remove_only_drops_our_entry() {
        let path = r"C:\Tools;C:\App\;%USERPROFILE%\bin;c:\app";
        assert_eq!(path_without(path, r"C:\App"), r"C:\Tools;%USERPROFILE%\bin");
        assert_eq!(path_without(r"C:\Tools", r"C:\App"), r"C:\Tools");
        assert_eq!(path_without(r"C:\App", r"C:\App"), "");
    }

    #[test]
    fn ps_scripts_fail_closed_and_contain_no_double_quotes() {
        for script in [PS_READ, PS_WRITE] {
            assert!(!script.contains('"'), "脚本不能含双引号 (Windows 参数转义会变复杂): {script}");
            assert!(script.starts_with("$ErrorActionPreference='Stop';"), "必须第一句就 fail-closed: {script}");
            assert!(script.contains("try{"), "必须显式 try, 不能只靠 $ErrorActionPreference 隐式升级异常: {script}");
            assert!(script.contains("}catch{"), "{script}");
            assert!(script.contains("exit 1"), "catch 里必须非 0 退出, 不能让最后一条无关语句的成功掩盖前面的异常: {script}");
        }
        assert!(PS_READ.contains("CCR1:"));
        assert!(PS_READ.contains("ToBase64String"));
        assert!(!PS_READ.contains("OutputEncoding"), "无控制台进程里碰这个属性可能直接抛异常, 不能再用: {PS_READ}");
        assert!(PS_READ.contains("DoNotExpandEnvironmentNames"));
        assert!(PS_WRITE.contains("GetValueKind"));
        assert!(PS_WRITE.contains("ExpandString"));
        assert!(PS_WRITE.contains("CCR_NEW_PATH"));
    }

    // 上面这条测试就是 bite check 的证据: 把 PS_READ/PS_WRITE 的 `catch{...}` 块里的 `exit 1`
    // 删掉, `assert!(script.contains("exit 1"), ...)` 会失败——已手动做过一次并还原, 记录见
    // final-fix-report.md「最后一轮」小节。

    #[test]
    fn decode_ps_read_cases() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let wrap = |s: &str| format!("CCR1:{}", STANDARD.encode(s.as_bytes()));

        assert_eq!(decode_ps_read(wrap(r"C:\Tools;C:\App").as_bytes()).unwrap(), r"C:\Tools;C:\App");
        // BOM: 只剥一个
        let mut with_bom = "\u{FEFF}".as_bytes().to_vec();
        with_bom.extend_from_slice(wrap(r"C:\Tools").as_bytes());
        assert_eq!(decode_ps_read(&with_bom).unwrap(), r"C:\Tools");
        // 空值是合法的空 PATH, 不是错误 (空字符串的 base64 就是空字符串)
        assert_eq!(decode_ps_read(b"CCR1:").unwrap(), "");
        // 结尾的 ; 和空格必须原样保留——base64 是按字节编码的, 不会因为外层 trim() 而丢
        assert_eq!(decode_ps_read(wrap("C:\\Tools; ").as_bytes()).unwrap(), "C:\\Tools; ");
        // CJK 往返: 中文用户名路径, 编码/解码要严格对称
        assert_eq!(decode_ps_read(wrap(r"C:\Users\张三\bin").as_bytes()).unwrap(), r"C:\Users\张三\bin");
        // 标记缺失: 读到的东西不是我们期望的脚本产出, 当错误处理
        assert!(decode_ps_read(b"C:\\Tools").is_err());
        assert!(decode_ps_read(b"").is_err());
        // 标记在, 但后面不是合法 base64
        assert!(decode_ps_read(b"CCR1:not-valid-base64!!!").is_err());
        // 合法 base64, 但解出来的字节不是合法 UTF-8 (GBK 的「你」是 0xC4 0xE3, 不是合法 UTF-8 序列)
        assert!(decode_ps_read(format!("CCR1:{}", STANDARD.encode([0xC4u8, 0xE3])).as_bytes()).is_err());
        // 整段 stdout 本身就不是合法 UTF-8 (还没到 base64 解码那一步)
        assert!(decode_ps_read(&[0xFF, 0xFE, 0x00]).is_err());
    }

    #[test]
    fn powershell_path_resolves_absolutely_with_fallback() {
        assert_eq!(
            powershell_path(Some(std::ffi::OsStr::new(r"D:\Win"))),
            PathBuf::from(r"D:\Win\System32\WindowsPowerShell\v1.0\powershell.exe")
        );
        assert_eq!(
            powershell_path(None),
            PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
        );
    }

    #[test]
    fn linux_helpers() {
        assert!(is_system_bin(Path::new("/usr/bin/cc-router-tui")));
        assert!(!is_system_bin(Path::new("/tmp/.mount_ccrAbC/usr/bin/cc-router-tui")));
        assert_eq!(local_bin(Path::new("/home/me")), PathBuf::from("/home/me/.local/bin"));
        assert!(unix_path_contains("/usr/bin:/home/me/.local/bin/:/bin", Path::new("/home/me/.local/bin")));
        assert!(!unix_path_contains("/usr/bin::/bin", Path::new("/home/me/.local/bin")));
    }

    #[test]
    fn can_install_needs_an_installable_kind_and_no_blocker() {
        let mut s = InstallStatus { kind: InstallKind::Symlink, in_path: false, installed_at: None, blocked: None, local_bin_off_path: false };
        assert!(s.can_install());
        s.blocked = Some(InstallBlocked::Occupied);
        assert!(!s.can_install());
        assert!(!InstallStatus::unavailable().can_install());
        s.blocked = None;
        s.kind = InstallKind::System;
        assert!(!s.can_install());
    }

    #[test]
    fn enums_serialize_as_snake_case() {
        assert_eq!(serde_json::to_value(InstallKind::UserPath).unwrap(), "user_path");
        assert_eq!(serde_json::to_value(InstallBlocked::OnDiskImage).unwrap(), "on_disk_image");
    }
}
