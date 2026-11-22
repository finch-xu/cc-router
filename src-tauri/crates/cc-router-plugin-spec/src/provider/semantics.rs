use crate::provider::model::Provider;
use crate::{SpecError, SpecResult};

/// schema 表达不了的跨字段约束。失败即该 yaml 加载失败 (load_all warn + 跳过, 单测在 CI 拦住)。
pub fn validate(p: &Provider) -> SpecResult<()> {
    use crate::provider::model::{AuthType, EndpointProtocol, SystemoneWire};
    use crate::provider::url_template::{placeholders, RESERVED_API_KEY, RESERVED_MODEL};

    let fail = |msg: String| Err(SpecError::Invalid(msg));

    // 1. declarations
    let mut ids: Vec<&str> = Vec::new();
    for param in &p.url_params {
        let ok_ident = !param.id.is_empty() && param.id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
        if !ok_ident || param.id == RESERVED_MODEL || param.id == RESERVED_API_KEY {
            return fail(format!("url_params '{}': id 只能是小写字母与下划线, 且不能是保留字 model / api_key", param.id));
        }
        if ids.contains(&param.id.as_str()) {
            return fail(format!("url_params '{}' 重复声明", param.id));
        }
        if regex::Regex::new(&param.pattern).is_err() {
            return fail(format!("url_params '{}': pattern 不是合法正则", param.id));
        }
        ids.push(&param.id);
    }

    // 2. every placeholder is declared, or a reserved name in its allowed place
    let check = |place: &str, s: &str, allow: Option<&str>| -> SpecResult<()> {
        for name in placeholders(s) {
            if ids.contains(&name.as_str()) || allow == Some(name.as_str()) {
                continue;
            }
            return Err(SpecError::Invalid(format!("{place}: 占位符 {{{name}}} 未在 url_params 声明或不允许出现在这里")));
        }
        Ok(())
    };
    for e in &p.endpoints {
        if e.protocol == EndpointProtocol::Systemone && p.auth.auth_type != AuthType::ApiKey {
            return fail(format!("endpoint '{}': protocol systemone 只允许出现在 auth.type = api_key 的 provider 下", e.id));
        }
        if e.systemone_wire != SystemoneWire::Standard && e.protocol != EndpointProtocol::Systemone {
            return fail(format!("endpoint '{}': systemone_wire 只能用在 protocol: systemone 的端点", e.id));
        }
        check(&format!("endpoint '{}' base_url", e.id), &e.base_url, None)?;
        // Only the systemone and Gemini dispatches fill {model}; anywhere else it would be sent literally.
        let fills_model = e.protocol == EndpointProtocol::Systemone
            || matches!(p.auth.auth_type, AuthType::GeminiApiKey | AuthType::GeminiInteractionsApiKey);
        check(&format!("endpoint '{}' messages_path", e.id), &e.messages_path, fills_model.then_some(RESERVED_MODEL))?;
        for (k, v) in &e.headers {
            check(&format!("endpoint '{}' header {k}", e.id), v, Some(RESERVED_API_KEY))?;
        }
    }
    for (k, v) in &p.required_headers {
        check(&format!("required_headers {k}"), v, Some(RESERVED_API_KEY))?;
    }
    if let Some(u) = p.model_discovery.url.as_deref() {
        check("model_discovery.url", u, None)?;
    }

    // 3. every declared param is used by at least one endpoint
    for param in &p.url_params {
        if !p.endpoints.iter().any(|e| p.params_used(e).contains(&param.id)) {
            return fail(format!("url_params '{}' 声明了但没有任何端点用到", param.id));
        }
    }
    Ok(())
}
