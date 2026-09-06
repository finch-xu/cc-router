import { useMemo, useState, type ReactNode } from "react";
import { Copy, Check } from "lucide-react";
import { runtime } from "@/runtime";
import { cn } from "@/lib/utils";
import { useT } from "@/i18n";

interface Props {
  text: string;
  className?: string;
  /** 是否对 env 片段做轻量语法着色,只对 shell env 行有意义 */
  highlight?: boolean;
  /**
   * block (默认): 终端风格深底多行代码块, 明暗主题下都保持深底.
   * inline: 单行浅底, 用主题变量, 随明暗主题反转. 用于设置页里的 URL / 地址这类一行内容.
   */
  variant?: "block" | "inline";
}

export function CopyableBlock({ text, className, highlight = false, variant = "block" }: Props) {
  const { t } = useT();
  const [copied, setCopied] = useState(false);
  const content = useMemo(
    () => (highlight ? renderHighlighted(text) : text),
    [text, highlight],
  );

  async function copy() {
    try {
      await runtime.copyText(text);
    } catch {
      /* ignore */
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  if (variant === "inline") {
    return (
      <div className={cn("copyline", className)}>
        <span className="copyline-text">{text}</span>
        <button className="copyline-copy" onClick={copy} type="button">
          {copied ? <Check size={11} /> : <Copy size={11} />}
          {copied ? t("copyable.copied") : t("copyable.copy")}
        </button>
      </div>
    );
  }

  return (
    <pre className={cn("codeblock", className)}>
      <button className="copy" onClick={copy} type="button">
        {copied ? (
          <>
            <Check size={11} /> {t("copyable.copied")}
          </>
        ) : (
          <>
            <Copy size={11} /> {t("copyable.copy")}
          </>
        )}
      </button>
      {content}
    </pre>
  );
}

const KEYWORDS = new Set(["export", "set", "$env:"]);

/** 极简 env 着色: `<keyword> KEY=VALUE` 三段染色 */
function renderHighlighted(text: string): ReactNode {
  return text.split("\n").map((line, i) => {
    const match = /^(\s*)(\S+)\s+([A-Z_][A-Z0-9_]*)=(.*)$/.exec(line);
    if (match && KEYWORDS.has(match[2])) {
      const [, indent, kw, key, val] = match;
      return (
        <div key={i}>
          {indent}
          <span className="k">{kw}</span> {key}=<span className="v">{val}</span>
        </div>
      );
    }
    return <div key={i}>{line || " "}</div>;
  });
}
