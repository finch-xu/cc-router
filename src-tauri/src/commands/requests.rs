//! 请求日志查询 command。简单 offset/limit 分页，按 timestamp 倒序。
//! 支持按 virtual_model_name / provider_id / status 筛选。
//! 另提供 CSV 导出 (同筛选条件, 流式写文件)。

use std::collections::HashMap;
use std::io::Write;

use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::State;

use crate::error::AppResult;
use crate::observability::live_feed::{self, RequestSummary, SUMMARY_COLUMNS};
use crate::provider::model::AuthType;
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
    pub first_content_ms: Option<i64>,
    pub output_tokens_per_second: Option<f64>,
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
    /// 思考强度 — 客户端本次请求携带的档位, 老日志/拿不到为 NULL
    pub client_effort: Option<String>,
    /// 思考强度 — cc-router 实际发往上游的档位
    pub effective_effort: Option<String>,
    /// `effective_effort` 的来源: "slot" / "client" / "yaml"
    pub effort_source: Option<String>,
    /// 上游响应回显的档位, 仅 OpenAI Responses 系有值
    pub upstream_effort: Option<String>,
    /// Anthropic stop_reason (或跨协议映射后的等价值), 老日志为 NULL
    pub stop_reason: Option<String>,
    /// 请求体里客户端声明的可用工具数 (tools 数组长度)
    pub tools_offered_count: Option<i64>,
    /// 请求体历史消息里 tool_result 块数
    pub tool_result_count: Option<i64>,
    /// 响应里 tool_use / server_tool_use 块数
    pub tool_use_count: Option<i64>,
    /// JSON 数组字符串, 前端 JSON.parse
    pub tool_use_names: Option<String>,
    /// 与实时路由同形的摘要 (用途 / 会话 / 归一后的 token / 尝试归组), 列表行与实时事件行共用。
    /// 由 [`live_feed::from_row`] 生成, 口径与 `live_request` 事件完全一致。
    pub summary: RequestSummary,
    /// 网络耗时分段 (proxy/net_timing.rs), 详情页耗时条用; 老日志各字段为 null。
    pub net: crate::proxy::net_timing::NetTimingFields,
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
    /// 只看这个时间点 (ms) 之后的请求
    pub since_ms: Option<i64>,
    /// 用途分组: main / agent / bg (所有后台类) / fail (非成功), 与实时路由的筛选 chips 同义
    pub purpose: Option<String>,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    /// 模糊搜索: 模型名 (请求 / 回显) / 工具名 / 请求 id 前缀
    pub q: Option<String>,
}

/// 实时路由里算作「后台任务」的用途, 与前端 `liveStats.ts::isBackground` 同表
const BACKGROUND_KINDS: &[&str] = &["compact", "guard", "title", "fetch", "probe", "background"];

// client_tool 特殊值 → 拼 `IS NULL` 而非 `= ?`. 与前端
// `src/types.ts::CLIENT_TOOL_UNKNOWN_SENTINEL` 必须保持同值, 否则筛选「未识别」会静默失效.
pub(crate) const UNKNOWN_SENTINEL: &str = "__unknown__";

#[derive(Debug, Clone)]
pub(crate) enum Bind {
    S(String),
    I(i64),
}

type Query<'q> = sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>;

pub(crate) fn bind_all<'q>(mut q: Query<'q>, binds: &[Bind]) -> Query<'q> {
    for b in binds {
        q = match b {
            Bind::S(v) => q.bind(v.clone()),
            Bind::I(v) => q.bind(*v),
        };
    }
    q
}

/// 动态构建 WHERE 子句。列名是白名单字面量, 值走 bind, 无注入风险。
/// list_requests / 导出 / 计数 / 上一条下一条共用, 保证「导出所见即所得」。
/// `skip` 里的维度不参与 (筛选 chips 的计数要排除自身那一维)。
fn build_filter_clause_except(filters: &RequestLogFilters, skip: &[&str]) -> (String, Vec<Bind>) {
    let mut conds: Vec<String> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();
    let on = |dim: &str| !skip.contains(&dim);
    let eq = [
        ("virtual_model_name", filters.virtual_model_name.as_deref()),
        ("provider_id", filters.provider_id.as_deref()),
        ("status", filters.status.as_deref()),
        ("subscription_id", filters.subscription_id.as_deref()),
        ("session_id", filters.session_id.as_deref()),
        ("trace_id", filters.trace_id.as_deref()),
    ];
    for (col, val) in eq {
        if let Some(v) = val.filter(|_| on(col)) {
            conds.push(format!("{col} = ?"));
            binds.push(Bind::S(v.to_owned()));
        }
    }
    if let Some(v) = filters.client_tool.as_deref() {
        if v == UNKNOWN_SENTINEL {
            conds.push("client_tool IS NULL".into());
        } else {
            conds.push("client_tool = ?".into());
            binds.push(Bind::S(v.to_owned()));
        }
    }
    if let Some(ms) = filters.since_ms {
        conds.push("timestamp >= ?".into());
        binds.push(Bind::I(ms));
    }
    if let Some(p) = filters.purpose.as_deref().filter(|_| on("purpose")) {
        match p {
            "fail" => conds.push("status != 'success'".into()),
            "bg" => conds.push(format!(
                "purpose_kind IN ({})",
                BACKGROUND_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",")
            )),
            other => {
                conds.push("purpose_kind = ?".into());
                binds.push(Bind::S(other.to_owned()));
            }
        }
    }
    if let Some(q) = filters.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let like = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        conds.push(
            "(real_model_name LIKE ? ESCAPE '\\' OR response_model_name LIKE ? ESCAPE '\\'
              OR tool_use_names LIKE ? ESCAPE '\\' OR id LIKE ? ESCAPE '\\')"
                .into(),
        );
        for _ in 0..3 {
            binds.push(Bind::S(like.clone()));
        }
        binds.push(Bind::S(format!("{}%", q.replace('%', "").replace('_', ""))));
    }
    if conds.is_empty() {
        return (String::new(), binds);
    }
    (format!(" WHERE {}", conds.join(" AND ")), binds)
}

fn build_filter_clause(filters: &RequestLogFilters) -> (String, Vec<Bind>) {
    build_filter_clause_except(filters, &[])
}

/// 列表 / 详情共用的列: 详情字段 + 摘要字段 (重名列 SQLite 允许重复出现)。
const DTO_COLUMNS: &str = "endpoint_id, is_streaming, upstream_response_body,
    client_user_agent, client_version, client_ip, entry_kind, downstream_http_version,
    client_effort, effort_source, upstream_effort, tool_result_count, tool_use_count";

fn dto_from_row(r: &sqlx::sqlite::SqliteRow, auth: &HashMap<String, AuthType>) -> RequestLogDto {
    RequestLogDto {
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
        first_content_ms: r.try_get("first_content_ms").ok(),
        output_tokens_per_second: row_output_speed(r),
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
        client_effort: r.try_get("client_effort").ok(),
        effective_effort: r.try_get("effective_effort").ok(),
        effort_source: r.try_get("effort_source").ok(),
        upstream_effort: r.try_get("upstream_effort").ok(),
        stop_reason: r.try_get("stop_reason").ok(),
        tools_offered_count: r.try_get("tools_offered_count").ok(),
        tool_result_count: r.try_get("tool_result_count").ok(),
        tool_use_count: r.try_get("tool_use_count").ok(),
        tool_use_names: r.try_get("tool_use_names").ok(),
        summary: live_feed::from_row(r, auth),
        net: crate::proxy::net_timing::NetTimingFields::from_row(r),
    }
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
    let count_q = bind_all(sqlx::query(sqlx::AssertSqlSafe(count_sql.as_str())), &binds);
    let total: i64 = count_q.fetch_one(&state.db).await?.try_get("c")?;

    let select_sql = format!(
        "SELECT {SUMMARY_COLUMNS}, {DTO_COLUMNS}
         FROM requests{where_clause}
         ORDER BY timestamp DESC, id DESC
         LIMIT ? OFFSET ?"
    );
    let rows = bind_all(sqlx::query(sqlx::AssertSqlSafe(select_sql.as_str())), &binds)
        .bind(page_size as i64)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    let auth = live_feed::auth_types(&state).await;
    let items = rows.iter().map(|r| dto_from_row(r, &auth)).collect();

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

/// 请求详情弹窗的三组请求头 (requests 表 migration 027 的三列)。不在列表接口里:
/// 每行可达 16 KB × 3, 只在打开详情时按 id 取。列为 NULL (老日志 / 关了记录) 或无法解析 → None。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RequestHeadersDto {
    pub client_request: Option<Vec<[String; 2]>>,
    /// `[name, value, source]`, source as in `proxy::header_source::HeaderSource`.
    pub upstream_request: Option<Vec<[String; 3]>>,
    pub upstream_response: Option<Vec<[String; 2]>>,
}

pub(crate) async fn load_request_headers(
    pool: &sqlx::SqlitePool,
    id: &str,
) -> AppResult<RequestHeadersDto> {
    let row = sqlx::query(
        "SELECT client_request_headers, upstream_request_headers, upstream_response_headers
         FROM requests WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| crate::error::AppError::BadRequest("请求记录不存在".into()))?;
    let raw = |name: &str| row.try_get::<Option<String>, _>(name).ok().flatten();
    let col = |name: &str| raw(name).and_then(|s| crate::proxy::header_log::parse(&s));
    Ok(RequestHeadersDto {
        client_request: col("client_request_headers"),
        upstream_request: raw("upstream_request_headers").and_then(|s| serde_json::from_str(&s).ok()),
        upstream_response: col("upstream_response_headers"),
    })
}

#[tauri::command]
pub async fn get_request_headers(state: State<'_, AppState>, id: String) -> AppResult<RequestHeadersDto> {
    load_request_headers(&state.db, &id).await
}

/// CSV 列 (与 RequestLogDto 对齐, 刻意不含 upstream_response_body —— 大且多行,
/// 排障专用不适合表格; 另在 timestamp 旁加人类可读的 timestamp_iso)。
const CSV_HEADER: &str = "id,timestamp,timestamp_iso,virtual_model_name,subscription_id,\
provider_id,endpoint_id,real_model_name,response_model_name,is_streaming,status,http_status,\
total_latency_ms,input_tokens,output_tokens,cache_creation_tokens,cache_read_tokens,\
error_message,client_tool,client_user_agent,client_version,client_ip,entry_kind,\
downstream_http_version,client_effort,effective_effort,effort_source,upstream_effort,\
stop_reason,tools_offered_count,tool_result_count,tool_use_count,tool_use_names,first_content_ms,output_tokens_per_second,\
purpose_kind,purpose_turn,purpose_step,session_id,trace_id";

/// 把当前筛选下的请求逐行写成 CSV (带 UTF-8 BOM + 表头), 返回行数.
/// 桌面导出写文件, 网页导出写 Vec<u8>, 共享同一份 SQL 与格式化.
pub async fn write_csv<W: Write>(
    state: &AppState,
    filters: RequestLogFilters,
    mut w: W,
) -> AppResult<u64> {
    let (where_clause, binds) = build_filter_clause(&filters);

    // 导出按时间正序, 符合表格阅读习惯 (页面展示是倒序)
    let sql = format!(
        "SELECT id, timestamp, virtual_model_name, subscription_id, provider_id, endpoint_id,
                real_model_name, response_model_name, is_streaming, status,
                http_status, total_latency_ms, first_content_ms,
                upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read, error_message,
                client_tool, client_user_agent, client_version, client_ip,
                entry_kind, downstream_http_version,
                client_effort, effective_effort, effort_source, upstream_effort,
                stop_reason, tools_offered_count, tool_result_count, tool_use_count, tool_use_names,
                purpose_kind, purpose_turn, purpose_step, session_id, trace_id
         FROM requests{}
         ORDER BY timestamp ASC",
        where_clause
    );
    let q = bind_all(sqlx::query(sqlx::AssertSqlSafe(sql.as_str())), &binds);

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
            opt_str(r.try_get("client_effort").ok()),
            opt_str(r.try_get("effective_effort").ok()),
            opt_str(r.try_get("effort_source").ok()),
            opt_str(r.try_get("upstream_effort").ok()),
            opt_str(r.try_get("stop_reason").ok()),
            opt_num(r.try_get("tools_offered_count").ok()),
            opt_num(r.try_get("tool_result_count").ok()),
            opt_num(r.try_get("tool_use_count").ok()),
            opt_str(r.try_get("tool_use_names").ok()),
            opt_num(r.try_get("first_content_ms").ok()),
            row_output_speed(&r).map(|v| v.to_string()).unwrap_or_default(),
            opt_str(r.try_get("purpose_kind").ok()),
            opt_num(r.try_get("purpose_turn").ok()),
            opt_num(r.try_get("purpose_step").ok()),
            opt_str(r.try_get("session_id").ok()),
            opt_str(r.try_get("trace_id").ok()),
        ]
        .join(",");
        writeln!(w, "{}", line).map_err(io_err)?;
        count += 1;
    }
    w.flush().map_err(io_err)?;
    Ok(count)
}

/// 按当前筛选条件把请求日志导出为 CSV 文件 (路径由前端 save dialog 提供)。
/// 流式逐行写 BufWriter, 不整表载入内存; 返回导出的行数。
/// 文件头带 UTF-8 BOM, Excel 直接打开中文不乱码。
#[tauri::command]
pub async fn export_requests_csv(
    state: State<'_, AppState>,
    path: String,
    filters: Option<RequestLogFilters>,
) -> AppResult<u64> {
    let file = std::fs::File::create(&path)
        .map_err(|e| crate::error::AppError::Internal(format!("创建导出文件失败: {e}")))?;
    let w = std::io::BufWriter::new(file);
    write_csv(&state, filters.unwrap_or_default(), w).await
}

/// 网页端导出: 返回 CSV 文本 (含 BOM), 由浏览器下载. Task 13 与 export_requests_csv 合并实现.
#[tauri::command]
pub async fn export_requests_csv_text(
    state: State<'_, AppState>,
    filters: Option<RequestLogFilters>,
) -> AppResult<String> {
    let mut buf: Vec<u8> = Vec::new();
    write_csv(&state, filters.unwrap_or_default(), &mut buf).await?;
    String::from_utf8(buf).map_err(|e| crate::error::AppError::Internal(format!("CSV 非 UTF-8: {e}")))
}

#[derive(Debug, Serialize)]
pub struct RequestDetailDto {
    pub request: RequestLogDto,
    /// 同一次入站请求的全部尝试 (含本条), 按时间正序; 老记录没有 trace_id 时只有本条
    pub attempts: Vec<RequestSummary>,
    /// 按当前价格表估算的费用 (没有启用的计价插件 / 没有匹配的价格 → None)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<crate::plugins::pricing::CostLine>,
}

/// 详情页: 一条请求的完整字段 + 同 trace 的全部尝试。找不到返回 None
/// (从实时路由跳来时这条可能还在批量写入队列里, 前端据此显示「正在写入」并重试)。
#[tauri::command]
pub async fn get_request_detail(state: State<'_, AppState>, id: String) -> AppResult<Option<RequestDetailDto>> {
    let sql = format!("SELECT {SUMMARY_COLUMNS}, {DTO_COLUMNS} FROM requests WHERE id = ?");
    let Some(row) = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(None);
    };
    let auth = live_feed::auth_types(&state).await;
    let request = dto_from_row(&row, &auth);
    let attempts = match request.summary.trace_id.as_deref() {
        Some(trace) => {
            let sql = format!("SELECT {SUMMARY_COLUMNS} FROM requests WHERE trace_id = ? ORDER BY timestamp ASC, id ASC");
            sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                .bind(trace)
                .fetch_all(&state.db)
                .await?
                .iter()
                .map(|r| live_feed::from_row(r, &auth))
                .collect()
        }
        None => vec![request.summary.clone()],
    };
    let cost = {
        let pricing = state.pricing();
        let ctx = crate::plugins::pricing::subscription_contexts(&state).await;
        let (auth_type, billing) = ctx.get(&request.subscription_id).copied().unzip();
        let n = |v: Option<i64>| v.unwrap_or(0).max(0) as u64;
        crate::plugins::pricing::cost_of(
            &pricing,
            &request.provider_id,
            &request.real_model_name,
            auth_type,
            billing.unwrap_or_default(),
            crate::plugins::pricing::Tokens {
                input_raw: n(request.input_tokens),
                output: n(request.output_tokens),
                cache_read: n(request.cache_read_tokens),
                cache_write: n(request.cache_creation_tokens),
            },
        )
    };
    Ok(Some(RequestDetailDto { request, attempts, cost }))
}

/// 同一会话里时间上最接近 `around_ms` 的请求 (前后各最多 `radius` 条), 按时间正序。
#[tauri::command]
pub async fn list_session_requests(
    state: State<'_, AppState>,
    session_id: String,
    around_ms: i64,
    radius: Option<u32>,
) -> AppResult<Vec<RequestSummary>> {
    let n = radius.unwrap_or(4).clamp(1, 20) as i64;
    let auth = live_feed::auth_types(&state).await;
    let before = format!(
        "SELECT {SUMMARY_COLUMNS} FROM requests WHERE session_id = ? AND timestamp <= ? ORDER BY timestamp DESC, id DESC LIMIT ?"
    );
    let after = format!(
        "SELECT {SUMMARY_COLUMNS} FROM requests WHERE session_id = ? AND timestamp > ? ORDER BY timestamp ASC, id ASC LIMIT ?"
    );
    let mut out: Vec<RequestSummary> = sqlx::query(sqlx::AssertSqlSafe(before.as_str()))
        .bind(&session_id)
        .bind(around_ms)
        .bind(n + 1)
        .fetch_all(&state.db)
        .await?
        .iter()
        .rev()
        .map(|r| live_feed::from_row(r, &auth))
        .collect();
    out.extend(
        sqlx::query(sqlx::AssertSqlSafe(after.as_str()))
            .bind(&session_id)
            .bind(around_ms)
            .bind(n)
            .fetch_all(&state.db)
            .await?
            .iter()
            .map(|r| live_feed::from_row(r, &auth)),
    );
    Ok(out)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct SessionFacet {
    pub client: Option<String>,
    pub id: String,
    pub count: i64,
    pub last_seen: i64,
}

#[derive(Debug, Serialize, Default, PartialEq)]
pub struct RequestFacetsDto {
    /// 与实时路由的用途 chips 同义: all / main / agent / bg / fail
    pub purpose: HashMap<&'static str, i64>,
    /// 当前筛选下最近活跃的会话 (最多 8 个)
    pub sessions: Vec<SessionFacet>,
}

/// 列表顶部 chips 的计数。用途计数不受「用途」筛选影响, 会话列表不受「会话」筛选影响。
#[tauri::command]
pub async fn get_request_facets(
    state: State<'_, AppState>,
    filters: Option<RequestLogFilters>,
) -> AppResult<RequestFacetsDto> {
    request_facets(&state.db, &filters.unwrap_or_default()).await
}

pub(crate) async fn request_facets(db: &sqlx::SqlitePool, filters: &RequestLogFilters) -> AppResult<RequestFacetsDto> {
    let bg = BACKGROUND_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
    let (where_clause, binds) = build_filter_clause_except(filters, &["purpose"]);
    let sql = format!(
        "SELECT COUNT(*) AS a,
                COALESCE(SUM(purpose_kind = 'main'), 0) AS m,
                COALESCE(SUM(purpose_kind = 'agent'), 0) AS g,
                COALESCE(SUM(purpose_kind IN ({bg})), 0) AS b,
                COALESCE(SUM(status != 'success'), 0) AS f
         FROM requests{where_clause}"
    );
    let r = bind_all(sqlx::query(sqlx::AssertSqlSafe(sql.as_str())), &binds)
        .fetch_one(db)
        .await?;
    let mut purpose = HashMap::new();
    for (k, c) in [("all", "a"), ("main", "m"), ("agent", "g"), ("bg", "b"), ("fail", "f")] {
        purpose.insert(k, r.try_get::<i64, _>(c).unwrap_or(0));
    }

    let (where_clause, binds) = build_filter_clause_except(filters, &["session_id"]);
    let joiner = if where_clause.is_empty() { " WHERE" } else { " AND" };
    let sql = format!(
        "SELECT session_id, MAX(session_client) AS client, COUNT(*) AS c, MAX(timestamp) AS last
         FROM requests{where_clause}{joiner} session_id IS NOT NULL
         GROUP BY session_id ORDER BY last DESC LIMIT 8"
    );
    let sessions = bind_all(sqlx::query(sqlx::AssertSqlSafe(sql.as_str())), &binds)
        .fetch_all(db)
        .await?
        .iter()
        .map(|r| SessionFacet {
            client: r.try_get("client").ok(),
            id: r.try_get("session_id").unwrap_or_default(),
            count: r.try_get("c").unwrap_or(0),
            last_seen: r.try_get("last").unwrap_or(0),
        })
        .collect();
    Ok(RequestFacetsDto { purpose, sessions })
}

#[derive(Debug, Serialize, Default, PartialEq)]
pub struct RequestNeighborsDto {
    /// 列表里排在它上面 (更新) 的那条
    pub newer: Option<String>,
    /// 列表里排在它下面 (更早) 的那条
    pub older: Option<String>,
}

/// 详情页的「上一条 / 下一条」: 在当前筛选结果里, 按列表同样的排序 (timestamp DESC, id DESC) 找邻居。
#[tauri::command]
pub async fn get_request_neighbors(
    state: State<'_, AppState>,
    id: String,
    filters: Option<RequestLogFilters>,
) -> AppResult<RequestNeighborsDto> {
    request_neighbors(&state.db, &id, &filters.unwrap_or_default()).await
}

pub(crate) async fn request_neighbors(
    db: &sqlx::SqlitePool,
    id: &str,
    filters: &RequestLogFilters,
) -> AppResult<RequestNeighborsDto> {
    let Some(ts) = sqlx::query_scalar::<_, i64>("SELECT timestamp FROM requests WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
    else {
        return Ok(RequestNeighborsDto::default());
    };
    let (where_clause, binds) = build_filter_clause(filters);
    let joiner = if where_clause.is_empty() { " WHERE" } else { " AND" };
    let pick = |cmp: &str, order: &str| {
        format!(
            "SELECT id FROM requests{where_clause}{joiner} (timestamp {cmp} ? OR (timestamp = ? AND id {cmp} ?))
             ORDER BY timestamp {order}, id {order} LIMIT 1"
        )
    };
    let mut out = RequestNeighborsDto::default();
    for (cmp, order, slot) in [(">", "ASC", &mut out.newer), ("<", "DESC", &mut out.older)] {
        let sql = pick(cmp, order);
        *slot = bind_all(sqlx::query(sqlx::AssertSqlSafe(sql.as_str())), &binds)
            .bind(ts)
            .bind(ts)
            .bind(id)
            .fetch_optional(db)
            .await?
            .and_then(|r| r.try_get("id").ok());
    }
    Ok(out)
}

/// 返回前端可在筛选器里展示的「已支持识别的 client tool」白名单。
/// 数据源是 [`crate::proxy::client_fingerprint::SUPPORTED_TOOLS`], 后端单一信息源,
/// 前端硬编码的 i18n 文案需手工同步 (类比 `ProviderLogo BRAND_MAP`).
#[tauri::command]
pub async fn list_supported_client_tools() -> AppResult<Vec<&'static str>> {
    Ok(crate::proxy::client_fingerprint::SUPPORTED_TOOLS.to_vec())
}

/// Live Routing「客户端接入」的聚合行: 某个客户端最近有没有真的把请求发进来。
/// `client_tool = None` 是未识别桶 (UA 认不出, 或迁移 009 之前的老日志)。
#[derive(Debug, Clone, Serialize)]
pub struct ClientActivityDto {
    pub client_tool: Option<String>,
    pub request_count: i64,
    /// 最近一次请求时间 (ms epoch)
    pub last_seen: i64,
    /// 上游输入 + 输出 token 之和 (不含缓存读写), 与统计页按订阅汇总同一口径;
    /// 拿不到用量的行 (失败 / 老日志) 按 0 计, 同日聚合表的 `unwrap_or(0)`
    pub total_tokens: i64,
}

/// 按 client_tool 聚合 requests: 条数 + 最近时间, 按最近时间倒序。
/// 与 command 分离, 单测直接打 (类比 statistics.rs::query_tool_breakdown)。
/// 不做时间窗过滤: 日志本身受 `log_retention_days` 清理, 保留期即天然窗口。
pub(crate) async fn query_client_activity(
    pool: &sqlx::SqlitePool,
) -> Result<Vec<ClientActivityDto>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT client_tool, COUNT(*) AS request_count, MAX(timestamp) AS last_seen,
                SUM(COALESCE(upstream_input_tokens, 0) + COALESCE(upstream_output_tokens, 0))
                    AS total_tokens
         FROM requests
         GROUP BY client_tool
         ORDER BY last_seen DESC",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(ClientActivityDto {
                client_tool: r.try_get("client_tool")?,
                request_count: r.try_get("request_count")?,
                last_seen: r.try_get("last_seen")?,
                total_tokens: r.try_get("total_tokens")?,
            })
        })
        .collect()
}

/// 哪些客户端最近确实走了 cc-router (被动流量检测, 零配置)。
/// 从未发过请求的客户端不会出现在结果里, 前端按「未接入」灰显。
#[tauri::command]
pub async fn get_client_activity(state: State<'_, AppState>) -> AppResult<Vec<ClientActivityDto>> {
    query_client_activity(&state.db).await.map_err(Into::into)
}

/// One calculation for the table DTO and both CSV exporters.
fn row_output_speed(row: &sqlx::sqlite::SqliteRow) -> Option<f64> {
    crate::proxy::request_timing::output_tokens_per_second(
        row.try_get::<i64, _>("is_streaming").unwrap_or(0) != 0,
        &row.try_get::<String, _>("status").unwrap_or_default(),
        row.try_get("upstream_output_tokens").ok(),
        row.try_get("total_latency_ms").ok(),
        row.try_get("first_content_ms").ok(),
    )
}


#[cfg(test)]
mod tests {
    use super::{csv_field, request_facets, request_neighbors, RequestLogFilters, CSV_HEADER};

    /// 插入一行最小可用的 requests 记录 (只关心 client_tool / timestamp 聚合)。
    async fn insert_request(
        pool: &sqlx::SqlitePool,
        id: &str,
        ts: i64,
        tool: Option<&str>,
        tokens: Option<(i64, i64)>,
    ) {
        sqlx::query(
            "INSERT INTO requests (id, timestamp, virtual_model_name, subscription_id, provider_id,
             endpoint_id, real_model_name, is_streaming, status, client_tool,
             upstream_input_tokens, upstream_output_tokens)
             VALUES (?, ?, 'vm', 's', 'p', 'e', 'm', 0, 'success', ?, ?, ?)",
        )
        .bind(id)
        .bind(ts)
        .bind(tool)
        .bind(tokens.map(|t| t.0))
        .bind(tokens.map(|t| t.1))
        .execute(pool)
        .await
        .unwrap();
    }

    async fn header_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    /// 带用途 / 会话 / trace 的行 (迁移 030 之后的形状)
    async fn insert_rich(pool: &sqlx::SqlitePool, id: &str, ts: i64, purpose: Option<&str>, status: &str, session: Option<&str>, model: &str) {
        sqlx::query(
            "INSERT INTO requests (id, timestamp, virtual_model_name, subscription_id, provider_id,
             endpoint_id, real_model_name, is_streaming, status, purpose_kind, session_id, session_client, tool_use_names)
             VALUES (?, ?, 'opus', 's', 'p', 'e', ?, 0, ?, ?, ?, 'claude', '[\"WebSearch\"]')",
        )
        .bind(id)
        .bind(ts)
        .bind(model)
        .bind(status)
        .bind(purpose)
        .bind(session)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn ids(pool: &sqlx::SqlitePool, f: &RequestLogFilters) -> Vec<String> {
        let (w, b) = super::build_filter_clause(f);
        let sql = format!("SELECT id FROM requests{w} ORDER BY timestamp DESC, id DESC");
        super::bind_all(sqlx::query(sqlx::AssertSqlSafe(sql.as_str())), &b)
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .map(|r| sqlx::Row::get::<String, _>(r, "id"))
            .collect()
    }

    async fn rich_pool() -> sqlx::SqlitePool {
        let pool = header_pool().await;
        insert_rich(&pool, "a", 10, Some("main"), "success", Some("s1"), "claude-opus").await;
        insert_rich(&pool, "b", 20, Some("agent"), "success", Some("s1"), "claude-haiku").await;
        insert_rich(&pool, "c", 30, Some("title"), "error", Some("s2"), "deepseek_v4").await;
        insert_rich(&pool, "d", 30, None, "success", None, "legacy-model").await;
        pool
    }

    #[tokio::test]
    async fn filters_by_purpose_session_time_and_search() {
        let pool = rich_pool().await;
        let f = |purpose: Option<&str>| RequestLogFilters { purpose: purpose.map(str::to_owned), ..Default::default() };
        assert_eq!(ids(&pool, &f(Some("main"))).await, ["a"]);
        assert_eq!(ids(&pool, &f(Some("bg"))).await, ["c"]);
        assert_eq!(ids(&pool, &f(Some("fail"))).await, ["c"]);
        let s1 = RequestLogFilters { session_id: Some("s1".into()), since_ms: Some(15), ..Default::default() };
        assert_eq!(ids(&pool, &s1).await, ["b"]);
        let q = |q: &str| RequestLogFilters { q: Some(q.into()), ..Default::default() };
        assert_eq!(ids(&pool, &q("haiku")).await, ["b"]);
        // `_` 是 LIKE 通配符, 必须按字面量匹配
        assert_eq!(ids(&pool, &q("v4")).await, ["c"]);
        assert_eq!(ids(&pool, &q("e_s")).await, Vec::<String>::new(), "e_s 当通配符会命中 deepseek");
        assert_eq!(ids(&pool, &q("_v4")).await, ["c"]);
        assert_eq!(ids(&pool, &q("websearch")).await.len(), 4, "工具名也能搜 (LIKE 不分大小写)");
    }

    #[tokio::test]
    async fn facets_ignore_their_own_dimension() {
        let pool = rich_pool().await;
        let f = RequestLogFilters { purpose: Some("main".into()), ..Default::default() };
        let facets = request_facets(&pool, &f).await.unwrap();
        assert_eq!(facets.purpose["all"], 4, "用途计数不受用途筛选影响");
        assert_eq!((facets.purpose["main"], facets.purpose["agent"], facets.purpose["bg"], facets.purpose["fail"]), (1, 1, 1, 1));
        // 会话列表受用途筛选影响 (只剩 main 那条所在的 s1), 按最近活跃排序
        assert_eq!(facets.sessions.iter().map(|s| (s.id.as_str(), s.count)).collect::<Vec<_>>(), [("s1", 1)]);
        let all = request_facets(&pool, &RequestLogFilters { session_id: Some("s2".into()), ..Default::default() }).await.unwrap();
        assert_eq!(all.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2", "s1"], "会话列表不受会话筛选影响");
    }

    #[tokio::test]
    async fn neighbors_follow_list_order_and_filters() {
        let pool = rich_pool().await;
        // 列表顺序 (timestamp DESC, id DESC): d, c, b, a —— c / d 同毫秒, 按 id 定序
        let none = RequestLogFilters::default();
        let n = request_neighbors(&pool, "c", &none).await.unwrap();
        assert_eq!((n.newer.as_deref(), n.older.as_deref()), (Some("d"), Some("b")));
        let n = request_neighbors(&pool, "d", &none).await.unwrap();
        assert_eq!((n.newer.as_deref(), n.older.as_deref()), (None, Some("c")));
        let s1 = RequestLogFilters { session_id: Some("s1".into()), ..Default::default() };
        let n = request_neighbors(&pool, "b", &s1).await.unwrap();
        assert_eq!((n.newer.as_deref(), n.older.as_deref()), (None, Some("a")));
        assert_eq!(request_neighbors(&pool, "nope", &none).await.unwrap(), Default::default());
    }

    #[tokio::test]
    async fn first_content_migration_preserves_unknown_historical_values() {
        let pool = header_pool().await;
        insert_request(&pool, "old", 1000, None, Some((10, 200))).await;
        let first: Option<i64> = sqlx::query_scalar("SELECT first_content_ms FROM requests WHERE id = 'old'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(first, None);
        sqlx::query("UPDATE requests SET first_content_ms = 1200 WHERE id = 'old'")
            .execute(&pool).await.unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let first: Option<i64> = sqlx::query_scalar("SELECT first_content_ms FROM requests WHERE id = 'old'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(first, Some(1200));
    }

    #[tokio::test]
    async fn detail_returns_parsed_headers() {
        let pool = header_pool().await;
        insert_request(&pool, "h1", 1000, Some("claude-code"), None).await;
        sqlx::query(
            "UPDATE requests SET client_request_headers = ?, upstream_request_headers = ?,
             upstream_response_headers = ? WHERE id = 'h1'",
        )
        .bind(r#"[["x-app","cli"],["anthropic-beta","a"]]"#)
        .bind(r#"[["x-api-key","sk-a…cdef","auth"]]"#)
        .bind(r#"[["request-id","r1"]]"#)
        .execute(&pool)
        .await
        .unwrap();
        let d = super::load_request_headers(&pool, "h1").await.unwrap();
        let pair = |a: &str, b: &str| [a.to_string(), b.to_string()];
        assert_eq!(d.client_request, Some(vec![pair("x-app", "cli"), pair("anthropic-beta", "a")]));
        assert_eq!(
            d.upstream_request,
            Some(vec![["x-api-key".to_string(), "sk-a…cdef".to_string(), "auth".to_string()]])
        );
        assert_eq!(d.upstream_response, Some(vec![pair("request-id", "r1")]));
    }

    #[tokio::test]
    async fn detail_returns_none_for_unrecorded_rows() {
        let pool = header_pool().await;
        insert_request(&pool, "h2", 1000, None, None).await;
        sqlx::query("UPDATE requests SET upstream_request_headers = 'garbage' WHERE id = 'h2'")
            .execute(&pool)
            .await
            .unwrap();
        let d = super::load_request_headers(&pool, "h2").await.unwrap();
        assert_eq!(d, super::RequestHeadersDto { client_request: None, upstream_request: None, upstream_response: None });
        assert!(super::load_request_headers(&pool, "missing").await.is_err());
    }

    /// 聚合按 client_tool 分组: 未识别 (NULL) 是独立一桶, 整体按 last_seen 倒序。
    #[tokio::test]
    async fn client_activity_groups_unknown_and_orders_by_last_seen() {
        use crate::db::run_migrations;
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        insert_request(&pool, "r1", 1000, Some("claude-code"), Some((100, 20))).await;
        insert_request(&pool, "r2", 3000, Some("claude-code"), Some((1_000, 5))).await;
        insert_request(&pool, "r3", 2000, Some("codex-cli"), None).await;
        insert_request(&pool, "r4", 4000, None, Some((7, 0))).await;

        let rows = super::query_client_activity(&pool).await.unwrap();
        assert_eq!(rows.len(), 3, "NULL 独立成组, 不与任何已知工具合并");
        assert_eq!(
            (
                rows[0].client_tool.as_deref(),
                rows[0].request_count,
                rows[0].last_seen
            ),
            (None, 1, 4000)
        );
        assert_eq!(
            (rows[1].client_tool.as_deref(), rows[1].request_count, rows[1].last_seen),
            (Some("claude-code"), 2, 3000)
        );
        assert_eq!(rows[2].client_tool.as_deref(), Some("codex-cli"));
        // token = 输入 + 输出; 全组都没有用量 (NULL) 时是 0 而不是 NULL
        let tokens: Vec<i64> = rows.iter().map(|r| r.total_tokens).collect();
        assert_eq!(tokens, vec![7, 1_125, 0]);
    }

    /// CSV 列数锁 —— 加字段时必须同步改 header / SELECT / 行写入三处, 漏一处就会串列。
    #[test]
    fn csv_header_has_expected_columns() {
        let cols: Vec<&str> = CSV_HEADER.split(',').collect();
        assert_eq!(cols.len(), 40, "改了 CSV 列数? 同步 SELECT 与行写入");
        assert_eq!(
            &cols[cols.len() - 5..],
            &["purpose_kind", "purpose_turn", "purpose_step", "session_id", "trace_id"]
        );
        assert_eq!(
            &cols[cols.len() - 16..cols.len() - 5],
            &[
                "client_effort",
                "effective_effort",
                "effort_source",
                "upstream_effort",
                "stop_reason",
                "tools_offered_count",
                "tool_result_count",
                "tool_use_count",
                "tool_use_names",
                "first_content_ms",
                "output_tokens_per_second",
            ]
        );
        // 刻意排除的大字段, 不能悄悄混进来
        assert!(!CSV_HEADER.contains("upstream_response_body"));
    }

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
