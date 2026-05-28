//! integrations::claude_code 集成测试 — 用 tempfile 注入 HOME, 不污染真实 ~/.claude.

use cc_router_lib::integrations::{
    claude_code::{self, settings_path_in, SyncStatus, BACKUP_SUFFIX},
    sibling_with_suffix,
};
use tempfile::TempDir;

const CC_ROUTER: &str = "http://127.0.0.1:23456";
const OTHER_PROXY: &str = "https://other.example.com";

fn make_home() -> TempDir {
    TempDir::new().expect("tempdir 创建失败")
}

async fn write_existing(home: &std::path::Path, content: &str) {
    let path = settings_path_in(home);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, content).await.unwrap();
}

#[tokio::test]
async fn write_creates_file_when_missing() {
    let home = make_home();
    let path = settings_path_in(home.path());
    assert!(!path.exists(), "前置: settings.json 不应存在");

    let content = serde_json::json!({
        "env": {
            "ANTHROPIC_BASE_URL": CC_ROUTER,
            "ANTHROPIC_AUTH_TOKEN": "tok",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "model-opus",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "model-haiku",
        }
    });
    let raw = serde_json::to_string_pretty(&content).unwrap();

    let outcome = claude_code::write_in(home.path(), &raw, CC_ROUTER)
        .await
        .expect("write 应成功");

    assert!(outcome.backup_path.is_none(), "新建场景不应触发备份");
    assert_eq!(outcome.bytes_written, raw.len());
    assert!(path.exists());
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk, raw, "Rust 端必须 verbatim 写入, 不重格式化");
}

#[tokio::test]
async fn write_preserves_user_unrelated_keys() {
    let home = make_home();
    let original = serde_json::json!({
        "permissions": {"deny": ["read_secret"]},
        "statusLine": {"format": "%h"},
        "language": "zh",
        "env": {"EDITOR": "vim"}
    });
    write_existing(home.path(), &serde_json::to_string_pretty(&original).unwrap()).await;

    // 模拟前端 merge 后的内容: env 内插入 5 核心 + 保留 EDITOR
    let merged = serde_json::json!({
        "permissions": {"deny": ["read_secret"]},
        "statusLine": {"format": "%h"},
        "language": "zh",
        "env": {
            "EDITOR": "vim",
            "ANTHROPIC_BASE_URL": CC_ROUTER,
            "ANTHROPIC_AUTH_TOKEN": "tok",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "model-opus",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "model-haiku",
        }
    });
    let raw = serde_json::to_string_pretty(&merged).unwrap();

    claude_code::write_in(home.path(), &raw, CC_ROUTER)
        .await
        .expect("write");

    let path = settings_path_in(home.path());
    let on_disk: serde_json::Value =
        serde_json::from_str(&tokio::fs::read_to_string(&path).await.unwrap()).unwrap();
    assert_eq!(on_disk["permissions"]["deny"][0], "read_secret");
    assert_eq!(on_disk["statusLine"]["format"], "%h");
    assert_eq!(on_disk["language"], "zh");
    assert_eq!(on_disk["env"]["EDITOR"], "vim");
    assert_eq!(on_disk["env"]["ANTHROPIC_BASE_URL"], CC_ROUTER);
}

#[tokio::test]
async fn write_backs_up_on_first_switch_from_other_proxy() {
    let home = make_home();
    let original = serde_json::json!({
        "env": {"ANTHROPIC_BASE_URL": OTHER_PROXY, "ANTHROPIC_AUTH_TOKEN": "old-tok"}
    });
    write_existing(home.path(), &serde_json::to_string_pretty(&original).unwrap()).await;

    let new_content = serde_json::json!({
        "env": {
            "ANTHROPIC_BASE_URL": CC_ROUTER,
            "ANTHROPIC_AUTH_TOKEN": "new-tok",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "model-opus",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "model-haiku",
        }
    });
    let raw = serde_json::to_string_pretty(&new_content).unwrap();

    let outcome = claude_code::write_in(home.path(), &raw, CC_ROUTER)
        .await
        .expect("write");

    assert!(
        outcome.backup_path.is_some(),
        "切换到 cc-router 前的旧地址非空且非自家, 必须备份"
    );
    let bak = outcome.backup_path.unwrap();
    let bak_content = tokio::fs::read_to_string(&bak).await.unwrap();
    let bak_parsed: serde_json::Value = serde_json::from_str(&bak_content).unwrap();
    assert_eq!(
        bak_parsed["env"]["ANTHROPIC_BASE_URL"], OTHER_PROXY,
        "备份必须保留原始的别家代理 URL"
    );
}

#[tokio::test]
async fn write_does_not_backup_when_already_cc_router() {
    let home = make_home();
    let original = serde_json::json!({
        "env": {"ANTHROPIC_BASE_URL": CC_ROUTER, "ANTHROPIC_AUTH_TOKEN": "tok"}
    });
    write_existing(home.path(), &serde_json::to_string_pretty(&original).unwrap()).await;

    let new_content = serde_json::json!({"env": {"ANTHROPIC_BASE_URL": CC_ROUTER}});
    let raw = serde_json::to_string_pretty(&new_content).unwrap();

    let outcome = claude_code::write_in(home.path(), &raw, CC_ROUTER)
        .await
        .expect("write");

    assert!(
        outcome.backup_path.is_none(),
        "旧 BASE_URL 已是 cc-router, 不应备份"
    );
}

#[tokio::test]
async fn write_does_not_overwrite_existing_backup() {
    let home = make_home();
    // 已有 .bak 模拟"用户之前切换过一次"的状态
    let path = settings_path_in(home.path());
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    // 用 production const + helper 构造 .bak 路径, 重命名 suffix 时测试不静默失效.
    let bak_path = sibling_with_suffix(&path, BACKUP_SUFFIX);
    tokio::fs::write(&bak_path, r#"{"origin":"first-backup"}"#)
        .await
        .unwrap();

    let original = serde_json::json!({
        "env": {"ANTHROPIC_BASE_URL": OTHER_PROXY}
    });
    tokio::fs::write(&path, serde_json::to_string_pretty(&original).unwrap())
        .await
        .unwrap();

    let new_content = serde_json::json!({"env": {"ANTHROPIC_BASE_URL": CC_ROUTER}});
    let raw = serde_json::to_string_pretty(&new_content).unwrap();

    let outcome = claude_code::write_in(home.path(), &raw, CC_ROUTER)
        .await
        .expect("write");

    assert!(
        outcome.backup_path.is_none(),
        ".bak 已存在时不应再次备份 (保护最早的原始状态)"
    );
    let bak_after = tokio::fs::read_to_string(&bak_path).await.unwrap();
    assert!(
        bak_after.contains("first-backup"),
        "原 .bak 内容不应被覆盖"
    );
}

#[tokio::test]
async fn write_rejects_invalid_json() {
    let home = make_home();
    let err = claude_code::write_in(home.path(), "{not json", CC_ROUTER)
        .await
        .expect_err("非法 JSON 应被拒收");
    assert!(
        format!("{err}").contains("settings.json"),
        "错误信息应说明是 settings.json 问题, got: {err}"
    );
}

#[tokio::test]
async fn inspect_covers_all_status_transitions() {
    let home = make_home();

    // 1) 文件不存在 → FileMissing
    let r = claude_code::inspect_in(home.path(), CC_ROUTER, "tok", true)
        .await
        .unwrap();
    assert_eq!(r.status, SyncStatus::FileMissing);

    // 2) 文件存在但无 ANTHROPIC_* → NeverApplied
    write_existing(home.path(), r#"{"language":"zh"}"#).await;
    let r = claude_code::inspect_in(home.path(), CC_ROUTER, "tok", true)
        .await
        .unwrap();
    assert_eq!(r.status, SyncStatus::NeverApplied);

    // 3) BASE_URL 是 cc-router, 5 字段都在, token 一致 → InSync
    let full = serde_json::json!({
        "env": {
            "ANTHROPIC_BASE_URL": CC_ROUTER,
            "ANTHROPIC_AUTH_TOKEN": "tok",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "model-opus",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "model-haiku",
        }
    });
    write_existing(home.path(), &full.to_string()).await;
    let r = claude_code::inspect_in(home.path(), CC_ROUTER, "tok", true)
        .await
        .unwrap();
    assert_eq!(r.status, SyncStatus::InSync);

    // 4) token 改了 → NeedsApply
    let r = claude_code::inspect_in(home.path(), CC_ROUTER, "new-tok", true)
        .await
        .unwrap();
    assert_eq!(r.status, SyncStatus::NeedsApply);
    assert!(!r.current_token_matches);

    // 5) JSON 损坏 → ParseError
    write_existing(home.path(), "{ broken").await;
    let r = claude_code::inspect_in(home.path(), CC_ROUTER, "tok", true)
        .await
        .unwrap();
    assert_eq!(r.status, SyncStatus::ParseError);
}

#[tokio::test]
async fn inspect_ignores_token_when_auth_disabled() {
    let home = make_home();
    // 5 字段都在, BASE_URL 对, 但 token 不一样 (用户没改 settings.json 的 token, cc-router 关了鉴权).
    let full = serde_json::json!({
        "env": {
            "ANTHROPIC_BASE_URL": CC_ROUTER,
            "ANTHROPIC_AUTH_TOKEN": "stale-tok",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "model-opus",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "model-haiku",
        }
    });
    write_existing(home.path(), &full.to_string()).await;

    // auth_required=true 时 token 不一致 → NeedsApply
    let r = claude_code::inspect_in(home.path(), CC_ROUTER, "fresh-tok", true)
        .await
        .unwrap();
    assert_eq!(r.status, SyncStatus::NeedsApply);

    // auth_required=false 时 token 字段被忽略 → InSync
    let r = claude_code::inspect_in(home.path(), CC_ROUTER, "fresh-tok", false)
        .await
        .unwrap();
    assert_eq!(r.status, SyncStatus::InSync);
    assert!(r.current_token_matches);
}
