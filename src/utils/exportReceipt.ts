import { toPng } from "html-to-image";
import jsPDF from "jspdf";
import type { ReceiptTheme } from "@/components/receipts/ReceiptSlip";
// ?inline: 拿到构建后的 css 文本 (双 CDN @font-face), 不注入页面
import receiptFontsCss from "@/receipt-fonts.css?inline";

const FILE_PREFIX = "cc-router-receipt";

/** 各主题实际使用的 webfont family; 经典主题走系统字体不需要内嵌 */
const THEME_FONT_FAMILIES: Record<ReceiptTheme, string[]> = {
  mono: [],
  color: [],
  jp_konbini: ["DotGothic16"],
  us_grocery: ["Courier Prime"],
  de_discount: ["IBM Plex Mono"],
  fr_market: ["Space Mono", "Cormorant Garamond"],
  pharmacy: ["Archivo", "IBM Plex Mono"],
  diner_check: ["Caveat", "Oswald"],
  car_label: ["Oswald", "Archivo Narrow"],
};

/**
 * 从 receipt-fonts.css 里挑出当前主题用到的 @font-face 块。
 * 小票本体是纯 inline style, 唯独 @font-face 无法内联到元素上——导出的 HTML
 * 必须自带这些规则, 否则主题字体整体回退系统字体。字体文件仍指公共 CDN
 * (导出的 HTML 无 CSP), 打开时有网即加载, 离线回退系统字体。
 */
function fontFaceCssFor(theme: ReceiptTheme): string {
  const families = THEME_FONT_FAMILIES[theme] ?? [];
  if (families.length === 0) return "";
  const blocks: string[] = [];
  const parts = receiptFontsCss.split("@font-face");
  for (const part of parts.slice(1)) {
    const end = part.indexOf("}");
    if (end < 0) continue;
    const block = "@font-face" + part.slice(0, end + 1);
    if (families.some((f) => block.includes(`font-family: '${f}'`) || block.includes(`font-family: "${f}"`) || block.includes(`font-family:${f}`))) {
      blocks.push(block);
    }
  }
  return blocks.join("\n");
}

/** 内联失败时的兜底 logo (公网)。app 内的 logo 是打包资源路径, 独立打开的 HTML 里解析不到会空白。 */
const PUBLIC_LOGO_URL = "https://ccrouter.app/assets/icon.png";

/** 把 app 内的 logo 资源取回并转成 base64 data URI, 让导出的 HTML 离线也能显示。 */
async function logoDataUri(src: string): Promise<string | null> {
  try {
    const blob = await (await fetch(src)).blob();
    return await new Promise<string>((resolve, reject) => {
      const fr = new FileReader();
      fr.onload = () => resolve(fr.result as string);
      fr.onerror = () => reject(fr.error);
      fr.readAsDataURL(blob);
    });
  } catch {
    return null;
  }
}

function triggerDownload(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

async function elementToPngDataUrl(el: HTMLElement): Promise<string> {
  const rect = el.getBoundingClientRect();
  return toPng(el, {
    width: rect.width,
    height: rect.height,
    pixelRatio: 2,
    cacheBust: true,
    backgroundColor: "#faf7f0",
  });
}

export async function exportPng(el: HTMLElement, slipNo: string, range: string): Promise<void> {
  const dataUrl = await elementToPngDataUrl(el);
  const res = await fetch(dataUrl);
  const blob = await res.blob();
  triggerDownload(blob, `${FILE_PREFIX}-${range}-${slipNo}.png`);
}

export async function exportPdf(el: HTMLElement, slipNo: string, range: string): Promise<void> {
  const dataUrl = await elementToPngDataUrl(el);
  const rect = el.getBoundingClientRect();

  const PADDING_PT = 24;
  const widthPt = rect.width + PADDING_PT * 2;
  const heightPt = rect.height + PADDING_PT * 2;

  const pdf = new jsPDF({
    unit: "pt",
    format: [widthPt, heightPt],
    orientation: widthPt > heightPt ? "landscape" : "portrait",
  });
  pdf.addImage(dataUrl, "PNG", PADDING_PT, PADDING_PT, rect.width, rect.height);
  pdf.save(`${FILE_PREFIX}-${range}-${slipNo}.pdf`);
}

/** 小票本体全 inline-style, 不依赖外部 CSS, outerHTML 直接复制即可在任何浏览器打开。
 *  唯一例外是 logo <img>: 打包资源路径离开 app 无法解析, 导出前克隆 DOM 把 src
 *  内联成 base64 data URI (离线可看); 取不到时兜底公网 URL。 */
export async function exportHtml(
  el: HTMLElement,
  slipNo: string,
  range: string,
  theme: ReceiptTheme,
): Promise<void> {
  const clone = el.cloneNode(true) as HTMLElement;
  const logo = clone.querySelector("img[data-receipt-logo]");
  if (logo) {
    const inline = await logoDataUri((logo as HTMLImageElement).src);
    logo.setAttribute("src", inline ?? PUBLIC_LOGO_URL);
  }

  const fontCss = fontFaceCssFor(theme);
  const html = `<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8">
<title>cc-router receipt ${slipNo}</title>
<style>body { margin: 0; padding: 32px; background: #f0ece2; display: flex; justify-content: center; }</style>
${fontCss ? `<style>\n${fontCss}\n</style>` : ""}
</head>
<body>
${clone.outerHTML}
</body>
</html>`;

  const blob = new Blob([html], { type: "text/html;charset=utf-8" });
  triggerDownload(blob, `${FILE_PREFIX}-${range}-${slipNo}.html`);
}
