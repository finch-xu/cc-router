//! 请求日志查询 command。简单 offset/limit 分页，按 timestamp 倒序。
//! 支持按 virtual_model_name / provider_id / status 筛选。
//! 另提供 CSV 导出 (同筛选条件, 流式写文件)。

use std::io::Write;

use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct RequestLogDto {
    pub id: String,
    pub timestamp: i64,
    pub virtual_model_name: String,
    pub subscription_id: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub real_model_name: String,
    pub response_model_name: Option<String>,
    pub is_streaming: bool,
    pub status: String,
    pub http_status: Option<i64>,
    pub total_latency_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub error_message: Option<String>,
    /// 上游错误响应 body 截断, 仅在错误路径有值, 用于前端排障详情抽屉
    pub upstream_response_body: Option<String>,
    /// 客户端识别结果 (Claude Code / Zed / Codex CLI / ...). NULL → 未识别 (前端展示 "unk")
    pub client_tool: Option<String>,
    /// 客户端原始 User-Agent. 详情抽屉展示
    pub client_user_agent: Option<String>,
    /// 从 UA 或 stainless headers 提取的客户端版本号
    pub client_version: Option<String>,
    /// TCP 对端 IP (来自 axum ConnectInfo). listen_all=true 场景下区分本机/局域网设备的关键
    pub client_ip: Option<String>,
    /// 请求入口: "messages" (POST /v1/messages) / "responses" (POST /v1/responses).
    /// 老日志为 NULL, 前端展示 "—".
    pub entry_kind: Option<String>,
    /// 下游 (CC ↔ cc-router) 协商的 HTTP 协议, 形如 "HTTP/1.1" / "HTTP/2.0".
    pub downstream_http_version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListRequestsResult {
    pub items: Vec<RequestLogDto>,
    pub total: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RequestLogFilters {
    pub virtual_model_name: Option<String>,
    pub provider_id: Option<String>,
    pub status: Option<String>,
    pub subscription_id: Option<String>,
    /// 按客户端工具筛选。特殊值 `__unknown__` 映射到 `client_tool IS NULL`
    /// (前端筛选器的「未识别」选项), 其余值走 `client_tool = ?` 精确匹配。
    pub client_tool: Option<String>,
}

// client_tool 特殊值 → 拼 `IS NULL` 而非 `= ?`. 与前端
// `src/types.ts::CLIENT_TOOL_UNKNOWN_SENTINEL` 必须保持同值, 否则筛选「未识别」会静默失效.
const UNKNOWN_SENTINEL: &str = "__unknown__";

/// 动态构建 WHERE 子句。列名是白名单字面量, 值走 bind, 无注入风险。
/// 返回 (where 子句, bind 值列表); `IS NULL` 分支不产生 bind 值。
/// list_requests 与 export_requests_csv 共用, 保证「导出所见即所得」。
fn build_filter_clause(filters: &RequestLogFilters) -> (String, Vec<String>) {
    let active: Vec<(&'static str, &str)> = [
        ("virtual_model_name", filters.virtual_model_name.as_deref()),
        ("provider_id", filters.provider_id.as_deref()),
        ("status", filters.status.as_deref()),
        ("subscription_id", filters.subscription_id.as_deref()),
        ("client_tool", filters.client_tool.as_deref()),
    ]
    .into_iter()
    .filter_map(|(col, val)| val.map(|v| (col, v)))
    .collect();

    if active.is_empty() {
        return (String::new(), Vec::new());
    }
    let conds: Vec<String> = active
        .iter()
        .map(|(c, v)| {
            if *c == "client_tool" && *v == UNKNOWN_SENTINEL {
                format!("{} IS NULL", c)
            } else {
                format!("{} = ?", c)
            }
        })
        .collect();
    let binds = active
        .iter()
        .filter(|(c, v)| !(*c == "client_tool" && *v == UNKNOWN_SENTINEL))
        .map(|(_, v)| v.to_string())
        .collect();
    (format!(" WHERE {}", conds.join(" AND ")), binds)
}

#[tauri::command]
pub async fn list_requests(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
    filters: Option<RequestLogFilters>,
) -> AppResult<ListRequestsResult> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let offset = (page - 1) as i64 * page_size as i64;
    let filters = filters.unwrap_or_default();

    let (where_clause, binds) = build_filter_clause(&filters);

    let count_sql = format!("SELECT COUNT(*) AS c FROM requests{}", where_clause);
    let mut count_q = sqlx::query(&count_sql);
    for v in &binds {
        count_q = count_q.bind(v);
    }
    let total: i64 = count_q.fetch_one(&state.db).await?.try_get("c")?;

    let select_sql = format!(
        "SELECT id, timestamp, virtual_model_name, subscription_id, provider_id, endpoint_id,
                real_model_name, response_model_name, is_streaming, status,
                http_status, total_latency_ms,
                upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read, error_message,
                upstream_response_body,
                client_tool, client_user_agent, client_version, client_ip,
                entry_kind, downstream_http_version
         FROM requests{}
         ORDER BY timestamp DESC
         LIMIT ? OFFSET ?",
        where_clause
    );
    let mut select_q = sqlx::query(&select_sql);
    for v in &binds {
        select_q = select_q.bind(v);
    }
    let rows = select_q
        .bind(page_size as i64)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    let items = rows
        .into_iter()
        .map(|r| RequestLogDto {
            id: r.try_get("id").unwrap_or_default(),
            timestamp: r.try_get("timestamp").unwrap_or(0),
            virtual_model_name: r.try_get("virtual_model_name").unwrap_or_default(),
            subscription_id: r.try_get("subscription_id").unwrap_or_default(),
            provider_id: r.try_get("provider_id").unwrap_or_default(),
            endpoint_id: r.try_get("endpoint_id").unwrap_or_default(),
            real_model_name: r.try_get("real_model_name").unwrap_or_default(),
            response_model_name: r.try_get("response_model_name").ok(),
            is_streaming: r.try_get::<i64, _>("is_streaming").unwrap_or(0) != 0,
            status: r.try_get("status").unwrap_or_default(),
            http_status: r.try_get("http_status").ok(),
            total_latency_ms: r.try_get("total_latency_ms").ok(),
            input_tokens: r.try_get("upstream_input_tokens").ok(),
            output_tokens: r.try_get("upstream_output_tokens").ok(),
            cache_creation_tokens: r.try_get("upstream_cache_creation").ok(),
            cache_read_tokens: r.try_get("upstream_cache_read").ok(),
            error_message: r.try_get("error_message").ok(),
            upstream_response_body: r.try_get("upstream_response_body").ok(),
            client_tool: r.try_get("client_tool").ok(),
            client_user_agent: r.try_get("client_user_agent").ok(),
            client_version: r.try_get("client_version").ok(),
            client_ip: r.try_get("client_ip").ok(),
            entry_kind: r.try_get("entry_kind").ok(),
            downstream_http_version: r.try_get("downstream_http_version").ok(),
        })
        .collect();

    Ok(ListRequestsResult { items, total })
}

/// CSV 字段转义: 含逗号/双引号/换行时整体加双引号并把 `"` 转义为 `""` (RFC 4180)。
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// CSV 列 (与 RequestLogDto 对齐, 刻意不含 upstream_response_body —— 大且多行,
/// 排障专用不适合表格; 另在 timestamp 旁加人类可读的 timestamp_iso)。
const CSV_HEADER: &str = "id,timestamp,timestamp_iso,virtual_model_name,subscription_id,\
provider_id,endpoint_id,real_model_name,response_model_name,is_streaming,status,http_status,\
total_latency_ms,input_tokens,output_tokens,cache_creation_tokens,cache_read_tokens,\
error_message,client_tool,client_user_agent,client_version,client_ip,entry_kind,\
downstream_http_version";

/// 按当前筛选条件把请求日志导出为 CSV 文件 (路径由前端 save dialog 提供)。
/// 流式逐行写 BufWriter, 不整表载入内存; 返回导出的行数。
/// 文件头带 UTF-8 BOM, Excel 直接打开中文不乱码。
#[tauri::command]
pub async fn export_requests_csv(
    state: State<'_, AppState>,
    path: String,
    filters: Option<RequestLogFilters>,
) -> AppResult<u64> {
    let filters = filters.unwrap_or_default();
    let (where_clause, binds) = build_filter_clause(&filters);

    // 导出按时间正序, 符合表格阅读习惯 (页面展示是倒序)
    let sql = format!(
        "SELECT id, timestamp, virtual_model_name, subscription_id, provider_id, endpoint_id,
                real_model_name, response_model_name, is_streaming, status,
                http_status, total_latency_ms,
                upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read, error_message,
                client_tool, client_user_agent, client_version, client_ip,
                entry_kind, downstream_http_version
         FROM requests{}
         ORDER BY timestamp ASC",
        where_clause
    );
    let mut q = sqlx::query(&sql);
    for v in &binds {
        q = q.bind(v);
    }

    let file = std::fs::File::create(&path)
        .map_err(|e| crate::error::AppError::Internal(format!("创建导出文件失败: {e}")))?;
    let mut w = std::io::BufWriter::new(file);
    let io_err = |e: std::io::Error| crate::error::AppError::Internal(format!("写入 CSV 失败: {e}"));

    w.write_all("\u{FEFF}".as_bytes()).map_err(io_err)?;
    writeln!(w, "{}", CSV_HEADER).map_err(io_err)?;

    let opt_str = |v: Option<String>| v.map(|s| csv_field(&s)).unwrap_or_default();
    let opt_num = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();

    let mut count: u64 = 0;
    let mut stream = q.fetch(&state.db);
    while let Some(r) = stream.try_next().await? {
        let ts: i64 = r.try_get("timestamp").unwrap_or(0);
        let ts_iso = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ts)
            .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .unwrap_or_default();
        let line = [
            csv_field(&r.try_get::<String, _>("id").unwrap_or_default()),
            ts.to_string(),
            ts_iso,
            csv_field(&r.try_get::<String, _>("virtual_model_name").unwrap_or_default()),
            csv_field(&r.try_get::<String, _>("subscription_id").unwrap_or_default()),
            csv_field(&r.try_get::<String, _>("provider_id").unwrap_or_default()),
            csv_field(&r.try_get::<String, _>("endpoint_id").unwrap_or_default()),
            csv_field(&r.try_get::<String, _>("real_model_name").unwrap_or_default()),
            opt_str(r.try_get("response_model_name").ok()),
            (r.try_get::<i64, _>("is_streaming").unwrap_or(0) != 0).to_string(),
            csv_field(&r.try_get::<String, _>("status").unwrap_or_default()),
            opt_num(r.try_get("http_status").ok()),
            opt_num(r.try_get("total_latency_ms").ok()),
            opt_num(r.try_get("upstream_input_tokens").ok()),
            opt_num(r.try_get("upstream_output_tokens").ok()),
            opt_num(r.try_get("upstream_cache_creation").ok()),
            opt_num(r.try_get("upstream_cache_read").ok()),
            opt_str(r.try_get("error_message").ok()),
            opt_str(r.try_get("client_tool").ok()),
            opt_str(r.try_get("client_user_agent").ok()),
            opt_str(r.try_get("client_version").ok()),
            opt_str(r.try_get("client_ip").ok()),
            opt_str(r.try_get("entry_kind").ok()),
            opt_str(r.try_get("downstream_http_version").ok()),
        ]
        .join(",");
        writeln!(w, "{}", line).map_err(io_err)?;
        count += 1;
    }
    w.flush().map_err(io_err)?;
    Ok(count)
}

/// 返回前端可在筛选器里展示的「已支持识别的 client tool」白名单。
/// 数据源是 [`crate::proxy::client_fingerprint::SUPPORTED_TOOLS`], 后端单一信息源,
/// 前端硬编码的 i18n 文案需手工同步 (类比 `ProviderLogo BRAND_MAP`).
#[tauri::command]
pub async fn list_supported_client_tools() -> AppResult<Vec<&'static str>> {
    Ok(crate::proxy::client_fingerprint::SUPPORTED_TOOLS.to_vec())
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn csv_field_escapes_specials() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field(""), "");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("he said \"hi\""), "\"he said \"\"hi\"\"\"");
        assert_eq!(csv_field("line1\nline2"), "\"line1\nline2\"");
        assert_eq!(csv_field("中文, 带逗号"), "\"中文, 带逗号\"");
    }
}
