//! 新建订阅向导。**不是标签页也不是弹窗**, 而是夹在弹窗与全局键之间的一层: 存在时内容区整个
//! 归它, 除 `Ctrl+C` 外所有按键归它 (所以 `q` / `r` / `1`-`5` 能当普通字符输入)。它之上仍然可以
//! 叠**一个**弹窗 (选厂商 / 选模型 / 退出确认), 所以不需要弹窗栈。
//!
//! 它刻意不实现 `Component` (`crate::pages::Component`): 那个 trait 的一半方法
//! (`on_subscriptions_changed` / `on_mutation_*` / `on_event`) 对向导没有意义, 而向导需要的
//! `on_open` 又不在里面。方法签名照着 `Component` 写, 只有一处不同: 产出的是 `WizardCmd` 而不是
//! `Cmd`——向导不知道自己的代次, 由 `App` 统一盖上再发出去 (见 `action::WizardResult`)。
//!
//! 两条路径: 内置厂商是两步 (`basics` 建订阅 → `slots` 绑模型), 自定义厂商是单页 (`custom`,
//! 探测模型不落库, 创建时槽位已经是真值)。每条路径的数据都挂在自己的 [`Stage`] 变体上, 阶段之外
//! 没有「只在某些阶段有意义」的字段。
//!
//! **表单交互的总规则** (三张表单共用):
//! - 表单是一列「行」, `↑` / `↓` 在**可聚焦**的行之间移动 (说明行与空行跳过), 不绕回;
//!   `Tab` 与 `↓` 同义、`BackTab` (Shift+Tab) 与 `↑` 同义, 在所有行类型上都有效。
//! - **文本行**: 直接打字 (不用先进入编辑模式); `⏎` = 移到下一个可聚焦行。
//! - **选择行**: `⏎` = 打开选择弹窗; 不能直接打字。
//! - **按钮行**: `⏎` = 执行。所以「下一步」「保存」「获取模型列表」**都不占用任何字符键**——
//!   这是表单吞掉全部按键之后唯一安全的做法。
//! - `Esc` = 退出向导 (`has_input()` 为真时先弹确认); 请求在飞时整张表单只读, 但 `Esc` 能不能用
//!   要看这个请求**会不会落库**: 创建 / 保存在飞时连 `Esc` 也吞 (撤不回后端落库); 只读的
//!   拉厂商 / 拉模型 / 探测在飞时 `Esc` 可用。
//! - **每个异步结果只在发起它的那个阶段被接受, 其余一律丢弃**: 向导同一时刻最多一个请求在飞,
//!   同一实例内按阶段判就够, 不需要 `Fetch` 那套 `issued` 序号 (见 `apply_wizard_result`);
//!   别的实例的晚到结果在到达这里之前已经被 `App` 按代次丢掉了。

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, BRAILLE_SIX};

use crate::action::{Action, OnYes, WizardCmd, WizardResult};
use crate::client::dto::{CustomProtocol, ModelSlots, ProbeModelsResult, Provider, RefreshModelsResult, Slot};
use crate::fx::Dir;
use crate::i18n::Strings;
use crate::pages::DrawCtx;
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::keybar::Hint;
use crate::widgets::picker::{PickerChoice, PickerTag};
use crate::widgets::spinner_state;
use crate::widgets::toast::ToastKind;

mod basics;
mod common;
mod custom;
mod fields;
mod form_state;
mod slots;
mod text;
use basics::BasicsForm;
use custom::CustomForm;
use fields::{default_display_name, CustomField, ProbedModels, SlotsDraft};
use slots::SlotsForm;
use text::TextField;

/// 向导走到哪一步了, 以及这一步的全部数据。
enum Stage {
    /// 正在拉厂商列表。
    Loading,
    /// 拉失败了, 表单画不出来, 只能 `Esc` 退出。
    LoadFailed(String),
    /// 内置路径第一步。
    Basics { form: BasicsForm, phase: BasicsPhase },
    /// 内置路径第二步: 订阅 `id` 已经建好, 给槽位选模型。`name` 是建订阅时的备注名 (保存成功的
    /// toast 用)。`saving` = `SaveSlots` 在飞, 表单只读, 连 `Esc` 也吞 (这个请求会落库)。
    Slots { id: String, name: String, form: SlotsForm, saving: bool },
    /// 自定义路径单页。表单比其它阶段大得多, 装箱免得每个 `Stage` 都按它分配。
    Custom { form: Box<CustomForm>, phase: CustomPhase },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BasicsPhase {
    Editing,
    /// `create_subscription` 在飞: 表单只读, **连 `Esc` 也吞**——这个请求会落库, 退出撤不回。
    Creating,
    /// 订阅 `id` 已经建好, 在等 `refresh_model_list`: 表单同样只读, 但这是只读请求, `Esc` 可用,
    /// 确认文案与在 `Slots` 退出相同 (订阅已经存在了)。
    LoadingModels { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomPhase {
    Editing,
    /// `probe_custom_models` 在飞: 只读请求, `Esc` 可用; 还什么都没落库, 确认文案是「放弃」。
    Probing,
    /// `create_subscription` 在飞: 连 `Esc` 也吞。
    Creating,
}

/// 各张表单 `draw` 需要的、不属于表单自己的东西。
struct Paint<'a> {
    theme: &'a Theme,
    s: &'static Strings,
    tick: u64,
    /// 有弹窗叠在向导上面时不设终端光标, 见 `Wizard::draw`。
    show_cursor: bool,
}

pub struct Wizard {
    stage: Stage,
    /// `list_providers` 拉到的厂商列表。
    providers: Vec<Provider>,
    /// 与页面的 `pending_notice` 同一套约定, 见 `take_notice`。
    notice: Option<(ToastKind, String)>,
    /// `take_close_request()` 的待办标记。
    close_request: bool,
    /// 换步动效待播的方向; `draw` 取走并调 `fx::wizard_step`。只有 `Basics → Slots` 这一次转场
    /// 会设它 (见 `apply_wizard_result` 的 `Models` 分支)——`Loading → Basics` 与自定义单页的阶段
    /// 切换都不算「换步」。
    pending_step_fx: Option<Dir>,
}

// clippy::new_without_default: `Wizard::new()` 是这个 crate 第一个零参数的 `new()`, 照 clippy 的
// 建议直接转发。
impl Default for Wizard {
    fn default() -> Self {
        Self::new()
    }
}

impl Wizard {
    pub fn new() -> Self {
        Self { stage: Stage::Loading, providers: Vec::new(), notice: None, close_request: false, pending_step_fx: None }
    }

    /// 刚打开: 要发的请求 (拉厂商列表)。`App` 在创建它之后立刻调一次。
    pub fn on_open(&mut self) -> Vec<WizardCmd> {
        vec![WizardCmd::LoadProviders]
    }

    /// 除 `Ctrl+C` 外的全部按键。`None` = 吞掉 (或只改了向导自己的状态)。`Esc` 跟阶段无关,
    /// 排在最前面统一判断; 其余按键只有编辑中的表单才会处理。**两种确认文案**: 订阅还没建时是
    /// `confirm_discard`; 订阅已经建好 (`Slots`, 以及等模型列表的 `LoadingModels`) 时是
    /// `wiz_confirm_exit_pending` (退出会留下带 (pending) 槽位的订阅, 不是「放弃编辑」)。
    pub fn handle_key(&mut self, key: KeyEvent, store: &Store, s: &'static Strings) -> Option<Action> {
        if key.code == KeyCode::Esc && self.can_cancel() {
            if !self.has_input() {
                return Some(Action::CloseWizard);
            }
            let created = matches!(self.stage, Stage::Slots { .. } | Stage::Basics { phase: BasicsPhase::LoadingModels { .. }, .. });
            let prompt = if created { s.wiz_confirm_exit_pending } else { s.confirm_discard };
            return Some(Action::OpenConfirm { prompt: prompt.to_string(), on_yes: OnYes::discard_then(Action::CloseWizard) });
        }
        match &mut self.stage {
            Stage::Basics { form, phase } if *phase == BasicsPhase::Editing => form.handle_key(key, phase, &self.providers, s),
            Stage::Slots { id, form, saving, .. } if !*saving => form.handle_key(key, id, saving, s),
            Stage::Custom { form, phase } if *phase == CustomPhase::Editing => form.handle_key(key, phase, store, s),
            Stage::Loading | Stage::LoadFailed(_) | Stage::Basics { .. } | Stage::Slots { .. } | Stage::Custom { .. } => None,
        }
    }

    /// 消费 `Action::WizardDone` (异步结果) 与 `Action::PickerDone` (选择弹窗的结果)。按键触发的
    /// 请求走 `Action::WizardRequest`, 不经过这里。`Created(Ok)` 会紧接着产出一次 `LoadModels`。
    pub fn update(&mut self, action: &Action, store: &Store, s: &'static Strings) -> Vec<WizardCmd> {
        match action {
            Action::WizardDone { result, .. } => self.apply_wizard_result(result, s),
            Action::PickerDone { tag, choice } => {
                self.apply_picker_choice(tag, choice, store, s);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// 按 `tag` 分派给打开这个弹窗的那张表单; 当前阶段不是那张表单就忽略。**穷尽 `match`**:
    /// 新增 `PickerTag` 变体时这里会编译失败, 逼着显式决定向导要不要关心它。
    fn apply_picker_choice(&mut self, tag: &PickerTag, choice: &PickerChoice, store: &Store, s: &'static Strings) {
        match (tag, &mut self.stage) {
            (PickerTag::WizardProvider, _) => self.apply_provider_choice(choice, store, s),
            (PickerTag::WizardEndpoint, Stage::Basics { form, .. }) => form.apply_endpoint_choice(choice, &self.providers),
            (PickerTag::WizardSlot { slot }, Stage::Slots { form, .. }) => form.apply_slot_choice(*slot, choice),
            (PickerTag::WizardSlot { slot }, Stage::Custom { form, .. }) => form.apply_slot_choice(*slot, choice),
            (PickerTag::WizardProtocol, Stage::Custom { form, .. }) => form.apply_protocol_choice(choice),
            (PickerTag::WizardAuth, Stage::Custom { form, .. }) => form.apply_auth_choice(choice),
            (PickerTag::WizardEndpoint | PickerTag::WizardSlot { .. } | PickerTag::WizardProtocol | PickerTag::WizardAuth, _) => {}
            (
                PickerTag::SlotModel { .. }
                | PickerTag::SlotEffort { .. }
                | PickerTag::VmAddSubscription { .. }
                | PickerTag::LiveFilter
                | PickerTag::LogsFilter,
                _,
            ) => {}
        }
    }

    /// 选中内置厂商 → 交给 `BasicsForm::choose_provider`; 选中自定义条目 → 换成一张全新的自定义
    /// 表单; 选中 OAuth 厂商 → 不设值, 只提示去桌面端添加。
    fn apply_provider_choice(&mut self, choice: &PickerChoice, store: &Store, s: &'static Strings) {
        let Stage::Basics { form, .. } = &mut self.stage else { return };
        // `allow_custom: false`: picker 不会产出 `Custom`。
        let PickerChoice::Item(id) = choice else { return };
        if let Some(wire) = id.strip_prefix("custom:") {
            let Some(protocol) = CustomProtocol::ALL.iter().find(|p| p.as_wire() == wire).copied() else { return };
            self.stage = Stage::Custom { form: Box::new(CustomForm::new(protocol)), phase: CustomPhase::Editing };
            return;
        }
        let Some(provider) = self.providers.iter().find(|p| &p.id == id) else { return };
        if provider.is_oauth() {
            self.notice = Some((ToastKind::Info, s.wiz_desktop_only.to_string()));
            return;
        }
        form.choose_provider(provider, store);
    }

    /// **刻意写成穷尽 `match`, 不用 `_` 兜底**: `WizardResult` 每加一个新变体, 这里就必须显式
    /// 接一条臂。
    ///
    /// **每个结果只在发起它的那个阶段被接受, 其余一律丢弃**: 按 `n` 打开向导 → 厂商列表还没回来
    /// 就 `Esc` → 再按 `n` 重开; 旧的结果晚到时如果不看阶段, 会把已经往前走的表单打回去, 或者
    /// 在错误的阶段关掉向导。
    ///
    /// **`Models` / `Probed` 还要核对结果自带的身份**, 作为代次之外的纵深防御: 这两个是只读请求,
    /// 在飞时 `Esc` 可用、关闭向导也不会取消请求, 别的向导实例的结果本该已经被 `App` 按代次丢掉;
    /// 万一漏过来, 单靠阶段拦不住。等待期间表单只读, 阶段里记下的 id / 草稿里的 Base URL 就是这次
    /// 请求发出时的值, 不一致必然不是这次请求的结果。漏过来的后果: 另一家厂商的模型名被预填进槽位,
    /// 或者把中转 X 的 `models_url` 与中转 Y 的 Base URL 一起落库。
    fn apply_wizard_result(&mut self, result: &WizardResult, s: &'static Strings) -> Vec<WizardCmd> {
        match result {
            WizardResult::Providers(inner) => {
                if !matches!(self.stage, Stage::Loading) {
                    return Vec::new();
                }
                self.stage = match inner {
                    Ok(list) => {
                        self.providers = list.clone();
                        Stage::Basics { form: BasicsForm::default(), phase: BasicsPhase::Editing }
                    }
                    Err(reason) => Stage::LoadFailed(reason.clone()),
                };
                Vec::new()
            }
            WizardResult::Created(inner) => match &mut self.stage {
                Stage::Basics { form, phase } if *phase == BasicsPhase::Creating => match inner {
                    Ok(created) => {
                        let systemone_examples = self
                            .providers
                            .iter()
                            .find(|p| p.id == form.draft.provider_id)
                            .and_then(|p| p.endpoints.iter().find(|e| e.id == form.draft.endpoint_id))
                            .filter(|e| e.is_systemone())
                            .map(|e| e.example_models.clone());
                        if let Some(examples) = systemone_examples {
                            // System One: 三家上游都没有可用的标准模型列表 (实测), 不拉模型, 直接进 Jev 槽。
                            let name = form.draft.display_name.value().trim().to_string();
                            self.stage =
                                Stage::Slots { id: created.id.clone(), name, form: SlotsForm::new_systemone(examples), saving: false };
                            self.pending_step_fx = Some(Dir::Forward);
                            return Vec::new();
                        }
                        *phase = BasicsPhase::LoadingModels { id: created.id.clone() };
                        vec![WizardCmd::LoadModels { id: created.id.clone() }]
                    }
                    Err(e) => {
                        *phase = BasicsPhase::Editing;
                        form.note = Some((s.wiz_create_failed)(e));
                        Vec::new()
                    }
                },
                // 自定义路径的槽位此刻已经是真值: 创建成功直接关向导, 不再拉模型 / 保存槽位。
                Stage::Custom { form, phase } if *phase == CustomPhase::Creating => {
                    match inner {
                        Ok(_created) => {
                            self.notice = Some((ToastKind::Success, (s.wiz_created)(form.draft.display_name.value().trim())));
                            self.close_request = true;
                        }
                        Err(e) => {
                            *phase = CustomPhase::Editing;
                            form.note = Some((s.wiz_create_failed)(e));
                        }
                    }
                    Vec::new()
                }
                Stage::Loading | Stage::LoadFailed(_) | Stage::Basics { .. } | Stage::Slots { .. } | Stage::Custom { .. } => Vec::new(),
            },
            WizardResult::Models { id, result: inner } => {
                let Stage::Basics { form, phase: BasicsPhase::LoadingModels { id: expected } } = &self.stage else { return Vec::new() };
                if expected != id {
                    return Vec::new();
                }
                let (draft, note) = match inner {
                    Ok(RefreshModelsResult::Auto { models, .. }) => {
                        let mut slots = ModelSlots::default();
                        // 桌面端同规则: 有候选就预填四个核心槽为第一项; 空候选就留空, 交给
                        // `validate_slots` 在保存时拦住。
                        if let Some(first) = models.first() {
                            for slot in [Slot::Fable, Slot::Opus, Slot::Sonnet, Slot::Haiku] {
                                slots.set(slot, first.id.clone());
                            }
                        }
                        (SlotsDraft { slots, models: models.clone() }, None)
                    }
                    Ok(RefreshModelsResult::ManualFallback { reason }) | Err(reason) => {
                        (SlotsDraft::default(), Some((s.wiz_models_manual)(reason)))
                    }
                };
                let examples = self
                    .providers
                    .iter()
                    .find(|p| p.id == form.draft.provider_id)
                    .map(|p| p.model_discovery.example_models.clone())
                    .unwrap_or_default();
                // 与发给后端的备注名同值 (`to_create_input` 里 trim 过), 提示里不带首尾空格。
                let name = form.draft.display_name.value().trim().to_string();
                self.stage = Stage::Slots { id: id.clone(), name, form: SlotsForm::new(draft, examples, note), saving: false };
                // 内置路径唯一的「换步」转场: 第一步的表单换成第二步。自定义单页没有第二步,
                // `Loading → Basics` 是加载而不是换步, 都不该播这个动效。
                self.pending_step_fx = Some(Dir::Forward);
                Vec::new()
            }
            WizardResult::Probed { base_url, result: inner } => {
                let Stage::Custom { form, phase } = &mut self.stage else { return Vec::new() };
                if *phase != CustomPhase::Probing || form.draft.base_url.value().trim() != base_url.as_str() {
                    return Vec::new();
                }
                let draft = &mut form.draft;
                match inner {
                    Ok(ProbeModelsResult::Auto { models, models_url }) => {
                        draft.slots.models = models.clone();
                        // 不自动预填槽位 (与桌面端一致): 自定义中转的模型名千差万别, 猜错不如留空。
                        draft.probe = Some(ProbedModels { base_url: base_url.clone(), models_url: models_url.clone() });
                        form.note = None;
                    }
                    Ok(ProbeModelsResult::ManualFallback { reason }) | Err(reason) => {
                        draft.slots.models = Vec::new();
                        draft.probe = None;
                        form.note = Some((s.wiz_models_manual)(reason));
                    }
                }
                form.state.focus_on(CustomField::Slot(Slot::Fable));
                *phase = CustomPhase::Editing;
                Vec::new()
            }
            WizardResult::SlotsSaved(inner) => {
                let Stage::Slots { name, form, saving, .. } = &mut self.stage else { return Vec::new() };
                if !*saving {
                    return Vec::new();
                }
                match inner {
                    Ok(()) => {
                        self.notice = Some((ToastKind::Success, (s.wiz_created)(name)));
                        self.close_request = true;
                    }
                    Err(e) => {
                        *saving = false;
                        form.note = Some((s.wiz_save_failed)(e));
                    }
                }
                Vec::new()
            }
        }
    }

    /// `popup_open`: 是否有弹窗叠在向导上面。有弹窗时向导不该再设终端光标——
    /// `Frame::set_cursor_position` 一帧只记一个坐标、最后一次生效; 向导先画、弹窗后画, 弹窗
    /// (Confirm / Help / Detail) 自己不设光标时, 向导设的坐标会留到帧尾, 光标停在被压暗的输入框里
    /// 闪烁。Picker 自己会设光标, 天然覆盖。
    pub fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx, popup_open: bool) {
        let paint = Paint { theme: ctx.theme, s: ctx.s, tick: ctx.tick, show_cursor: !popup_open };
        let s = ctx.s;
        // 两个动效调用点都在这里 (几何只有 `draw` 知道): 校验失败时每张表单自己的 `draw` 已经
        // 返回了聚焦行的 (下标, 矩形), 这里只需要取走 `pending_field_err` 决定要不要播;
        // `mem::take` 让下一帧不会重复播 (与 `pages::subscriptions` 的 `flash_rows` 同一套写法)。
        match &mut self.stage {
            Stage::Loading => {
                let state = spinner_state(ctx.tick);
                // `to_symbol_span` 自己已经在符号后面带一个空格, 这里不用再加。
                let spinner = Throbber::default().throbber_set(BRAILLE_SIX).to_symbol_span(&state);
                let line = Line::from(vec![spinner, Span::raw(s.wiz_loading_providers)]).centered();
                Self::draw_placeholder(frame, area, ctx.theme, s, line);
            }
            Stage::LoadFailed(reason) => {
                let line = Line::styled((s.wiz_load_failed)(reason), Style::new().fg(ctx.theme.err)).centered();
                Self::draw_placeholder(frame, area, ctx.theme, s, line);
            }
            Stage::Basics { form, phase } => {
                let focus = form.draw(frame, area, phase, &self.providers, &paint);
                if std::mem::take(&mut form.pending_field_err) {
                    if let Some((row, rect)) = focus {
                        ctx.fx.field_err(row, rect, ctx.theme.err);
                    }
                }
            }
            Stage::Slots { form, saving, .. } => {
                let focus = form.draw(frame, area, *saving, &paint);
                if std::mem::take(&mut form.pending_field_err) {
                    if let Some((row, rect)) = focus {
                        ctx.fx.field_err(row, rect, ctx.theme.err);
                    }
                }
            }
            Stage::Custom { form, phase } => {
                let focus = form.draw(frame, area, *phase, &paint);
                if std::mem::take(&mut form.pending_field_err) {
                    if let Some((row, rect)) = focus {
                        ctx.fx.field_err(row, rect, ctx.theme.err);
                    }
                }
            }
        }
        if let Some(dir) = self.pending_step_fx.take() {
            ctx.fx.wizard_step(dir, area, ctx.theme.border);
        }
    }

    /// `Loading` / `LoadFailed` 共用: 带边框的空容器 + 居中一行。
    fn draw_placeholder(frame: &mut Frame, area: Rect, theme: &Theme, s: &'static Strings, line: Line<'static>) {
        let block = Block::bordered().border_type(BorderType::Rounded).border_style(theme.border_style()).title_top(format!(" {} ", s.wiz_title));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(line, inner.centered_vertically(Constraint::Length(1)));
    }

    /// 底栏左侧。没有可操作字段的阶段 (加载中 / 失败 / 请求在飞) 留空; 编辑中的表单按**当前聚焦
    /// 行的类型**给提示: `↑↓ 字段` 常驻; 选择行追加 `⏎ 选择`; 文本行追加 `⏎ 下一项`, API Key 行
    /// 再多一条 `Ctrl+R 显示/隐藏`; 按钮行追加 `⏎` + **按钮自己的标签**, 让用户一眼知道回车会发生
    /// 什么。
    pub fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>> {
        match &self.stage {
            Stage::Basics { form, phase: BasicsPhase::Editing } => form.hints(s),
            Stage::Slots { form, saving: false, .. } => form.hints(s),
            Stage::Custom { form, phase: CustomPhase::Editing } => form.hints(s),
            Stage::Loading | Stage::LoadFailed(_) | Stage::Basics { .. } | Stage::Slots { .. } | Stage::Custom { .. } => Vec::new(),
        }
    }

    /// 请求在飞时能不能按 `Esc` 退出——`App::draw` 据此决定要不要在底栏右侧显示 `Esc 取消`。
    /// 只有**会落库**的请求 (创建 / 保存槽位) 在飞时才吞 `Esc`: 撤不回后端落库。
    pub fn can_cancel(&self) -> bool {
        match &self.stage {
            Stage::Loading | Stage::LoadFailed(_) => true,
            Stage::Basics { phase, .. } => *phase != BasicsPhase::Creating,
            Stage::Slots { saving, .. } => !saving,
            Stage::Custom { phase, .. } => *phase != CustomPhase::Creating,
        }
    }

    /// 用户已经填过东西 / 已经创建过订阅 —— `Esc` 要不要先确认看这个。编辑中的第一步看草稿;
    /// 请求一旦发出、或者已经到了第二步, 恒真; 自定义表单也恒真——能进来本身就意味着用户已经
    /// 从厂商 picker 里选了一个自定义条目 (与「厂商已选」是同一件事)。
    pub fn has_input(&self) -> bool {
        match &self.stage {
            Stage::Loading | Stage::LoadFailed(_) => false,
            Stage::Basics { form, phase } => *phase != BasicsPhase::Editing || form.has_input(),
            Stage::Slots { .. } | Stage::Custom { .. } => true,
        }
    }

    /// 与页面同一套: `update()` 内部想弹 toast 就存这里, `App` 调用后轮询取走。
    pub fn take_notice(&mut self) -> Option<(ToastKind, String)> {
        self.notice.take()
    }

    /// 向导在处理**结果**时想关掉自己 (保存成功 / 创建成功)。`update()` 只能返回
    /// `Vec<WizardCmd>`, 塞不进 `Action::CloseWizard`——与 `take_notice` 同一条出路。取走即清零。
    pub fn take_close_request(&mut self) -> bool {
        std::mem::take(&mut self.close_request)
    }

    /// 测试专用: 直接置位「向导想关闭自己」。
    #[cfg(test)]
    pub fn request_close_for_test(&mut self) {
        self.close_request = true;
    }

    /// 测试专用: 直接塞一条待发的 notice, 供 `App` 转发 `take_notice()` 的测试使用。
    #[cfg(test)]
    pub fn request_notice_for_test(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.notice = Some((kind, text.into()));
    }
}

/// 备注名跟着厂商名自动生成 (内置路径选厂商、自定义路径编辑厂商名共用): 备注名为空、**或**仍等于
/// 上一次自动生成的值时, 重算成 `default_display_name(source)` 并记下来; 用户手改过之后不再跟随。
/// 返回是否真的重算了 (调用方据此清掉备注名行自己的错误)。
fn follow_display_name(display_name: &mut TextField, last_auto: &mut Option<String>, source: &str, store: &Store) -> bool {
    let still_auto = display_name.value().is_empty() || last_auto.as_deref() == Some(display_name.value());
    if !still_auto {
        return false;
    }
    let generated = default_display_name(source, store);
    display_name.set(generated.clone());
    *last_auto = Some(generated);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::dto::{CreatedSubscription, ModelInfo};
    use fields::{BasicsField, CustomField, SlotsField};

    fn provider(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            display_name: id.to_string(),
            description: None,
            endpoints: vec![],
            default_endpoint: None,
            auth: crate::client::dto::ProviderAuth { auth_type: "api_key".into() },
            model_discovery: crate::client::dto::ModelDiscovery { enabled: true, example_models: vec![] },
            translations: crate::client::dto::ProviderTranslations { en: text(id), ja: text(id) },
            url_params: vec![],
        }
    }

    fn text(name: &str) -> crate::client::dto::ProviderText {
        crate::client::dto::ProviderText { display_name: name.to_string(), description: None, endpoints: Default::default(), url_params: Default::default() }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, ratatui::crossterm::event::KeyModifiers::NONE)
    }

    /// 向导自己不看代次 (`App` 已经按代次筛过), 这里随便给一个。
    fn done(result: WizardResult) -> Action {
        Action::WizardDone { epoch: 0, result: Box::new(result) }
    }

    fn at(stage: Stage) -> Wizard {
        Wizard { stage, ..Wizard::new() }
    }

    fn basics_at(phase: BasicsPhase) -> Wizard {
        at(Stage::Basics { form: BasicsForm::default(), phase })
    }

    fn slots_at(id: &str, saving: bool) -> Wizard {
        at(Stage::Slots { id: id.into(), name: String::new(), form: SlotsForm::new(SlotsDraft::default(), Vec::new(), None), saving })
    }

    fn custom_at(phase: CustomPhase) -> Wizard {
        at(Stage::Custom { form: Box::new(CustomForm::new(CustomProtocol::Anthropic)), phase })
    }

    fn basics(w: &mut Wizard) -> &mut BasicsForm {
        match &mut w.stage {
            Stage::Basics { form, .. } => form,
            _ => panic!("不在 Basics 阶段"),
        }
    }

    fn slots(w: &mut Wizard) -> &mut SlotsForm {
        match &mut w.stage {
            Stage::Slots { form, .. } => form,
            _ => panic!("不在 Slots 阶段"),
        }
    }

    fn custom(w: &mut Wizard) -> &mut CustomForm {
        match &mut w.stage {
            Stage::Custom { form, .. } => form.as_mut(),
            _ => panic!("不在 Custom 阶段"),
        }
    }

    #[test]
    fn on_open_requests_the_provider_list() {
        let mut w = Wizard::new();
        assert_eq!(w.on_open(), vec![WizardCmd::LoadProviders]);
    }

    #[test]
    fn has_no_input_yet_so_escape_closes_without_confirming() {
        let mut w = Wizard::new();
        assert!(!w.has_input());
        assert_eq!(w.handle_key(key(KeyCode::Esc), &Store::default(), &crate::i18n::ZH), Some(Action::CloseWizard));
    }

    /// 除 `Esc` 外的按键在 `Stage::Loading` 一律被吞掉——没有任何字段可以接收字符输入。
    #[test]
    fn other_keys_are_swallowed() {
        let mut w = Wizard::new();
        for code in [KeyCode::Char('q'), KeyCode::Char('r'), KeyCode::Char('1'), KeyCode::Tab, KeyCode::Enter] {
            assert_eq!(w.handle_key(key(code), &Store::default(), &crate::i18n::ZH), None, "{code:?}");
        }
    }

    #[test]
    fn a_successful_provider_list_is_recorded_quietly() {
        let mut w = Wizard::new();
        let action = done(WizardResult::Providers(Ok(vec![provider("zhipu")])));
        let cmds = w.update(&action, &Store::default(), &crate::i18n::ZH);
        assert!(cmds.is_empty());
        assert!(w.take_notice().is_none());
        assert!(!w.take_close_request());
        // 进了 `Stage::Basics`, 但草稿还是空的 (`has_input()` 仍然为假), Esc 照常直接关闭。
        assert_eq!(w.handle_key(key(KeyCode::Esc), &Store::default(), &crate::i18n::ZH), Some(Action::CloseWizard));
    }

    #[test]
    fn a_failed_provider_list_does_not_produce_a_command_notice_or_close_request() {
        let mut w = Wizard::new();
        let action = done(WizardResult::Providers(Err("network".into())));
        let cmds = w.update(&action, &Store::default(), &crate::i18n::ZH);
        assert!(cmds.is_empty());
        assert!(w.take_notice().is_none());
        assert!(!w.take_close_request());
    }

    #[test]
    fn request_close_for_test_is_taken_exactly_once() {
        let mut w = Wizard::new();
        assert!(!w.take_close_request());
        w.request_close_for_test();
        assert!(w.take_close_request());
        assert!(!w.take_close_request(), "取走之后应该清零");
    }

    /// 会落库的请求 (创建) 在飞时连 `Esc` 也该被吞掉, 不弹确认——两条路径的创建各一遍。
    #[test]
    fn escape_is_swallowed_while_a_request_is_in_flight() {
        for mut w in [basics_at(BasicsPhase::Creating), custom_at(CustomPhase::Creating)] {
            assert!(!w.can_cancel(), "在飞时不该能取消");
            assert_eq!(w.handle_key(key(KeyCode::Esc), &Store::default(), &crate::i18n::ZH), None, "在飞时 Esc 应该被吞掉");
        }
    }

    /// 同上, 保存槽位也会落库。
    #[test]
    fn escape_is_swallowed_while_saving() {
        let mut w = slots_at("sub-1", true);
        assert!(!w.can_cancel(), "保存在飞时不该能取消");
        assert_eq!(w.handle_key(key(KeyCode::Esc), &Store::default(), &crate::i18n::ZH), None, "保存在飞时 Esc 应该被吞掉");
    }

    /// 底栏提示按焦点行的类型变化, 四种行 (选择行 / API Key 文本行 / 备注名文本行 / 按钮行) 都要
    /// 断言到。按钮行要断言出现的是**按钮自己的标签**, 不是一个通用词。
    #[test]
    fn hints_follow_focus_on_every_basics_row_type() {
        let s = &crate::i18n::ZH;
        let mut w = basics_at(BasicsPhase::Editing);

        basics(&mut w).state.focus_on(BasicsField::Provider);
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.key_pick)], "选择行应该提示 ⏎ 选择");

        basics(&mut w).state.focus_on(BasicsField::ApiKey);
        assert_eq!(
            w.hints(s),
            vec![("↑↓", s.key_field), ("⏎", s.key_next_field), ("Ctrl+R", s.key_reveal)],
            "API Key 行额外带 Ctrl+R 提示"
        );

        basics(&mut w).state.focus_on(BasicsField::DisplayName);
        let hints = w.hints(s);
        assert_eq!(hints, vec![("↑↓", s.key_field), ("⏎", s.key_next_field)], "备注名行是文本行, 但不该有 Ctrl+R 提示");
        assert!(!hints.iter().any(|(k, _)| *k == "Ctrl+R"), "备注名行不该出现 Ctrl+R\n{hints:?}");

        basics(&mut w).state.focus_on(BasicsField::Submit);
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.wiz_btn_next)], "按钮行应该显示按钮自己的标签, 不是通用词");
    }

    /// 同上, 第二步的两种行: 槽位行 (选择行) 与 `Save` 按钮行。
    #[test]
    fn hints_follow_focus_on_every_slots_row_type() {
        let s = &crate::i18n::ZH;
        let mut w = slots_at("sub-1", false);

        slots(&mut w).state.focus_on(SlotsField::Row(Slot::Fable));
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.key_pick)], "槽位行应该提示 ⏎ 选择");

        slots(&mut w).state.focus_on(SlotsField::Save);
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.wiz_btn_save)], "保存按钮行应该显示它自己的标签");
    }

    /// 同上, 自定义表单覆盖全部行类型: 选择行 (`Protocol`)、解锁的鉴权选择行、锁定的鉴权行 (没有
    /// 额外提示)、四个普通文本行、API Key 文本行 (带 Ctrl+R)、`Probe` 按钮行、槽位选择行、
    /// `Submit` 按钮行。
    #[test]
    fn hints_follow_focus_on_every_custom_row_type() {
        let s = &crate::i18n::ZH;
        let mut w = custom_at(CustomPhase::Editing);

        custom(&mut w).state.focus_on(CustomField::Protocol);
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.key_pick)], "协议行应该提示 ⏎ 选择");

        // `custom_at` 默认造的是 Anthropic 协议, 鉴权头未锁定。
        assert!(!custom(&mut w).draft.protocol.auth_locked(), "准备: Anthropic 的鉴权头不该锁定");
        custom(&mut w).state.focus_on(CustomField::Auth);
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.key_pick)], "解锁的鉴权行应该提示 ⏎ 选择");

        let mut locked = custom_at(CustomPhase::Editing);
        custom(&mut locked).draft.apply_protocol(CustomProtocol::Gemini);
        custom(&mut locked).state.focus_on(CustomField::Auth);
        assert_eq!(locked.hints(s), vec![("↑↓", s.key_field)], "锁定的鉴权行不该有额外提示");

        for field in [CustomField::ProviderName, CustomField::BaseUrl, CustomField::MessagesPath, CustomField::DisplayName] {
            custom(&mut w).state.focus_on(field);
            assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.key_next_field)], "{field:?} 应该是普通文本行提示");
        }

        custom(&mut w).state.focus_on(CustomField::ApiKey);
        assert_eq!(
            w.hints(s),
            vec![("↑↓", s.key_field), ("⏎", s.key_next_field), ("Ctrl+R", s.key_reveal)],
            "API Key 行额外带 Ctrl+R 提示"
        );

        custom(&mut w).state.focus_on(CustomField::Probe);
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.wiz_btn_probe)], "Probe 按钮行应该显示它自己的标签");

        custom(&mut w).state.focus_on(CustomField::Slot(Slot::Fable));
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.key_pick)], "槽位行应该提示 ⏎ 选择");

        custom(&mut w).state.focus_on(CustomField::Submit);
        assert_eq!(w.hints(s), vec![("↑↓", s.key_field), ("⏎", s.wiz_btn_create)], "Submit 按钮行应该显示它自己的标签");
    }

    /// 编辑字段只清自己的错误——错误是单个 `Option`, 退化成「任何编辑都清」时正向用例照样能过,
    /// 必须有一条编辑别的字段、断言原字段错误还在的用例才咬得住。
    #[test]
    fn editing_one_field_does_not_clear_another_fields_error() {
        let s = &crate::i18n::ZH;
        let mut w = basics_at(BasicsPhase::Editing);
        basics(&mut w).state.reject(BasicsField::Provider, s.wiz_err_provider);
        basics(&mut w).state.focus_on(BasicsField::DisplayName);

        w.handle_key(key(KeyCode::Char('a')), &Store::default(), s);

        assert_eq!(basics(&mut w).state.error(), Some((BasicsField::Provider, s.wiz_err_provider)), "编辑备注名不该清掉厂商行的错误");
    }

    /// 统一编辑路径的正向用例 (两张表单各一条): 只移动光标不算编辑, 错误留着; 值真的变了才清掉
    /// 这个字段自己的错误。
    #[test]
    fn editing_a_text_field_clears_its_own_error_only_when_the_value_changes() {
        let s = &crate::i18n::ZH;
        let mut w = basics_at(BasicsPhase::Editing);
        basics(&mut w).state.reject(BasicsField::ApiKey, s.wiz_err_api_key);
        w.handle_key(key(KeyCode::Left), &Store::default(), s);
        assert_eq!(basics(&mut w).state.error(), Some((BasicsField::ApiKey, s.wiz_err_api_key)), "只移动光标不该清错误");
        w.handle_key(key(KeyCode::Char('x')), &Store::default(), s);
        assert_eq!(basics(&mut w).state.error(), None, "给 API Key 打字应该清掉它自己的错误");

        let mut c = custom_at(CustomPhase::Editing);
        custom(&mut c).state.reject(CustomField::BaseUrl, s.wiz_err_base_url_empty);
        c.handle_key(key(KeyCode::Left), &Store::default(), s);
        assert_eq!(custom(&mut c).state.error(), Some((CustomField::BaseUrl, s.wiz_err_base_url_empty)), "只移动光标不该清错误");
        c.handle_key(key(KeyCode::Char('h')), &Store::default(), s);
        assert_eq!(custom(&mut c).state.error(), None, "给 Base URL 打字应该清掉它自己的错误");
    }

    /// 同上, 槽位错误的负向用例: 给 `Fable` 挂错误, 选 `Opus` 的模型, `Fable` 的错误应该原封不动。
    #[test]
    fn selecting_one_slot_does_not_clear_another_slots_error() {
        let s = &crate::i18n::ZH;
        let mut w = slots_at("sub-1", false);
        slots(&mut w).state.reject(SlotsField::Row(Slot::Fable), s.wiz_err_slot);

        let pick = Action::PickerDone { tag: PickerTag::WizardSlot { slot: Slot::Opus }, choice: PickerChoice::Item("glm-4.6".into()) };
        w.update(&pick, &Store::default(), s);

        assert_eq!(slots(&mut w).draft.slots.opus, "glm-4.6", "准备: Opus 的选值应该已经写进草稿");
        assert_eq!(slots(&mut w).state.error(), Some((SlotsField::Row(Slot::Fable), s.wiz_err_slot)), "选定 Opus 的模型不该清掉 Fable 行的错误");
    }

    /// 第一步 `Esc` (有输入时) 用的是 `confirm_discard`, 不是订阅已建好之后的
    /// `wiz_confirm_exit_pending`——第一步订阅还没建, 用错文案会让用户以为已经建出了一条订阅。
    #[test]
    fn basics_escape_uses_the_discard_prompt_not_the_pending_one() {
        let s = &crate::i18n::ZH;
        let mut w = basics_at(BasicsPhase::Editing);
        basics(&mut w).draft.provider_id = "zhipu".into(); // has_input() 为真
        assert_eq!(
            w.handle_key(key(KeyCode::Esc), &Store::default(), s),
            Some(Action::OpenConfirm { prompt: s.confirm_discard.to_string(), on_yes: OnYes::discard_then(Action::CloseWizard) })
        );
    }

    /// 等模型列表是只读请求 (不落库), `Esc` 应该可用, 且文案是 `wiz_confirm_exit_pending`
    /// (订阅已经建好了, 与在 `Slots` 退出是一回事)。
    #[test]
    fn escape_is_available_while_loading_models() {
        let s = &crate::i18n::ZH;
        let mut w = basics_at(BasicsPhase::LoadingModels { id: "sub-1".into() });
        assert!(w.can_cancel(), "等模型列表时应该能取消");
        assert_eq!(
            w.handle_key(key(KeyCode::Esc), &Store::default(), s),
            Some(Action::OpenConfirm { prompt: s.wiz_confirm_exit_pending.to_string(), on_yes: OnYes::discard_then(Action::CloseWizard) })
        );
    }

    /// 每个异步结果只在发起它的那个阶段被接受——构造一个「晚到」的结果喂进不对的阶段, 断言阶段与
    /// 数据都不变, 也不产出请求。
    ///
    /// `Providers` 晚到 (向导已经走到 `Slots`): 不该把表单打回 `Basics`, 也不该采纳这份厂商列表。
    #[test]
    fn a_stale_provider_list_is_discarded_outside_loading() {
        let mut w = slots_at("sub-1", false);
        slots(&mut w).state.focus_on(SlotsField::Save);
        let action = done(WizardResult::Providers(Ok(vec![provider("late")])));
        let cmds = w.update(&action, &Store::default(), &crate::i18n::ZH);
        assert!(cmds.is_empty());
        assert!(w.providers.is_empty(), "过期的厂商列表不该被采纳\n{:?}", w.providers);
        assert!(matches!(w.stage, Stage::Slots { .. }), "阶段不该被晚到的厂商列表打回 Basics");
        assert_eq!(slots(&mut w).state.focus(), SlotsField::Save);
    }

    /// `Created` 晚到: 已经在 `Slots` 或已经在等模型列表 (订阅 id 已经是真实值) 时, 不该覆盖 id,
    /// 也不该再发一次 `LoadModels`; 编辑中的两张表单同样不该被它推进或关掉。
    #[test]
    fn a_stale_created_result_is_discarded_outside_creating() {
        let stale = done(WizardResult::Created(Ok(CreatedSubscription { id: "stale-id".into() })));

        let mut w = slots_at("real-id", false);
        let cmds = w.update(&stale, &Store::default(), &crate::i18n::ZH);
        assert!(cmds.is_empty(), "不该再发一次 LoadModels");
        assert!(matches!(&w.stage, Stage::Slots { id, .. } if id == "real-id"), "订阅 id 不该被晚到的结果覆盖");

        let mut w = basics_at(BasicsPhase::LoadingModels { id: "real-id".into() });
        let cmds = w.update(&stale, &Store::default(), &crate::i18n::ZH);
        assert!(cmds.is_empty(), "不该再发一次 LoadModels");
        assert!(
            matches!(&w.stage, Stage::Basics { phase: BasicsPhase::LoadingModels { id }, .. } if id == "real-id"),
            "等模型列表时订阅 id 不该被晚到的结果覆盖"
        );

        let mut w = basics_at(BasicsPhase::Editing);
        assert!(w.update(&stale, &Store::default(), &crate::i18n::ZH).is_empty());
        assert!(matches!(w.stage, Stage::Basics { phase: BasicsPhase::Editing, .. }), "编辑中的第一步不该被推进");

        for phase in [CustomPhase::Editing, CustomPhase::Probing] {
            let mut w = custom_at(phase);
            assert!(w.update(&stale, &Store::default(), &crate::i18n::ZH).is_empty());
            assert!(!w.take_close_request(), "{phase:?}: 不该关向导");
            assert!(w.take_notice().is_none(), "{phase:?}: 不该弹 toast");
            assert!(matches!(w.stage, Stage::Custom { phase: p, .. } if p == phase), "{phase:?}: 阶段不该变");
        }
    }

    /// `Models` 晚到 (还在创建, 没到等模型列表): 不该凭空进 `Slots`。
    #[test]
    fn a_stale_models_result_is_discarded_outside_loading_models() {
        let mut w = basics_at(BasicsPhase::Creating);
        let action = done(WizardResult::Models {
            id: "sub-1".into(),
            result: Ok(RefreshModelsResult::Auto { models: vec![ModelInfo { id: "glm-4.6".into(), display_name: None }], fetched_at: 0 }),
        });
        let cmds = w.update(&action, &Store::default(), &crate::i18n::ZH);
        assert!(cmds.is_empty());
        assert!(matches!(w.stage, Stage::Basics { phase: BasicsPhase::Creating, .. }), "阶段不该被晚到的 Models 结果打进 Slots");
    }

    /// `SlotsSaved` 晚到 (没有保存在飞): 不该关向导、不该弹 toast。
    #[test]
    fn a_stale_slots_saved_result_is_discarded_outside_saving() {
        let mut w = slots_at("sub-1", false);
        let action = done(WizardResult::SlotsSaved(Ok(())));
        let cmds = w.update(&action, &Store::default(), &crate::i18n::ZH);
        assert!(cmds.is_empty());
        assert!(!w.take_close_request(), "不该关向导");
        assert!(w.take_notice().is_none(), "不该弹 toast");
        assert!(matches!(w.stage, Stage::Slots { saving: false, .. }));
    }

    fn systemone_provider() -> Provider {
        let mut p = provider("ollama");
        p.endpoints = vec![crate::client::dto::ProviderEndpoint {
            id: "systemone".into(),
            label: "System One".into(),
            base_url: "http://localhost:11434".into(),
            protocol: "systemone".into(),
            example_models: vec!["clef-flash".into()],
            url_params_used: vec![],
        }];
        p.default_endpoint = Some("systemone".into());
        p
    }

    /// System One 端点: 第一步提交时不发 `(pending)` 占位槽, 发全空槽 (Jev 空 = 透传)。
    #[test]
    fn systemone_basics_submit_sends_empty_slots() {
        let s = &crate::i18n::ZH;
        let providers = vec![systemone_provider()];
        let mut form = BasicsForm::default();
        form.draft.provider_id = "ollama".into();
        form.draft.endpoint_id = "systemone".into();
        form.draft.api_key = super::text::SecretField::new("sk-test");
        form.draft.display_name.set("Ollama");
        form.state.focus_on(BasicsField::Submit);
        let mut phase = BasicsPhase::Editing;
        let action = form.handle_key(key(KeyCode::Enter), &mut phase, &providers, s);
        let Some(Action::WizardRequest(cmd)) = action else { panic!("应该发出创建请求: {action:?}") };
        let WizardCmd::Create(input) = *cmd else { panic!("应该是 Create") };
        assert_eq!(input.model_slots, ModelSlots::default());
        assert_eq!(phase, BasicsPhase::Creating);
    }

    /// System One 端点创建成功: 不拉模型列表, 直接进只有 Jev 一行的第二步, 候选是端点的示例模型。
    #[test]
    fn systemone_created_skips_model_loading_and_offers_only_the_jev_slot() {
        let s = &crate::i18n::ZH;
        let mut w = basics_at(BasicsPhase::Creating);
        w.providers = vec![systemone_provider()];
        basics(&mut w).draft.provider_id = "ollama".into();
        basics(&mut w).draft.endpoint_id = "systemone".into();
        basics(&mut w).draft.display_name.set(" Ollama ");
        let cmds = w.update(&done(WizardResult::Created(Ok(CreatedSubscription { id: "sub-9".into() }))), &Store::default(), s);
        assert!(cmds.is_empty(), "不该拉模型列表: {cmds:?}");
        match &w.stage {
            Stage::Slots { id, name, saving, .. } => {
                assert_eq!(id, "sub-9");
                assert_eq!(name, "Ollama");
                assert!(!saving);
            }
            _ => panic!("应该直接进 Slots 阶段"),
        }
        let form = slots(&mut w);
        assert_eq!(common::FormFields::order(form), vec![SlotsField::Row(Slot::Jev), SlotsField::Save]);
        assert_eq!(form.state.focus(), SlotsField::Row(Slot::Jev));
        assert_eq!(form.examples, vec!["clef-flash".to_string()]);
    }

    /// 厂商选择弹窗的每一项都带能力标记: 对话 / Jev / 两者都有 / 自定义项恒为 LLM。
    #[test]
    fn provider_picker_labels_carry_capability_suffixes() {
        let s = &crate::i18n::EN;
        let ep = |protocol: &str| crate::client::dto::ProviderEndpoint {
            id: protocol.into(),
            label: protocol.into(),
            base_url: "u".into(),
            protocol: protocol.into(),
            example_models: vec![],
            url_params_used: vec![],
        };
        let mut llm = provider("llm");
        llm.endpoints = vec![ep("messages")];
        let mut jev = provider("jev");
        jev.endpoints = vec![ep("systemone")];
        let mut both = provider("both");
        both.endpoints = vec![ep("messages"), ep("systemone")];
        let mut oauth = provider("kiro");
        oauth.endpoints = vec![ep("messages")];
        oauth.auth.auth_type = "kiro_oauth".into();
        let mut w = basics_at(BasicsPhase::Editing);
        let Some(Action::OpenPicker(spec)) =
            basics(&mut w).handle_key(key(KeyCode::Enter), &mut BasicsPhase::Editing, &[llm, jev, both, oauth], s)
        else {
            panic!("厂商行 回车 应该开选择弹窗")
        };
        let labels: Vec<&str> = spec.items.iter().map(|i| i.label.as_str()).collect();
        let oauth_label = format!("kiro [LLM] · {}", s.wiz_desktop_only);
        assert_eq!(&labels[..4], ["llm [LLM]", "jev [Jev]", "both [LLM] [Jev]", oauth_label.as_str()]);
        assert!(labels[4..].iter().all(|l| l.ends_with(" [LLM]")), "{labels:?}");
    }

    /// Jev 槽可以留空 (= 透传): 直接保存, 发全空槽。选择弹窗置顶「清空」项。
    #[test]
    fn systemone_slots_save_with_an_empty_jev_slot() {
        let s = &crate::i18n::ZH;
        let mut form = SlotsForm::new_systemone(vec!["clef-flash".into()]);
        let picker = form.handle_key(key(KeyCode::Enter), "sub-9", &mut false, s);
        let Some(Action::OpenPicker(spec)) = picker else { panic!("Jev 行 ⏎ 应该开选择弹窗: {picker:?}") };
        assert_eq!(spec.items.first().map(|i| (i.id.as_str(), i.label.as_str())), Some(("", s.pick_clear_jev)));
        assert!(spec.items.iter().any(|i| i.id == "clef-flash"));

        form.handle_key(key(KeyCode::Down), "sub-9", &mut false, s);
        let mut saving = false;
        let action = form.handle_key(key(KeyCode::Enter), "sub-9", &mut saving, s);
        assert_eq!(
            action,
            Some(Action::WizardRequest(Box::new(WizardCmd::SaveSlots { id: "sub-9".into(), model_slots: ModelSlots::default() })))
        );
        assert!(saving);
    }

    /// 创建成功的提示用 trim 后的备注名——与发给后端、落库的值一致。内置路径的名字在进第二步时
    /// 记下, 保存成功时才弹; 自定义路径创建成功即弹。两条都用首尾带空格的输入走一遍。
    #[test]
    fn created_notice_uses_the_trimmed_display_name() {
        let s = &crate::i18n::ZH;
        let expected = Some((ToastKind::Success, (s.wiz_created)("主力")));

        let mut w = basics_at(BasicsPhase::LoadingModels { id: "sub-1".into() });
        basics(&mut w).draft.display_name.set("  主力  ");
        let models = done(WizardResult::Models {
            id: "sub-1".into(),
            result: Ok(RefreshModelsResult::Auto { models: vec![ModelInfo { id: "glm-4.6".into(), display_name: None }], fetched_at: 0 }),
        });
        w.update(&models, &Store::default(), s);
        match &mut w.stage {
            Stage::Slots { name, saving, .. } => {
                assert_eq!(name, "主力");
                *saving = true;
            }
            _ => panic!("应该进了 Slots 阶段"),
        }
        w.update(&done(WizardResult::SlotsSaved(Ok(()))), &Store::default(), s);
        assert_eq!(w.take_notice(), expected, "内置路径");

        let mut w = custom_at(CustomPhase::Creating);
        custom(&mut w).draft.display_name.set("  主力  ");
        w.update(&done(WizardResult::Created(Ok(CreatedSubscription { id: "sub-2".into() }))), &Store::default(), s);
        assert_eq!(w.take_notice(), expected, "自定义路径");
    }
}
