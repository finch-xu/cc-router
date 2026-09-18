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
fn is_on_disk_image_at_volume_root(sidecar: &Path) -> bool {
    let comps: Vec<&str> = sidecar.components().filter_map(|c| c.as_os_str().to_str()).collect();
    // comps[0] 是根 "/"; [1] 应为 "Volumes"; [3] (卷名之后紧跟的一段) 若以 ".app" 结尾就是卷根。
    comps.get(1).is_some_and(|c| *c == "Volumes") && comps.get(3).is_some_and(|c| c.ends_with(".app"))
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

pub fn symlink_shell(sidecar: &Path, link: &Path) -> String {
    let dir = link.parent().unwrap_or(Path::new("/"));
    format!(
        "mkdir -p {} && ln -sfn {} {}",
        sh_quote(&dir.to_string_lossy()),
        sh_quote(&sidecar.to_string_lossy()),
        sh_quote(&link.to_string_lossy())
    )
}

pub fn unlink_shell(link: &Path) -> String {
    format!("rm -f {}", sh_quote(&link.to_string_lossy()))
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
    /// 内容不同, 也没有我们放的 marker: 不是我们的东西, 不碰
    Foreign,
    /// 内容跟当前 sidecar 不一致, 但旁边有 marker 文件——app 更新后 sidecar 变了, 这其实是自家旧副本。
    /// 覆盖 / 删除都安全。
    StaleOurs,
}

/// 与 `target` 同目录的标记文件: 证明 `target` 是 `copy_install` 放的, 不是用户自己的同名文件。
/// 没有这个标记, app 更新后旧副本内容对不上新 sidecar, 会被误判成 `Foreign`——那对我们自己的
/// 旧副本是错的 (见 fix-1 R1)。`copy_install` 写它, `copy_uninstall` 删它。
#[cfg(unix)]
pub const COPY_MARKER: &str = ".cc-router-tui.installed-by-cc-router";

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
    let has_marker = target.parent().is_some_and(|d| d.join(COPY_MARKER).is_file());
    if has_marker {
        CopyState::StaleOurs
    } else {
        CopyState::Foreign
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
    // 写 marker: 之后 app 更新、sidecar 字节变了, 仍能认出这是自家旧副本 (CopyState::StaleOurs)。
    std::fs::write(dir.join(COPY_MARKER), b"").map_err(|e| format!("写 marker: {e}"))?;
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

    // 直接读写注册表值而不是 [Environment]::Get/SetEnvironmentVariable: 后者读出来的是**展开后**的值,
    // 写回去又一律存成 REG_SZ —— 用户 PATH 里的 %USERPROFILE%\… 会被永久展开, 类型也被改掉。
    // 更不能用 setx (截断到 1024 字符)。写完后借一次对不存在变量的 SetEnvironmentVariable 广播 WM_SETTINGCHANGE,
    // 新开的终端立即看到新 PATH。
    const READ: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
        $k=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment'); \
        if($k){[Console]::Out.Write($k.GetValue('Path','',[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames))}";
    const WRITE: &str = "$k=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); \
        $kind=if($k.GetValueNames() -contains 'Path'){$k.GetValueKind('Path')}else{[Microsoft.Win32.RegistryValueKind]::ExpandString}; \
        $k.SetValue('Path',$env:CCR_NEW_PATH,$kind); \
        [Environment]::SetEnvironmentVariable('CCR_TUI_PATH_REFRESH',$null,'User')";

    fn powershell(script: &str, new_path: Option<&str>) -> Result<String, String> {
        let exe = powershell_path(std::env::var_os("SystemRoot").as_deref());
        let mut cmd = Command::new(exe);
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]).creation_flags(CREATE_NO_WINDOW);
        if let Some(p) = new_path {
            // 新值走环境变量传进去, 不拼进脚本文本: 路径里的引号 / 分号 / $ 都不需要转义。
            cmd.env("CCR_NEW_PATH", p);
        }
        let out = cmd.output().map_err(|e| format!("无法调用 PowerShell: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    fn install_dir(sidecar: &Path) -> Option<String> {
        sidecar.parent().map(|d| d.to_string_lossy().into_owned())
    }

    pub fn status(sidecar: &Path) -> InstallStatus {
        let dir = install_dir(sidecar);
        let user_path = powershell(READ, None).unwrap_or_default();
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
        let current = powershell(READ, None)?;
        let next = path_with(&current, &dir);
        if next == current {
            return Ok(Outcome::Done);
        }
        powershell(WRITE, Some(&next)).map(|_| Outcome::Done)
    }

    pub fn uninstall(sidecar: &Path) -> InstallResult {
        let dir = install_dir(sidecar).ok_or("无法确定安装目录")?;
        let current = powershell(READ, None)?;
        let next = path_without(&current, &dir);
        if next == current {
            return Ok(Outcome::Done);
        }
        powershell(WRITE, Some(&next)).map(|_| Outcome::Done)
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
        let in_path = state == CopyState::Ours;
        let blocked = (state == CopyState::Foreign).then_some(InstallBlocked::Occupied);
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

        // 加上 marker: app 更新后 sidecar 换了字节, 但这其实是自家旧副本
        std::fs::write(dir.path().join(COPY_MARKER), b"").unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::StaleOurs);

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

        // 旧副本是我们自己放的 (内容对不上新 sidecar, 但旁边有 marker —— StaleOurs): 覆盖后必须是 0755
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"v1").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(target.parent().unwrap().join(COPY_MARKER), b"").unwrap();
        assert_eq!(copy_state(&target, &sidecar), CopyState::StaleOurs);

        copy_install(&sidecar, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"v2");
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(target.parent().unwrap().join(COPY_MARKER).is_file(), "marker 保留, 供下次识别");

        copy_uninstall(&target, &sidecar).unwrap();
        assert!(!target.exists());
        assert!(!target.parent().unwrap().join(COPY_MARKER).exists(), "marker 跟着一起删");
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
            r"mkdir -p '/usr/local/bin' && ln -sfn '/Users/o'\''brien/cc-router.app/Contents/MacOS/cc-router-tui' '/usr/local/bin/cc-router-tui'"
        );
        let script = admin_script(&shell);
        assert!(script.starts_with("do shell script \"mkdir -p "));
        assert!(script.ends_with("\" with administrator privileges"));
        // shell 里的反斜杠在 AppleScript 字面量里必须成对
        assert!(script.contains(r"o'\\''brien"), "{script}");
        assert_eq!(unlink_shell(Path::new("/usr/local/bin/cc-router-tui")), "rm -f '/usr/local/bin/cc-router-tui'");
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
