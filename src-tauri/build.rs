use std::fmt::Write as _;
use std::path::Path;

fn main() {
    embed_providers();
    ensure_sidecar_placeholder();
    tauri_build::build()
}

/// tauri-build 在**每次** cargo 构建 (含 check / test) 时都会复制 `bundle.externalBin` 指向的文件,
/// 不存在就直接失败。真正的 sidecar 由 `scripts/build-tui-sidecar.mjs` 在 tauri dev / build 的钩子里生成;
/// 这里只保证「没跑过钩子」的场景 (新克隆后直接 cargo test) 也能编过: 缺文件时放一个 0 字节占位。
/// 占位永远不会覆盖真文件, `commands::tui` 把 0 字节文件视为「未包含 TUI」。
fn ensure_sidecar_placeholder() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let target = std::env::var("TARGET").expect("TARGET");
    let ext = if target.contains("windows") { ".exe" } else { "" };
    let dir = Path::new(&manifest_dir).join("binaries");
    let file = dir.join(format!("cc-router-tui-{target}{ext}"));
    if !file.exists() {
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("创建 {} 失败: {e}", dir.display()));
        std::fs::write(&file, b"").unwrap_or_else(|e| panic!("写入 {} 失败: {e}", file.display()));
    }
}

/// 把 `providers/*.yaml` 编进二进制: 生成 `$OUT_DIR/embedded_providers.rs`, 内容是一张
/// `(文件名, include_str!(绝对路径))` 表, 由 `provider::loader` 用 `include!` 引入。
///
/// 目录扫描放在 build.rs 而不是手写 `include_str!` 列表, 是为了「加 provider 只需要丢一个 yaml」——
/// 以前漏登记 `tauri.conf.json::bundle.resources` 会导致 release 包启动即 fatal。
fn embed_providers() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dir = Path::new(&manifest_dir).join("providers");
    // 对目录声明: cargo 递归比较 mtime, 新增 / 删除 / 修改 yaml 都会重跑本脚本。
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("读取 {} 失败: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|x| x.to_str()), Some("yaml" | "yml")))
        .collect();
    // read_dir 顺序依赖文件系统; 排序保证生成结果 (进而二进制) 可复现。
    files.sort();
    assert!(!files.is_empty(), "{} 下没有任何 provider yaml", dir.display());

    let mut out = String::from("pub static EMBEDDED_PROVIDERS: &[(&str, &str)] = &[\n");
    for path in &files {
        let name = path.file_name().and_then(|n| n.to_str()).expect("yaml 文件名非 UTF-8");
        // {:?} 负责转义: Windows 路径里的反斜杠必须变成合法的 Rust 字符串字面量。
        writeln!(out, "    ({name:?}, include_str!({:?})),", path.display().to_string()).unwrap();
    }
    out.push_str("];\n");

    let dest = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("embedded_providers.rs");
    std::fs::write(&dest, out).unwrap_or_else(|e| panic!("写入 {} 失败: {e}", dest.display()));
}
