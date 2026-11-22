import { forwardRef, useMemo } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QRCodeSVG } from "qrcode.react";
import JsBarcode from "jsbarcode";
import logoUrl from "@/assets/logo.png";
import { scopedCss } from "@/plugins/receipt/css";
import { receiptData, type ReceiptContext } from "@/plugins/receipt/fields";
import { escapeHtml, renderTemplate } from "@/plugins/receipt/template";
import type { CompiledReceipt } from "@/plugins/receipt/validate";
import type { ReceiptDto } from "@/types";

const SITE_URL = "https://ccrouter.app";
const DATA_URL_RE = /^data:[a-z0-9.+/-]+;base64,[A-Za-z0-9+/]+=*$/;
const FONT_FORMAT: Record<string, string> = { woff2: "woff2", woff: "woff", ttf: "truetype", otf: "opentype" };

export function receiptScope(pluginId: string): string {
  return `ccr-rcpt-${pluginId}`;
}

/** 作用域样式 + 插件自带字体的 @font-face。字体以 data URL 内嵌, 导出的 HTML 离线也能用。 */
export function pluginReceiptCss(pluginId: string, r: CompiledReceipt, assetUrl: (p: string) => string | null): string {
  let faces = "";
  for (const [name, path] of Object.entries(r.fonts)) {
    const u = assetUrl(path);
    if (!u || !DATA_URL_RE.test(u)) continue;
    const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
    faces += `@font-face { font-family: "${name}"; src: url("${u}") format("${FONT_FORMAT[ext] ?? "woff2"}"); font-display: swap; }\n`;
  }
  return faces + scopedCss(r.rules, receiptScope(pluginId));
}

function qrSvg(size: number): string {
  return renderToStaticMarkup(<QRCodeSVG value={SITE_URL} size={size} fgColor="currentColor" bgColor="transparent" level="M" />);
}

function barcodeSvg(height: number): string {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  try {
    JsBarcode(el, SITE_URL, { format: "CODE128", width: 1, height, displayValue: false, margin: 0, lineColor: "currentColor", background: "transparent" });
  } catch {
    return "";
  }
  el.setAttribute("style", "max-width:100%");
  return el.outerHTML;
}

function logoImg(size: number): string {
  return `<img data-receipt-logo="" src="${escapeHtml(logoUrl)}" width="${size}" height="${size}" alt="cc-router" />`;
}

/** 把模板渲染结果里的 <ccr-*> 换成真实内容 (属性已在安装时校验为范围内的整数) */
function expandCcr(html: string): string {
  return html.replace(/<ccr-(qr|barcode|logo)\b([^>]*?)\/?>(?:\s*<\/ccr-\1>)?/g, (_m, kind: string, attrs: string) => {
    const num = (name: string, dflt: number) => {
      const m = new RegExp(`${name}\\s*=\\s*["']?(\\d+)`).exec(attrs);
      return m ? Number(m[1]) : dflt;
    };
    if (kind === "qr") return qrSvg(num("size", 80));
    if (kind === "barcode") return barcodeSvg(num("height", 36));
    return logoImg(num("size", 32));
  });
}

/**
 * 第三方小票: 模板 + 作用域样式。外壳 `position: relative; isolation: isolate; overflow: hidden`,
 * 小票里的定位元素出不了这个框。<style> 放在外壳里, 导出 HTML (outerHTML) 时一起带走。
 */
export const PluginSlip = forwardRef<
  HTMLDivElement,
  { pluginId: string; receipt: CompiledReceipt; css: string; dto: ReceiptDto; ctx: ReceiptContext }
>(function PluginSlip({ pluginId, receipt, css, dto, ctx }, ref) {
  const html = useMemo(() => expandCcr(renderTemplate(receipt.nodes, receiptData(dto, ctx))), [receipt, dto, ctx]);
  return (
    <div
      ref={ref}
      className={receiptScope(pluginId)}
      style={{ width: receipt.width, position: "relative", isolation: "isolate", overflow: "hidden" }}
    >
      <style>{css}</style>
      <div dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
});
