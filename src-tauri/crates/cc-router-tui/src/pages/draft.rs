//! 页面草稿的通用生命周期 (D3, fix round P3b)。订阅详情页 ([`crate::pages::subscriptions`]) 与
//! 虚拟模型页 ([`crate::pages::virtual_models`]) 在 Task 5/6 里各自长出了一份几乎相同的「草稿」逻辑:
//! 首次编辑时从 `Store` 惰性克隆、与 base 相等时视为没有修改、只有保存成功才清空。两份 review 都
//! 挑出同一对缺陷——在途保存吞掉后续编辑 (D1)、草稿与 `Store` 相等却仍然占着位置的"僵尸"草稿
//! (D2)——根因都是"哪里该刷新 dirty / 该不该清草稿"分散在 5-6 个改动入口各自记, 容易漏。
//!
//! `Draft<T>` 把这套规则收进一个类型, 只留一个编辑入口 ([`Draft::edit`]): **草稿的值与当前 base
//! 相等就立刻丢弃**, 不管是编辑完之后 (`edit`) 还是 `Store` 单独变了 (`sync`) 触发的重新核对——
//! "5-6 个入口各自记得刷新" 这个问题由此从"约定"变成"类型保证不了忘记的事"。
//!
//! `sync`/`refresh_dirty` 的区分对应仓库根 `CLAUDE.md`「TUI 界面架构」一段的单向数据流规则:
//! `draw()` 不改业务状态, 只允许重算几何 / 取走待播动效这类允许的 `&mut`——真正丢弃草稿 (业务状态
//! 变更) 只能发生在 `update()`/`handle_key()` 里 (`sync`), `draw()` 每帧只能重算一遍缓存的 `dirty`
//! 布尔 (`refresh_dirty`), 不能顺带把草稿本身也清空, 否则"同一状态画两次" 的前提就被破坏了。
//!
//! 草稿只放页面自己的状态, 不进 `Store` (spec 全局约束)——`Draft<T>` 本身完全不碰 `Store`, 调用方
//! 每次都把当前应该比较的基准值 (`base`) 传进来。

#[derive(Debug, Clone)]
pub struct Draft<T: PartialEq + Clone> {
    value: Option<T>,
    /// `is_dirty()` 的缓存: 草稿存在且与最近一次核对时的 base 不同。`value` 与这个缓存永远一起
    /// 维护 (`edit`/`sync`/`refresh_dirty`/`clear` 各自负责), 调用方不需要 (也不该) 单独计算它。
    dirty: bool,
}

impl<T: PartialEq + Clone> Default for Draft<T> {
    fn default() -> Self {
        Self { value: None, dirty: false }
    }
}

impl<T: PartialEq + Clone> Draft<T> {
    /// 当前草稿值; `None` = 没有未保存的修改。
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// 唯一的编辑入口。草稿不存在时从 `base` 惰性克隆, 存在就直接复用 (保留上一步的修改, **不会**
    /// 被这一刻的 `base` 悄悄重置——`base` 可能是这一刻才从 `Store` 读到的新值, 用户在途编辑期间
    /// 不该被它打断, 这正是 D1 的教训)。应用 `f` 之后, 结果如果与 `base` 相等就视为没有修改,
    /// 立刻丢弃草稿——D2 的"零编辑不留草稿 / 改回原值清脏"由这一条规则统一保证, 调用方不需要在
    /// 每个改动点各自记得再核对一遍。
    pub fn edit(&mut self, base: &T, f: impl FnOnce(&mut T)) {
        let mut value = self.value.take().unwrap_or_else(|| base.clone());
        f(&mut value);
        if &value == base {
            self.value = None;
            self.dirty = false;
        } else {
            self.value = Some(value);
            self.dirty = true;
        }
    }

    /// `update()`/`handle_key()` 专用的核对: 草稿与当前 `base` 相等 (改回原值, 或者别的客户端把
    /// `Store` 改成了跟草稿一样) 就真的丢弃; `base` 是 `None` (草稿对应的实体从 `Store` 消失) 也
    /// 丢弃, 返回值告诉调用方"消失了"——具体要不要因此挪焦点 / 弹通知是页面自己的业务逻辑,
    /// `Draft<T>` 不管。**不要在 `draw()` 里调这个**——它会真的修改 `value`, 属于业务状态变更;
    /// `draw()` 只能调 [`Self::refresh_dirty`]。
    pub fn sync(&mut self, base: Option<&T>) -> bool {
        let Some(value) = &self.value else {
            self.dirty = false;
            return false;
        };
        match base {
            None => {
                self.value = None;
                self.dirty = false;
                true
            }
            Some(base) => {
                self.dirty = value != base;
                if !self.dirty {
                    self.value = None;
                }
                false
            }
        }
    }

    /// `draw()` 专用: 只重算缓存的 `dirty` 布尔, 绝不碰 `value`——保证「同一状态画两次得到同一帧」
    /// 不受影响。草稿对应的实体从 `Store` 消失 (`base` 为 `None`) 时按"不脏"处理 (与
    /// [`Self::sync`] 的丢弃在视觉上一致: 都不再显示 `*`), 真正的清理留给下一次 `update()` 走
    /// [`Self::sync`]。
    pub fn refresh_dirty(&mut self, base: Option<&T>) {
        self.dirty = match (&self.value, base) {
            (Some(v), Some(b)) => v != b,
            _ => false,
        };
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 显式放弃当前草稿 (比如 Esc 确认放弃、保存成功之后)。
    pub fn clear(&mut self) {
        self.value = None;
        self.dirty = false;
    }

    /// D1: 一次保存请求发出去的那一刻草稿是什么样, 跟结果落地那一刻的草稿是否还是同一份——只有
    /// 相等才能安全清空, 否则会把保存在途期间的新编辑悄悄冲掉。
    pub fn matches(&self, saved: &T) -> bool {
        self.value.as_ref() == Some(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct V(i32);

    #[test]
    fn edit_clones_lazily_and_reuses_the_existing_value() {
        let mut d: Draft<V> = Draft::default();
        assert_eq!(d.get(), None);
        d.edit(&V(1), |v| v.0 = 2);
        assert_eq!(d.get(), Some(&V(2)));
        assert!(d.is_dirty());

        // 第二次编辑: base 变了 (比如 Store 又刷新了一次), 但已有草稿应该被复用, 不被新 base 重置。
        d.edit(&V(9), |v| v.0 += 1);
        assert_eq!(d.get(), Some(&V(3)), "应该在已有草稿 2 上继续加, 不是从新 base 9 重新开始");
    }

    #[test]
    fn edit_drops_the_draft_when_the_result_equals_base() {
        let mut d: Draft<V> = Draft::default();
        d.edit(&V(1), |v| v.0 = 2);
        assert!(d.get().is_some());
        d.edit(&V(1), |v| v.0 = 1); // 改回原值
        assert_eq!(d.get(), None, "改回 base 应该丢弃草稿");
        assert!(!d.is_dirty());
    }

    #[test]
    fn edit_never_creates_a_draft_for_a_true_noop() {
        let mut d: Draft<V> = Draft::default();
        d.edit(&V(1), |_| {}); // 什么都没改
        assert_eq!(d.get(), None, "零编辑不该留下草稿");
        assert!(!d.is_dirty());
    }

    #[test]
    fn sync_drops_when_equal_and_reports_vanished() {
        let mut d: Draft<V> = Draft::default();
        d.edit(&V(1), |v| v.0 = 2);
        assert!(!d.sync(Some(&V(2))), "与新 base 相等: 丢弃, 不算消失");
        assert_eq!(d.get(), None);
        assert!(!d.is_dirty());

        d.edit(&V(1), |v| v.0 = 2);
        assert!(!d.sync(Some(&V(1))), "与 base 不等: 保留");
        assert_eq!(d.get(), Some(&V(2)));
        assert!(d.is_dirty());

        assert!(d.sync(None), "base 消失应该丢弃并报告 vanished");
        assert_eq!(d.get(), None);
        assert!(!d.is_dirty());
    }

    #[test]
    fn sync_on_an_empty_draft_is_a_noop() {
        let mut d: Draft<V> = Draft::default();
        assert!(!d.sync(Some(&V(1))));
        assert!(!d.sync(None));
        assert!(!d.is_dirty());
    }

    #[test]
    fn refresh_dirty_never_touches_the_value() {
        let mut d: Draft<V> = Draft::default();
        d.edit(&V(1), |v| v.0 = 2);
        d.refresh_dirty(Some(&V(2))); // 与新 base 相等
        assert!(!d.is_dirty(), "缓存应该变成不脏");
        assert_eq!(d.get(), Some(&V(2)), "但草稿本身不该被 draw 安全的 refresh_dirty 清掉");

        d.refresh_dirty(None); // base 消失
        assert!(!d.is_dirty());
        assert_eq!(d.get(), Some(&V(2)), "消失也不该在 refresh_dirty 里真的丢弃");
    }

    #[test]
    fn matches_checks_the_current_value_not_just_dirtiness() {
        let mut d: Draft<V> = Draft::default();
        assert!(!d.matches(&V(1)), "没有草稿时不该匹配任何东西");
        d.edit(&V(1), |v| v.0 = 2);
        assert!(d.matches(&V(2)));
        assert!(!d.matches(&V(3)));
    }

    #[test]
    fn clear_drops_unconditionally() {
        let mut d: Draft<V> = Draft::default();
        d.edit(&V(1), |v| v.0 = 2);
        d.clear();
        assert_eq!(d.get(), None);
        assert!(!d.is_dirty());
    }
}
