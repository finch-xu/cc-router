//! 内置厂商路径的第一步: 选厂商 / 选接入点 / 填 API Key / 备注名 → 下一步。

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

use super::common::{self, Cell, FieldKind, FormFields, KeyOutcome, RowHints, Rows};
use super::fields::{sync_url_params, validate_basics, BasicsDraft, BasicsField};
use super::form_state::FormState;
use super::text::TextInput;
use super::{follow_display_name, BasicsPhase, Paint};
use crate::action::{Action, WizardCmd};
use crate::client::dto::{capability_suffix, CreateInput, CreateSource, CustomProtocol, ModelSlots, Provider};
use crate::i18n::Strings;
use crate::store::Store;
use crate::widgets::form::{self, FormView};
use crate::widgets::keybar::Hint;
use crate::widgets::picker::{PickerChoice, PickerItem, PickerSpec, PickerTag};
use crate::widgets::toast::ToastKind;

pub(super) struct BasicsForm {
    pub(super) draft: BasicsDraft,
    pub(super) state: FormState<BasicsField>,
    /// 上一次自动算出来的备注名, 见 `follow_display_name`。
    pub(super) last_auto_name: Option<String>,
    /// 上一次创建失败的原因, 挂在表单顶部。
    pub(super) note: Option<String>,
    /// 提交刚刚被拒绝, `draw` 该在算出那一行的 `Rect` 之后播一次 `fx::field_err` 并取走它
    /// (`Wizard::draw` 直接 `mem::take` 这个字段, 见该函数的调用点)。
    pub(super) pending_field_err: bool,
}

impl Default for BasicsForm {
    fn default() -> Self {
        Self {
            draft: BasicsDraft::default(),
            state: FormState::new(BasicsField::Provider),
            last_auto_name: None,
            note: None,
            pending_field_err: false,
        }
    }
}

impl FormFields for BasicsForm {
    type Field = BasicsField;

    fn state(&self) -> &FormState<BasicsField> {
        &self.state
    }

    fn state_mut(&mut self) -> &mut FormState<BasicsField> {
        &mut self.state
    }

    fn order(&self) -> Vec<BasicsField> {
        BasicsField::order(self.draft.url_params.len())
    }

    fn kind(&self, field: BasicsField, s: &'static Strings) -> FieldKind {
        match field {
            BasicsField::Provider | BasicsField::Endpoint => FieldKind::Pick,
            BasicsField::ApiKey => FieldKind::Secret,
            BasicsField::DisplayName | BasicsField::UrlParam(_) => FieldKind::Text,
            BasicsField::Submit => FieldKind::Button(s.wiz_btn_next),
        }
    }

    fn text_field(&mut self, field: BasicsField) -> Option<&mut dyn TextInput> {
        self.draft.text_field(field)
    }

    fn cursor_for(&self, field: BasicsField) -> Option<usize> {
        match field {
            BasicsField::ApiKey => Some(self.draft.api_key.visual_cursor()),
            BasicsField::DisplayName => Some(self.draft.display_name.visual_cursor()),
            BasicsField::UrlParam(i) => self.draft.url_params.get(i).map(|p| p.value.visual_cursor()),
            BasicsField::Provider | BasicsField::Endpoint | BasicsField::Submit => None,
        }
    }
}

impl BasicsForm {
    /// 只在 `BasicsPhase::Editing` 下被调用。提交成功时把 `phase` 推进到 `Creating`。
    pub(super) fn handle_key(&mut self, key: KeyEvent, phase: &mut BasicsPhase, providers: &[Provider], s: &'static Strings) -> Option<Action> {
        match common::handle_key(self, key, s) {
            KeyOutcome::Handled | KeyOutcome::Edited(_) => None,
            KeyOutcome::Activate(BasicsField::Provider) => Some(self.provider_picker(providers, s)),
            KeyOutcome::Activate(BasicsField::Endpoint) => Some(self.endpoint_picker(providers, s)),
            KeyOutcome::Activate(BasicsField::Submit) => self.submit(phase, providers, s),
            KeyOutcome::Activate(BasicsField::ApiKey | BasicsField::DisplayName | BasicsField::UrlParam(_)) => None,
        }
    }

    /// `Submit` 行 `⏎`: 校验通过则打包 `WizardCmd::Create` (槽位先放 `ModelSlots::pending()`,
    /// 第二步再绑; System One 端点没有四个核心槽, 放全空槽——Jev 可留空) 并进 `Creating`。
    fn submit(&mut self, phase: &mut BasicsPhase, providers: &[Provider], s: &'static Strings) -> Option<Action> {
        if !self.state.validate(validate_basics(&self.draft, s)) {
            self.pending_field_err = true;
            return None;
        }
        self.note = None;
        let cmd = WizardCmd::Create(CreateInput {
            // 校验按 trim 后判空, 发出去的也是 trim 后的值——校验什么就发送什么。
            display_name: self.draft.display_name.value().trim().to_string(),
            api_key: self.draft.api_key.secret(),
            model_slots: if self.selected_endpoint_is_systemone(providers) { ModelSlots::default() } else { ModelSlots::pending() },
            source: CreateSource::Builtin {
                provider_id: self.draft.provider_id.clone(),
                endpoint_id: self.draft.endpoint_id.clone(),
                url_params: self.draft.url_params.iter().map(|p| (p.id.clone(), p.value.value().trim().to_string())).collect(),
            },
        });
        *phase = BasicsPhase::Creating;
        Some(Action::WizardRequest(Box::new(cmd)))
    }

    /// 草稿里选中的接入点是不是 System One 协议。
    pub(super) fn selected_endpoint_is_systemone(&self, providers: &[Provider]) -> bool {
        providers
            .iter()
            .find(|p| p.id == self.draft.provider_id)
            .and_then(|p| p.endpoints.iter().find(|e| e.id == self.draft.endpoint_id))
            .is_some_and(|e| e.is_systemone())
    }

    /// `Provider` 行 `⏎`: 条目是全部厂商 + 5 个自定义协议。OAuth 类厂商 (TUI 不做设备码流程)
    /// 的 label 后面追加提示语, picker 本身没有「置灰不可选」的能力, 选中时用「可选但选了只给
    /// 提示」等效 (见 `Wizard::apply_provider_choice`)。
    fn provider_picker(&self, providers: &[Provider], s: &'static Strings) -> Action {
        let mut items: Vec<PickerItem> = providers
            .iter()
            .map(|p| {
                let (llm, jev) = p.capabilities();
                let base = if p.is_oauth() { format!("{} · {}", p.display_name, s.wiz_desktop_only) } else { p.display_name.clone() };
                PickerItem { id: p.id.clone(), label: format!("{base}{}", capability_suffix(llm, jev)), hint: p.description.clone() }
            })
            .collect();
        for (protocol, label) in CustomProtocol::ALL.iter().zip(s.wiz_custom_labels.iter()) {
            items.push(PickerItem { id: format!("custom:{}", protocol.as_wire()), label: format!("{label}{}", capability_suffix(true, false)), hint: None });
        }
        Action::OpenPicker(PickerSpec {
            tag: PickerTag::WizardProvider,
            title: s.wiz_pick_provider.to_string(),
            items,
            allow_custom: false,
            initial: self.draft.provider_id.clone(),
        })
    }

    /// `Endpoint` 行 `⏎`: 厂商还没选时就地拒绝, 不开弹窗。
    fn endpoint_picker(&self, providers: &[Provider], s: &'static Strings) -> Action {
        let Some(provider) = providers.iter().find(|p| p.id == self.draft.provider_id) else {
            return Action::Notify { kind: ToastKind::Info, text: s.wiz_pick_provider_first.to_string() };
        };
        let items =
            provider.endpoints.iter().map(|e| PickerItem { id: e.id.clone(), label: e.label.clone(), hint: Some(e.base_url.clone()) }).collect();
        Action::OpenPicker(PickerSpec {
            tag: PickerTag::WizardEndpoint,
            title: s.wiz_pick_endpoint.to_string(),
            items,
            allow_custom: false,
            initial: self.draft.endpoint_id.clone(),
        })
    }

    /// 选中一个可用的内置厂商: 记 `provider_id`, 接入点置为默认值, 备注名按需跟随, 焦点移到
    /// `ApiKey`。**重选同一个厂商什么都不重算**——否则用户手动换过的接入点会在无意中确认同一项
    /// 时被悄悄弹回默认值。无论重选与否, 厂商行自己的错误都清掉 (这个字段已经合法了)。
    pub(super) fn choose_provider(&mut self, provider: &Provider, store: &Store) {
        self.state.clear(BasicsField::Provider);
        if provider.id == self.draft.provider_id {
            return;
        }
        self.draft.provider_id = provider.id.clone();
        self.draft.endpoint_id = provider.default_endpoint().map(|e| e.id.clone()).unwrap_or_default();
        self.draft.url_params = sync_url_params(&[], provider, &self.draft.endpoint_id);
        self.state.clear(BasicsField::Endpoint);
        if follow_display_name(&mut self.draft.display_name, &mut self.last_auto_name, &provider.display_name, store) {
            self.state.clear(BasicsField::DisplayName);
        }
        self.state.focus_on(BasicsField::ApiKey);
    }

    pub(super) fn apply_endpoint_choice(&mut self, choice: &PickerChoice, providers: &[Provider]) {
        if let PickerChoice::Item(id) = choice {
            self.draft.endpoint_id = id.clone();
            if let Some(p) = providers.iter().find(|p| p.id == self.draft.provider_id) {
                self.draft.url_params = sync_url_params(&self.draft.url_params, p, &self.draft.endpoint_id);
            }
            self.state.clear(BasicsField::Endpoint);
        }
    }

    /// 「厂商已选」或「API Key 非空」或「备注名非空」任一成立——填了一半按 `Esc` 不该直接丢掉。
    pub(super) fn has_input(&self) -> bool {
        !self.draft.provider_id.is_empty() || !self.draft.api_key.is_empty() || !self.draft.display_name.value().trim().is_empty()
    }

    pub(super) fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>> {
        common::hints(self, s)
    }

    /// 三个阶段的行结构完全一样; 请求在飞时 (`Creating` / `LoadingModels`) 全部字段画成只读、
    /// 按钮换成对应的进行中文案。**返回聚焦行的下标 + 矩形**, 供 `Wizard::draw` 在校验失败时播
    /// `fx::field_err`——几何只有这里知道, `draw` 本身仍然是纯函数 (不改业务状态)。
    pub(super) fn draw(&self, frame: &mut Frame, area: Rect, phase: &BasicsPhase, providers: &[Provider], p: &Paint) -> Option<(usize, Rect)> {
        let s = p.s;
        let busy_label = match phase {
            BasicsPhase::Editing => None,
            BasicsPhase::Creating => Some(s.wiz_creating),
            BasicsPhase::LoadingModels { .. } => Some(s.wiz_loading_models),
        };
        let provider = providers.iter().find(|p| p.id == self.draft.provider_id);
        let provider_label = provider.map(|p| p.display_name.clone()).unwrap_or_default();
        let endpoint_label = provider
            .and_then(|p| p.endpoints.iter().find(|e| e.id == self.draft.endpoint_id))
            .map(|e| e.label.clone())
            .unwrap_or_default();
        let api_key_text = self.draft.api_key.display();
        let hints = RowHints::new(s);

        let mut rows = Rows::new(self, &hints, s, busy_label.is_some());
        rows.note(self.note.as_deref());
        rows.field(BasicsField::Provider, Cell { label: s.wiz_f_provider, value: &provider_label, ..Cell::default() });
        rows.field(BasicsField::Endpoint, Cell { label: s.wiz_f_endpoint, value: &endpoint_label, ..Cell::default() });
        for (i, p) in self.draft.url_params.iter().enumerate() {
            rows.field(BasicsField::UrlParam(i), Cell { label: &p.label, value: p.value.value(), ..Cell::default() });
        }
        rows.field(BasicsField::ApiKey, Cell { label: s.wiz_f_api_key, value: &api_key_text, ..Cell::default() });
        rows.field(
            BasicsField::DisplayName,
            Cell { label: s.wiz_f_display_name, value: self.draft.display_name.value(), ..Cell::default() },
        );
        rows.spacer();
        rows.button(BasicsField::Submit, busy_label);
        let built = rows.finish();
        let focus_index = built.focus;

        let view = FormView {
            title: s.wiz_title,
            steps: Some((0, s.wiz_steps.as_slice())),
            rows: &built.rows,
            focus: built.focus,
            tick: p.tick,
            show_cursor: p.show_cursor,
        };
        form::draw(frame, area, &view, p.theme, s).map(|rect| (focus_index, rect))
    }
}
