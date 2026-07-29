import { toPng } from "html-to-image";
import jsPDF from "jspdf";

const FILE_PREFIX = "cc-router-receipt";

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
export async function exportHtml(el: HTMLElement, slipNo: string, range: string): Promise<void> {
  const clone = el.cloneNode(true) as HTMLElement;
  const logo = clone.querySelector("img[data-receipt-logo]");
  if (logo) {
    const inline = await logoDataUri((logo as HTMLImageElement).src);
    logo.setAttribute("src", inline ?? PUBLIC_LOGO_URL);
  }

  const html = `<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8">
<title>cc-router receipt ${slipNo}</title>
<style>body { margin: 0; padding: 32px; background: #f0ece2; display: flex; justify-content: center; }</style>
</head>
<body>
${clone.outerHTML}
</body>
</html>`;

  const blob = new Blob([html], { type: "text/html;charset=utf-8" });
  triggerDownload(blob, `${FILE_PREFIX}-${range}-${slipNo}.html`);
}
