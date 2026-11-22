//! 从 GitHub 取插件: ref → 提交号 (api.github.com), 文件从 raw.githubusercontent.com 按提交号下载。
//! 只有「解析分支 / tag」会用到匿名 API (60 次 / 小时 / IP); 填完整提交号或从社区安装都不消耗。

use std::time::Duration;

use reqwest::StatusCode;

use super::link::{is_full_sha, RepoRef};
use crate::error::{AppError, AppResult};

const TIMEOUT: Duration = Duration::from_secs(30);

fn net_err(e: reqwest::Error) -> AppError {
    AppError::BadGateway(format!("连接 GitHub 失败: {e}"))
}

/// 把 ref (缺省 = 默认分支) 解析成 40 位提交号。
pub async fn resolve_commit(client: &reqwest::Client, r: &RepoRef) -> AppResult<String> {
    if let Some(sha) = r.git_ref.as_deref().filter(|s| is_full_sha(s)) {
        return Ok(sha.to_ascii_lowercase());
    }
    let git_ref = r.git_ref.as_deref().unwrap_or("HEAD");
    let url = format!("https://api.github.com/repos/{}/{}/commits/{}", r.owner, r.repo, git_ref);
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github.sha")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(net_err)?;
    match resp.status() {
        StatusCode::OK => {}
        StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
            return Err(AppError::BadRequest(format!("找不到仓库 {}/{} 或其中的「{git_ref}」(只支持公开仓库)", r.owner, r.repo)));
        }
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
            return Err(AppError::BadGateway("GitHub 接口调用次数已用完, 请稍后再试, 或直接填写完整的 40 位提交号".into()));
        }
        s => return Err(AppError::BadGateway(format!("GitHub 返回 {s}"))),
    }
    let sha = resp.text().await.map_err(net_err)?.trim().to_ascii_lowercase();
    if !is_full_sha(&sha) {
        return Err(AppError::BadGateway("GitHub 返回的提交号格式不对".into()));
    }
    Ok(sha)
}

/// 下载提交里的一个文件, 超过 `max` 字节立刻中断。404 返回 `Ok(None)`。
pub async fn fetch_file(client: &reqwest::Client, r: &RepoRef, sha: &str, path: &str, max: u64) -> AppResult<Option<Vec<u8>>> {
    let full = match &r.path {
        Some(sub) => format!("{sub}/{path}"),
        None => path.to_string(),
    };
    let url = format!("https://raw.githubusercontent.com/{}/{}/{sha}/{full}", r.owner, r.repo);
    fetch_limited(client, &url, max).await
}

/// GET 一个 URL, 流式读取并限制大小。404 → `Ok(None)`。
pub async fn fetch_limited(client: &reqwest::Client, url: &str, max: u64) -> AppResult<Option<Vec<u8>>> {
    let mut resp = client.get(url).timeout(TIMEOUT).send().await.map_err(net_err)?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(AppError::BadGateway(format!("下载失败 ({}): {url}", resp.status())));
    }
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(AppError::BadRequest(format!("文件超过大小上限 ({} MB): {url}", max / 1024 / 1024)));
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(net_err)? {
        out.extend_from_slice(&chunk);
        if out.len() as u64 > max {
            return Err(AppError::BadRequest(format!("文件超过大小上限 ({} MB): {url}", max / 1024 / 1024)));
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 打真实 GitHub: `cargo test --lib plugins::github -- --ignored`
    #[tokio::test]
    #[ignore = "needs network"]
    async fn live_resolve_and_fetch() {
        let client = reqwest::Client::builder().user_agent("cc-router-test").build().unwrap();
        let r = super::super::link::parse("https://github.com/finch-xu/cc-router").unwrap();
        let sha = resolve_commit(&client, &r).await.unwrap();
        assert_eq!(sha.len(), 40);
        let pkg = fetch_file(&client, &r, &sha, "package.json", 1024 * 1024).await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&pkg).contains("\"name\": \"cc-router\""));
        assert!(fetch_file(&client, &r, &sha, "no-such-file.json", 1024).await.unwrap().is_none());
        // 超过上限立即中断
        assert!(fetch_file(&client, &r, &sha, "package.json", 16).await.is_err());
        let missing = super::super::link::parse("finch-xu/definitely-not-a-repo-xyz").unwrap();
        assert!(resolve_commit(&client, &missing).await.is_err());
    }
}
