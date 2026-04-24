import { useState } from "react";
import { Copy, Check } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

interface Props {
  text: string;
  className?: string;
}

export function CopyableBlock({ text, className }: Props) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await writeText(text);
    } catch {
      // fallback: 浏览器剪贴板
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
    <div className={cn("relative", className)}>
      <pre className="overflow-x-auto rounded-md border bg-muted p-4 font-mono text-xs leading-relaxed">
        {text}
      </pre>
      <Button
        variant="outline"
        size="sm"
        className="absolute right-2 top-2 h-7 px-2 text-xs"
        onClick={copy}
      >
        {copied ? (
          <>
            <Check className="h-3 w-3" /> 已复制
          </>
        ) : (
          <>
            <Copy className="h-3 w-3" /> 复制
          </>
        )}
      </Button>
    </div>
  );
}
