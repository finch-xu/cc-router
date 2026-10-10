这一版把 OpenAI 风格的模型别名改成 astra / sol / terra / luna 四档，与 fable / opus / sonnet / haiku 一一对应；设置页的网页界面卡片现在直接显示登录令牌。别名调整是不兼容变更：`gpt-*-sol`、`gpt-*-terra`、`gpt-*-luna` 各下移一档，`gpt-5.5` 这类纯版本号和 `gpt-*-mini` 不再识别，在 Codex 等客户端里用了这些名字的，升级后需要按下面的对照改一次。使用 `model-*` 或 `claude-*` 名字的不受影响。

## 新功能
- **OpenAI 风格模型别名改为四档**（不兼容变更）：客户端可以写的 `gpt-*` 别名改为 astra / sol / terra / luna，按名字里的档位段匹配，不看版本号，`gpt-6-sol`、`gpt-6.1-sol`、`openai/gpt-6-sol` 都算 sol 档。
  - `gpt-*-astra` 对应 `model-fable`（新增）。
  - `gpt-*-sol` 对应 `model-opus`（原来对应 `model-fable`）。
  - `gpt-*-terra` 对应 `model-sonnet`（原来对应 `model-opus`）。
  - `gpt-*-luna` 对应 `model-haiku`（原来对应 `model-sonnet`）。
  - `gpt-5.6`、`gpt-5.5`、`gpt-5.4` 与 `gpt-*-mini` 不再识别，用这些名字的请求会走 `model-fallback`。请改用上面的档位名，或直接写 `model-opus` 这类虚拟模型名。
- **网页界面卡片显示登录令牌**：在设置 → Web & TUI → 网页界面，「登录鉴权」开着时，访问地址下方多一行「登录令牌」，可以直接复制。
  - 此前令牌只在「安全与访问」标签里开着「Token 鉴权」时才显示，关着就找不到；网页登录页的提示也改为指向新位置。

## 其他
- **检查更新页的新版本内容**：按格式显示标题、列表、粗体与链接，并且只显示当前界面语言的那一段，不再把三种语言的全文以纯文本整块显示。
- **`GET /v1/models` 模型列表更新**：Claude 别名换成每档 5.5 与 6 两个版本（如 `claude-opus-5-5`、`claude-opus-6`），OpenAI 别名换成四档各一个（如 `gpt-6-sol`），共 32 条。`claude-opus-4-7` 这类旧写法仍然可以路由，只是不再出现在列表里。
- 接入指南 → 通用接入方式的模型表与实时路由页的别名提示已按新规则更新。
