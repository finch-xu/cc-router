import { useMemo, useState, type ReactNode } from "react";
import { Copy, Check } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { cn } from "@/lib/utils";

interface Props {
  text: string;
  className?: string;
  /** 是否对 env 片段做轻量语法着色 */
  highlight?: boolean;
}

export function CopyableBlock({ text, className, highlight = true }: Props) {
  const [copied, setCopied] = useState(false);
  const content = useMemo(
    () => (highlight ? renderHighlighted(text) : text),
    [text, highlight],
  );

  async function copy() {
    try {
      await writeText(text);
    } catch {
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        /* ignore */
      }
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  return (
    <pre className={cn("codeblock", className)}>
      <button className="copy" onClick={copy} type="button">
        {copied ? (
          <>
            <Check size={11} /> 已复制
          </>
        ) : (
          <>
            <Copy size={11} /> 复制
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
