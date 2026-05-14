import type { ComponentType } from "react";
import { Bot, Boxes } from "lucide-react";
import Anthropic from "@lobehub/icons/es/Anthropic";
import DeepSeek from "@lobehub/icons/es/DeepSeek";
import Moonshot from "@lobehub/icons/es/Moonshot";
import Zhipu from "@lobehub/icons/es/Zhipu";
import Minimax from "@lobehub/icons/es/Minimax";
import XiaomiMiMo from "@lobehub/icons/es/XiaomiMiMo";
import AlibabaCloud from "@lobehub/icons/es/AlibabaCloud";
import Volcengine from "@lobehub/icons/es/Volcengine";
import OpenRouter from "@lobehub/icons/es/OpenRouter";
import TencentCloud from "@lobehub/icons/es/TencentCloud";
import Ollama from "@lobehub/icons/es/Ollama";
import Fireworks from "@lobehub/icons/es/Fireworks";
import Stepfun from "@lobehub/icons/es/Stepfun";
import BaiduCloud from "@lobehub/icons/es/BaiduCloud";
import ModelScope from "@lobehub/icons/es/ModelScope";
import OpenAI from "@lobehub/icons/es/OpenAI";
import Gemini from "@lobehub/icons/es/Gemini";
import { cn } from "@/lib/utils";

type IconVariant = ComponentType<{ size?: number | string }>;

type BrandIcon = IconVariant & {
  Color?: IconVariant;
  colorPrimary: string;
};

const BRAND_MAP: Record<string, BrandIcon> = {
  anthropic: Anthropic as unknown as BrandIcon,
  deepseek: DeepSeek as unknown as BrandIcon,
  moonshot: Moonshot as unknown as BrandIcon,
  zhipu: Zhipu as unknown as BrandIcon,
  minimax: Minimax as unknown as BrandIcon,
  xiaomi: XiaomiMiMo as unknown as BrandIcon,
  alibaba: AlibabaCloud as unknown as BrandIcon,
  volcengine: Volcengine as unknown as BrandIcon,
  openrouter: OpenRouter as unknown as BrandIcon,
  tencent: TencentCloud as unknown as BrandIcon,
  ollama: Ollama as unknown as BrandIcon,
  fireworks: Fireworks as unknown as BrandIcon,
  stepfun: Stepfun as unknown as BrandIcon,
  baidu: BaiduCloud as unknown as BrandIcon,
  modelscope: ModelScope as unknown as BrandIcon,
  openai: OpenAI as unknown as BrandIcon,
  openai_codex: OpenAI as unknown as BrandIcon,
  google: Gemini as unknown as BrandIcon,
  google_ai_studio: Gemini as unknown as BrandIcon,
};

interface Props {
  iconId?: string | null;
  size?: number;
  className?: string;
  /** true 时跳过 .Color 变体使用单色 currentColor 形态; 默认 false 保持现有行为 */
  monochrome?: boolean;
}

export function ProviderIcon({ iconId, size = 20, className, monochrome = false }: Props) {
  // 自定义订阅: 不属于 BRAND_MAP 的任何品牌, 用一个通用图标区分于"未知"
  if (iconId === "custom") {
    return (
      <Boxes
        className={cn("text-muted-foreground shrink-0", className)}
        style={{ width: size, height: size }}
      />
    );
  }
  const Brand = iconId ? BRAND_MAP[iconId] : undefined;
  if (!Brand) {
    return (
      <Bot
        className={cn("text-muted-foreground shrink-0", className)}
        style={{ width: size, height: size }}
      />
    );
  }
  if (Brand.Color && !monochrome) {
    return (
      <span className={cn("inline-flex shrink-0", className)} aria-hidden>
        <Brand.Color size={size} />
      </span>
    );
  }
  return (
    <span
      className={cn("inline-flex shrink-0", className)}
      style={{ color: "currentColor" }}
      aria-hidden
    >
      <Brand size={size} />
    </span>
  );
}
