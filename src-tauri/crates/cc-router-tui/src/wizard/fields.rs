//! 两条路径各自的字段、校验与预填。与 `mod.rs` 分开是因为这些是**纯数据与纯函数**: 给定
//! 一份草稿, 算出要画哪些行、哪些字段不合法。没有 `Frame`, 没有 `Cmd`, 好测。

use super::text::{SecretField, TextField, TextInput};
use crate::client::dto::{AuthHeaderFormat, CustomProtocol, ModelInfo, ModelSlots, Provider, Slot};
use crate::i18n::Strings;
use crate::store::Store;

/// 内置路径第一步的草稿。文本字段直接持有输入框 (`TextField`/`SecretField`, 值只存这一份)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BasicsDraft {
    pub provider_id: String,
    pub endpoint_id: String,
    pub api_key: SecretField,
    pub display_name: TextField,
    /// 当前接入点需要的 URL 参数行, 由 `sync_url_params` 随厂商 / 接入点重建。
    pub url_params: Vec<UrlParamDraft>,
}

/// 一个 URL 参数行的草稿: 声明里的 id / 标签 / 格式, 加上用户输入的值。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrlParamDraft {
    pub id: String,
    pub label: String,
    pub pattern: String,
    pub value: TextField,
}

/// Rebuild the param rows for the chosen endpoint, keeping values the user already typed for
/// ids that are still needed (direct -> gateway keeps the account id).
pub fn sync_url_params(current: &[UrlParamDraft], provider: &Provider, endpoint_id: &str) -> Vec<UrlParamDraft> {
    let Some(ep) = provider.endpoints.iter().find(|e| e.id == endpoint_id) else { return Vec::new() };
    ep.url_params_used
        .iter()
        .filter_map(|id| provider.url_params.iter().find(|u| &u.id == id))
        .map(|u| UrlParamDraft {
            id: u.id.clone(),
            label: u.label.clone(),
            pattern: u.pattern.clone(),
            value: current.iter().find(|c| c.id == u.id).map(|c| c.value.clone()).unwrap_or_default(),
        })
        .collect()
}

impl BasicsDraft {
    /// 焦点所在的文本字段; 选择行 / 按钮行没有。
    pub fn text_field(&mut self, field: BasicsField) -> Option<&mut dyn TextInput> {
        match field {
            BasicsField::ApiKey => Some(&mut self.api_key),
            BasicsField::DisplayName => Some(&mut self.display_name),
            BasicsField::UrlParam(i) => self.url_params.get_mut(i).map(|p| &mut p.value as &mut dyn TextInput),
            BasicsField::Provider | BasicsField::Endpoint | BasicsField::Submit => None,
        }
    }
}

/// 第一步的字段。顺序即上下键的顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicsField {
    Provider,
    Endpoint,
    /// 下标指向 `BasicsDraft.url_params`。
    UrlParam(usize),
    ApiKey,
    DisplayName,
    Submit,
}

impl BasicsField {
    pub const ALL: [BasicsField; 5] =
        [BasicsField::Provider, BasicsField::Endpoint, BasicsField::ApiKey, BasicsField::DisplayName, BasicsField::Submit];

    /// Field order with the endpoint's params inserted after `Endpoint`.
    pub fn order(param_count: usize) -> Vec<BasicsField> {
        let mut out = Vec::with_capacity(Self::ALL.len() + param_count);
        for field in Self::ALL {
            out.push(field);
            if field == BasicsField::Endpoint {
                out.extend((0..param_count).map(BasicsField::UrlParam));
            }
        }
        out
    }
}

/// 校验失败的字段与原因 (原因是 `Strings` 的字段, 不是字面量)。第一个不合法的字段决定光标落点——
/// 顺序与 `BasicsField::order` 一致: 厂商 → 接入点 → URL 参数 → API Key → 备注名 (`Submit` 本身不参与校验,
/// 它是触发校验的那个按钮, 不可能是校验失败的对象)。
pub fn validate_basics(d: &BasicsDraft, s: &'static Strings) -> Option<(BasicsField, &'static str)> {
    if d.provider_id.is_empty() {
        return Some((BasicsField::Provider, s.wiz_err_provider));
    }
    if d.endpoint_id.is_empty() {
        return Some((BasicsField::Endpoint, s.wiz_err_endpoint));
    }
    for (i, p) in d.url_params.iter().enumerate() {
        let v = p.value.value().trim();
        if v.is_empty() {
            return Some((BasicsField::UrlParam(i), s.wiz_err_url_param_empty));
        }
        if !regex::Regex::new(&p.pattern).is_ok_and(|re| re.is_match(v)) {
            return Some((BasicsField::UrlParam(i), s.wiz_err_url_param_format));
        }
    }
    if d.api_key.is_empty() {
        return Some((BasicsField::ApiKey, s.wiz_err_api_key));
    }
    if d.display_name.value().trim().is_empty() {
        return Some((BasicsField::DisplayName, s.wiz_err_display_name));
    }
    None
}

/// 备注名的默认值: 厂商显示名; `Store` 里已经有同名订阅时追加 ` 2` / ` 3` … 直到不重名。
///
/// 桌面端用的是 `<厂商名> <6 位随机 base36>`, TUI 不照抄: 这个 crate 没有随机数依赖 (也不为了
/// 一个后缀去加), 而序号比随机串可读。后端对 `display_name` 没有唯一性约束, 这只是默认值,
/// 用户随时可以改。
pub fn default_display_name(provider_name: &str, store: &Store) -> String {
    let taken = |name: &str| store.subscriptions().iter().any(|sub| sub.display_name == name);
    if !taken(provider_name) {
        return provider_name.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{provider_name} {n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// 第二步 (绑定模型) 的草稿。与订阅页的 `Draft<Subscription>` 不同: 向导是从零填, 没有"与 Store
/// 比对相等就丢弃"的问题, 所以直接放一份 `ModelSlots`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotsDraft {
    pub slots: ModelSlots,
    /// 可选的模型候选 (来自 `refresh_model_list` / `probe_custom_models`), 空 = 只能手输。
    pub models: Vec<ModelInfo>,
}

/// 第二步的字段: 五个槽位行 (`Row`, 带着是哪个 `Slot`) + 保存按钮。顺序即上下键的顺序——与
/// `BasicsField` 同一套 `ALL` + `FormState::step` 写法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotsField {
    Row(Slot),
    Save,
}

impl SlotsField {
    pub const ALL: [SlotsField; 6] = [
        SlotsField::Row(Slot::Fable),
        SlotsField::Row(Slot::Opus),
        SlotsField::Row(Slot::Sonnet),
        SlotsField::Row(Slot::Haiku),
        SlotsField::Row(Slot::Fallback),
        SlotsField::Save,
    ];
}

/// 四个核心槽位都要非空 (兜底槽可以空 = 未配置), 与桌面端 `allSlotsFilled` 同规则。第一个不合法的
/// 槽位决定光标落点, 顺序与 `SlotsField::ALL` 一致 (fable → opus → sonnet → haiku; `Fallback`
/// 不参与, 不可能是校验失败的对象)。
pub fn validate_slots(d: &SlotsDraft, s: &'static Strings) -> Option<(Slot, &'static str)> {
    for slot in [Slot::Fable, Slot::Opus, Slot::Sonnet, Slot::Haiku] {
        if d.slots.get(slot).is_empty() {
            return Some((slot, s.wiz_err_slot));
        }
    }
    None
}

/// 自定义厂商单页的草稿。这一页把「选协议」「填连接信息」「探测模型」「选槽位」放在同一屏, 所以
/// 字段更多; `slots` 直接复用 `SlotsDraft`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomDraft {
    pub protocol: CustomProtocol,
    pub provider_display_name: TextField,
    pub base_url: TextField,
    pub messages_path: TextField,
    /// 锁定协议下恒等于 `protocol.preset()` 的那一对; Anthropic 下是 `ANTHROPIC_AUTH_PRESETS`
    /// 里选的那一对。
    pub auth_header_name: String,
    pub auth_header_format: AuthHeaderFormat,
    pub api_key: SecretField,
    pub display_name: TextField,
    pub slots: SlotsDraft,
    /// 上一次**成功**探测的结果: 那一刻的 `base_url` 与后端回的 `models_url`。换协议、探测失败
    /// 都要清空 (`apply_protocol`); **编辑 `base_url` 本身不清**——`models_url()` 自己按值比对,
    /// 提前清空反而会丢掉"改回去又生效"这条桌面端行为。
    pub probe: Option<ProbedModels>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedModels {
    pub base_url: String,
    pub models_url: String,
}

impl CustomDraft {
    /// 选中厂商 picker 里的 `custom:<protocol>` 条目时构造一份全新草稿——除了协议预设字段,
    /// 其余全是空值 (还没填过任何东西)。内部就是"造一个占位壳 + `apply_protocol`", 不重复
    /// 一遍预设填充逻辑。
    pub fn new(protocol: CustomProtocol) -> Self {
        let mut draft = CustomDraft {
            protocol,
            provider_display_name: TextField::default(),
            base_url: TextField::default(),
            messages_path: TextField::default(),
            auth_header_name: String::new(),
            auth_header_format: AuthHeaderFormat::Bearer,
            api_key: SecretField::default(),
            display_name: TextField::default(),
            slots: SlotsDraft::default(),
            probe: None,
        };
        draft.apply_protocol(protocol);
        draft
    }

    /// 换协议: 把 `base_url` / `messages_path` / 鉴权头重置成这个协议的预设, 并清掉 `probe` 与
    /// 探测到的候选模型 (`slots.models`)——旧协议探测到的模型对新协议没有意义 (OpenAI Responses
    /// 下探测到的 `gpt-5.5` 留在 Gemini 的候选里, 选上就建出模型名对不上协议的订阅)。**已经填进
    /// 槽位的值不动**, **API Key / 备注名也不动** (桌面端同规则——用户切协议大概率是选错了重选,
    /// 不该连已经填好的凭据 / 名字都丢)。表单顶部的说明行由 `CustomForm` 自己清。
    pub fn apply_protocol(&mut self, protocol: CustomProtocol) {
        let preset = protocol.preset();
        self.protocol = protocol;
        self.base_url.set(preset.base_url);
        self.messages_path.set(preset.messages_path);
        self.auth_header_name = preset.auth_header_name.to_string();
        self.auth_header_format = preset.auth_header_format;
        self.probe = None;
        self.slots.models = Vec::new();
    }

    /// 只有探测成功、且此后 `base_url`(trim 后) 一个字都没改过时才回传 `models_url`。与桌面端
    /// `customProbe.baseUrl === baseUrl` 同规则——`probe.base_url` 在探测那一刻已经 trim 过
    /// (见 `wizard::mod::apply_wizard_result` 的 `Probed(Ok(Auto))` 分支), 这里只需要再 trim
    /// 一遍*当前*的 `base_url` 参与比较, 用户中途多打的首尾空白不该算"改过"。
    pub fn models_url(&self) -> Option<&str> {
        match &self.probe {
            Some(p) if p.base_url == self.base_url.value().trim() => Some(p.models_url.as_str()),
            _ => None,
        }
    }

    /// 焦点所在的文本字段; 选择行 / 按钮行 / 槽位行没有。
    pub fn text_field(&mut self, field: CustomField) -> Option<&mut dyn TextInput> {
        match field {
            CustomField::ProviderName => Some(&mut self.provider_display_name),
            CustomField::BaseUrl => Some(&mut self.base_url),
            CustomField::MessagesPath => Some(&mut self.messages_path),
            CustomField::ApiKey => Some(&mut self.api_key),
            CustomField::DisplayName => Some(&mut self.display_name),
            CustomField::Protocol | CustomField::Auth | CustomField::Probe | CustomField::Slot(_) | CustomField::Submit => None,
        }
    }
}

/// 自定义单页的字段, 顺序即上下键的顺序——但与 `BasicsField`/`SlotsField` 不同, 这里的可聚焦
/// 列表**依赖运行时状态** (`Auth` 锁定时被排除), 所以没有固定的 `ALL` 常量, 改用 [`CustomField::all`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomField {
    Protocol,
    ProviderName,
    BaseUrl,
    MessagesPath,
    Auth,
    ApiKey,
    DisplayName,
    Probe,
    Slot(Slot),
    Submit,
}

impl CustomField {
    /// 焦点导航顺序。`auth_locked` 为真时 `Auth` 被剔除在外 (↑↓ 跳过它)——这一行仍然会被画出来
    /// (锁定态, muted), 只是键盘导航永远不会落到它上面; `⏎` 在它身上的时候 (理论上不该发生,
    /// 见 `wizard::common::handle_key` 的 `FieldKind::Locked` 分支) 也什么都不做。
    pub fn all(auth_locked: bool) -> Vec<CustomField> {
        let mut fields = vec![CustomField::Protocol, CustomField::ProviderName, CustomField::BaseUrl, CustomField::MessagesPath];
        if !auth_locked {
            fields.push(CustomField::Auth);
        }
        fields.push(CustomField::ApiKey);
        fields.push(CustomField::DisplayName);
        fields.push(CustomField::Probe);
        fields.extend([Slot::Fable, Slot::Opus, Slot::Sonnet, Slot::Haiku, Slot::Fallback].map(CustomField::Slot));
        fields.push(CustomField::Submit);
        fields
    }
}

/// 校验顺序与桌面端 `saveCustom` 逐条对齐: 厂商名 → base_url 非空 → base_url 前缀 →
/// messages_path 前缀 → gemini 的 `{model}` → API Key → 备注名 → 四个核心槽。
///
/// **`base_url`/`messages_path` 校验的是 `trim` 后的值, 不是原样的草稿字符串**——这是有意
/// 修正桌面端"校验不 trim、提交时才 trim"的不一致 (桌面端校验用原始输入, 真正发请求前才
/// `.trim()`, 于是"Base URL 只有首尾空白"这种输入能通过校验、发请求时却变成空字符串; TUI
/// 这里直接校验 trim 后的值, 校验通过 ⇔ 提交时真正发出去的值也合法), 与后端自己对这些字段的
/// 校验口径一致。不要为了跟桌面端字面一致而改回去。
pub fn validate_custom(d: &CustomDraft, s: &'static Strings) -> Option<(CustomField, &'static str)> {
    if d.provider_display_name.value().trim().is_empty() {
        return Some((CustomField::ProviderName, s.wiz_err_provider_name));
    }
    let base_url = d.base_url.value().trim();
    if base_url.is_empty() {
        return Some((CustomField::BaseUrl, s.wiz_err_base_url_empty));
    }
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Some((CustomField::BaseUrl, s.wiz_err_base_url_scheme));
    }
    let messages_path = d.messages_path.value().trim();
    if !messages_path.starts_with('/') {
        return Some((CustomField::MessagesPath, s.wiz_err_messages_path));
    }
    if d.protocol.requires_model_placeholder() && !messages_path.contains("{model}") {
        return Some((CustomField::MessagesPath, s.wiz_err_gemini_placeholder));
    }
    if d.api_key.is_empty() {
        return Some((CustomField::ApiKey, s.wiz_err_api_key));
    }
    if d.display_name.value().trim().is_empty() {
        return Some((CustomField::DisplayName, s.wiz_err_display_name));
    }
    validate_slots(&d.slots, s).map(|(slot, message)| (CustomField::Slot(slot), message))
}

/// 「获取模型列表」只需要能连上: Base URL (trim 后) 与 API Key 非空, 与桌面端一致, 其余字段这一步
/// 不校验。
pub fn validate_probe(d: &CustomDraft, s: &'static Strings) -> Option<(CustomField, &'static str)> {
    if d.base_url.value().trim().is_empty() {
        return Some((CustomField::BaseUrl, s.wiz_err_base_url_empty));
    }
    if d.api_key.is_empty() {
        return Some((CustomField::ApiKey, s.wiz_err_api_key));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled_draft() -> BasicsDraft {
        BasicsDraft {
            provider_id: "zhipu".into(),
            endpoint_id: "default".into(),
            api_key: SecretField::new("sk-test"),
            display_name: TextField::new("智谱 AI"),
            url_params: vec![],
        }
    }

    #[test]
    fn basics_field_order_inserts_params_after_endpoint() {
        assert_eq!(BasicsField::order(0), BasicsField::ALL.to_vec());
        assert_eq!(
            BasicsField::order(2),
            vec![
                BasicsField::Provider,
                BasicsField::Endpoint,
                BasicsField::UrlParam(0),
                BasicsField::UrlParam(1),
                BasicsField::ApiKey,
                BasicsField::DisplayName,
                BasicsField::Submit
            ]
        );
    }

    #[test]
    fn url_param_fields_are_validated_in_order() {
        let s = &crate::i18n::ZH;
        let mut d = BasicsDraft {
            provider_id: "p".into(),
            endpoint_id: "e".into(),
            api_key: SecretField::new("k"),
            display_name: TextField::new("n"),
            ..Default::default()
        };
        d.url_params = vec![UrlParamDraft {
            id: "account_id".into(),
            label: "Account ID".into(),
            pattern: "^[0-9a-f]{32}$".into(),
            value: TextField::new(""),
        }];
        assert_eq!(validate_basics(&d, s), Some((BasicsField::UrlParam(0), s.wiz_err_url_param_empty)));
        d.url_params[0].value = TextField::new("xyz");
        assert_eq!(validate_basics(&d, s), Some((BasicsField::UrlParam(0), s.wiz_err_url_param_format)));
        d.url_params[0].value = TextField::new("0123456789abcdef0123456789abcdef");
        assert_eq!(validate_basics(&d, s), None);
    }

    #[test]
    fn validate_basics_reports_the_first_bad_field() {
        let s = &crate::i18n::ZH;

        let empty = BasicsDraft::default();
        assert_eq!(validate_basics(&empty, s), Some((BasicsField::Provider, s.wiz_err_provider)), "全空应该先报厂商");

        let missing_key = BasicsDraft { api_key: SecretField::default(), ..filled_draft() };
        assert_eq!(validate_basics(&missing_key, s), Some((BasicsField::ApiKey, s.wiz_err_api_key)), "只缺 key 应该报 ApiKey, 不是别的字段");

        assert_eq!(validate_basics(&filled_draft(), s), None, "都填了应该通过");

        // 备注名全是空白也算空 (trim 之后判断)。
        let blank_name = BasicsDraft { display_name: TextField::new("   "), ..filled_draft() };
        assert_eq!(validate_basics(&blank_name, s), Some((BasicsField::DisplayName, s.wiz_err_display_name)));
    }

    fn store_with_names(names: &[&str]) -> Store {
        let mut store = Store::default();
        let subs = names
            .iter()
            .enumerate()
            .map(|(i, name)| crate::client::dto::Subscription {
                id: format!("s{i}"),
                display_name: (*name).to_string(),
                provider_display_name: "p".into(),
                provider_names: None,
                enabled: true,
                state: crate::client::dto::SubscriptionState::Healthy,
                cooldown_until: None,
                last_error_message: None,
                is_dispatchable: true,
                quota_usage: vec![],
                provider_id: "p".into(),
                base_url: "https://example.invalid".into(),
                auth_type: "api_key".into(),
                endpoint_protocol: "messages".into(),
                model_slots: crate::client::dto::ModelSlots::pending(),
                slot_efforts: Default::default(),
                referenced_by: vec![],
                balance_supported: false,
                balance_cache: None,
                model_cache: None,
            })
            .collect();
        store.apply_subscriptions(1, subs);
        store
    }

    #[test]
    fn default_display_name_appends_a_number_on_collision() {
        let empty = Store::default();
        assert_eq!(default_display_name("智谱 AI", &empty), "智谱 AI", "没有重名时原样返回");

        let one_taken = store_with_names(&["智谱 AI"]);
        assert_eq!(default_display_name("智谱 AI", &one_taken), "智谱 AI 2");

        let two_taken = store_with_names(&["智谱 AI", "智谱 AI 2"]);
        assert_eq!(default_display_name("智谱 AI", &two_taken), "智谱 AI 3");
    }

    fn filled_slots() -> ModelSlots {
        ModelSlots { fable: "glm-4.6".into(), opus: "glm-4.6".into(), sonnet: "glm-4.6".into(), haiku: "glm-4.6".into(), fallback: String::new(), jev: String::new() }
    }

    #[test]
    fn validate_slots_ignores_the_fallback_slot() {
        let s = &crate::i18n::ZH;

        let full = SlotsDraft { slots: filled_slots(), models: vec![] };
        assert_eq!(validate_slots(&full, s), None, "四个核心槽填了、兜底空应该通过");

        let missing_opus = SlotsDraft { slots: ModelSlots { opus: String::new(), ..filled_slots() }, ..full.clone() };
        assert_eq!(validate_slots(&missing_opus, s), Some((Slot::Opus, s.wiz_err_slot)), "少一个核心槽应该报那个槽, 不是别的");

        // 兜底槽本身留空不该被当成校验失败的对象。
        let fallback_only = SlotsDraft { slots: ModelSlots { fallback: String::new(), ..filled_slots() }, ..full.clone() };
        assert_eq!(validate_slots(&fallback_only, s), None, "兜底槽空着不该报错");
    }

    /// 五个协议各一次: `base_url`/`messages_path`/鉴权头都应该等于 `preset()`, 且旧的 `probe`
    /// 应该被清空——`apply_protocol` 是唯一改这三个连接字段的入口, 不管调用前草稿是什么状态。
    #[test]
    fn apply_protocol_prefills_and_clears_the_probe() {
        for protocol in CustomProtocol::ALL {
            let mut d = CustomDraft::new(protocol);
            d.probe = Some(ProbedModels { base_url: "https://old.example.com".into(), models_url: "https://old.example.com/v1/models".into() });

            d.apply_protocol(protocol);

            let preset = protocol.preset();
            assert_eq!(d.base_url.value(), preset.base_url, "{protocol:?}");
            assert_eq!(d.messages_path.value(), preset.messages_path, "{protocol:?}");
            assert_eq!(d.auth_header_name, preset.auth_header_name, "{protocol:?}");
            assert_eq!(d.auth_header_format, preset.auth_header_format, "{protocol:?}");
            assert!(d.probe.is_none(), "换协议应该清空 probe ({protocol:?})");
        }
    }

    /// 一份填满全部字段 (含四个核心槽) 的草稿——`CustomProtocol::Anthropic` 的预设 `base_url`
    /// 是空串, 这里补一个真实值, 其它协议的预设本来就是非空 https 地址, 原样保留。
    fn filled_custom_draft(protocol: CustomProtocol) -> CustomDraft {
        let mut d = CustomDraft::new(protocol);
        d.provider_display_name.set("中转站");
        if d.base_url.value().is_empty() {
            d.base_url.set("https://relay.example.com");
        }
        d.api_key = SecretField::new("sk-test");
        d.display_name.set("中转站");
        d.slots.slots = filled_slots();
        d
    }

    /// 逐条构造只违反其中一条规则的草稿, 断言 `validate_custom` 报的是那一条、不是别的——顺序
    /// 与桌面端 `saveCustom` 一致: 厂商名 → base_url 非空 → base_url 前缀 → messages_path
    /// 前缀 → gemini 的 `{model}` → API Key → 备注名 → 四个核心槽。
    #[test]
    fn validate_custom_follows_the_desktop_order() {
        let s = &crate::i18n::ZH;

        let empty = CustomDraft::new(CustomProtocol::Anthropic);
        assert_eq!(validate_custom(&empty, s), Some((CustomField::ProviderName, s.wiz_err_provider_name)), "全空应该先报厂商名");

        let mut bad_scheme = filled_custom_draft(CustomProtocol::Anthropic);
        bad_scheme.base_url.set("api.example.com");
        assert_eq!(validate_custom(&bad_scheme, s), Some((CustomField::BaseUrl, s.wiz_err_base_url_scheme)), "base_url 缺 scheme 应该报这一条");

        let mut bad_path = filled_custom_draft(CustomProtocol::Anthropic);
        bad_path.messages_path.set("v1/messages");
        assert_eq!(validate_custom(&bad_path, s), Some((CustomField::MessagesPath, s.wiz_err_messages_path)), "请求路径不带前导 / 应该报这一条");

        let mut gemini_missing_placeholder = filled_custom_draft(CustomProtocol::Gemini);
        gemini_missing_placeholder.messages_path.set("/v1beta/models/generateContent");
        assert_eq!(
            validate_custom(&gemini_missing_placeholder, s),
            Some((CustomField::MessagesPath, s.wiz_err_gemini_placeholder)),
            "Gemini 的请求路径缺 {{model}} 应该单独报这一条"
        );

        // 变异验证目标: 这一条如果被误改成对 GeminiInteractions 也要求占位符, 这里就会失败——
        // `requires_model_placeholder()` 只对 `Gemini` 返回真。
        let mut gemini_interactions_same_path = filled_custom_draft(CustomProtocol::GeminiInteractions);
        gemini_interactions_same_path.messages_path.set("/v1beta/models/generateContent");
        assert_eq!(validate_custom(&gemini_interactions_same_path, s), None, "GeminiInteractions 不要求占位符, 同样的路径应该通过");

        let mut missing_key = filled_custom_draft(CustomProtocol::Anthropic);
        missing_key.api_key = SecretField::default();
        assert_eq!(validate_custom(&missing_key, s), Some((CustomField::ApiKey, s.wiz_err_api_key)));

        let mut missing_name = filled_custom_draft(CustomProtocol::Anthropic);
        missing_name.display_name.set("   ");
        assert_eq!(validate_custom(&missing_name, s), Some((CustomField::DisplayName, s.wiz_err_display_name)));

        let mut missing_slot = filled_custom_draft(CustomProtocol::Anthropic);
        missing_slot.slots.slots.opus = String::new();
        assert_eq!(validate_custom(&missing_slot, s), Some((CustomField::Slot(Slot::Opus), s.wiz_err_slot)), "四个核心槽任一空都应该报到那个槽");

        assert_eq!(validate_custom(&filled_custom_draft(CustomProtocol::Anthropic), s), None, "全填好应该通过");
    }

    /// 只校验能不能连上: Base URL 只有空白也算空; 其余字段 (厂商名 / 槽位……) 这一步不管。
    #[test]
    fn validate_probe_only_checks_base_url_and_api_key() {
        let s = &crate::i18n::ZH;

        let mut d = CustomDraft::new(CustomProtocol::Anthropic);
        d.base_url.set("   ");
        d.api_key = SecretField::new("sk-test");
        assert_eq!(validate_probe(&d, s), Some((CustomField::BaseUrl, s.wiz_err_base_url_empty)), "只有空白的 Base URL 应该算空");

        d.base_url.set("https://relay.example.com");
        d.api_key = SecretField::default();
        assert_eq!(validate_probe(&d, s), Some((CustomField::ApiKey, s.wiz_err_api_key)));

        d.api_key = SecretField::new("sk-test");
        assert_eq!(validate_probe(&d, s), None, "厂商名 / 备注名 / 槽位都空着也可以探测");
    }

    /// 探测后原样 → `Some`; 改一个字符 → `None`; 改回去 → 又是 `Some`——与桌面端
    /// `customProbe.baseUrl === baseUrl` 同规则。
    #[test]
    fn models_url_is_only_sent_back_when_the_base_url_is_unchanged() {
        let mut d = CustomDraft::new(CustomProtocol::Anthropic);
        d.base_url.set("https://relay.example.com");
        d.probe =
            Some(ProbedModels { base_url: "https://relay.example.com".into(), models_url: "https://relay.example.com/v1/models".into() });
        assert_eq!(d.models_url(), Some("https://relay.example.com/v1/models"), "探测后原样应该回传");

        d.base_url.set("https://relay.example.com/changed");
        assert_eq!(d.models_url(), None, "改了一个字符应该不再回传");

        d.base_url.set("https://relay.example.com");
        assert_eq!(d.models_url(), Some("https://relay.example.com/v1/models"), "改回去应该又生效");
    }
}

#[cfg(test)]
mod url_param_tests {
    use super::*;
    use crate::client::dto::{ModelDiscovery, Provider, ProviderAuth, ProviderEndpoint, ProviderText, ProviderTranslations, UrlParam};

    fn cf_provider() -> Provider {
        let ep = |id: &str, used: &[&str]| ProviderEndpoint {
            id: id.into(),
            label: id.into(),
            base_url: "https://api.example.com".into(),
            protocol: "messages".into(),
            example_models: vec![],
            url_params_used: used.iter().map(|s| s.to_string()).collect(),
        };
        let text = || ProviderText { display_name: "cf".into(), description: None, endpoints: Default::default(), url_params: Default::default() };
        Provider {
            id: "cloudflare".into(),
            display_name: "cf".into(),
            description: None,
            endpoints: vec![ep("direct", &["account_id"]), ep("gateway", &["account_id", "gateway_id"])],
            default_endpoint: Some("direct".into()),
            auth: ProviderAuth { auth_type: "openai_chat_completions_api_key".into() },
            model_discovery: ModelDiscovery { enabled: true, example_models: vec![] },
            translations: ProviderTranslations { en: text(), ja: text() },
            url_params: vec![
                UrlParam { id: "account_id".into(), label: "Account ID".into(), placeholder: None, pattern: "^[0-9a-f]{32}$".into() },
                UrlParam { id: "gateway_id".into(), label: "Gateway ID".into(), placeholder: None, pattern: "^[A-Za-z0-9_-]{1,64}$".into() },
            ],
        }
    }

    #[test]
    fn sync_url_params_keeps_typed_values() {
        let p = cf_provider();
        let mut rows = sync_url_params(&[], &p, "direct");
        assert_eq!(rows.len(), 1);
        rows[0].value = TextField::new("abc");
        let rows = sync_url_params(&rows, &p, "gateway");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "account_id");
        assert_eq!(rows[0].value.value(), "abc");
        assert_eq!(rows[1].id, "gateway_id");
        assert_eq!(rows[1].value.value(), "");
        assert!(sync_url_params(&rows, &p, "nope").is_empty());
    }
}
