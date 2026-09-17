use std::fmt::Write as _;
use std::path::Path;

fn main() {
    embed_providers();
    tauri_build::build()
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
