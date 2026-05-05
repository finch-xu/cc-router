import { toPng } from "html-to-image";
import jsPDF from "jspdf";

const FILE_PREFIX = "cc-router-receipt";

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

/** 小票本体全 inline-style, 不依赖外部 CSS, outerHTML 直接复制即可在任何浏览器打开。 */
export function exportHtml(el: HTMLElement, slipNo: string, range: string): void {
  const html = `<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8">
<title>cc-router receipt ${slipNo}</title>
<style>body { margin: 0; padding: 32px; background: #f0ece2; display: flex; justify-content: center; }</style>
</head>
<body>
${el.outerHTML}
</body>
</html>`;

  const blob = new Blob([html], { type: "text/html;charset=utf-8" });
  triggerDownload(blob, `${FILE_PREFIX}-${range}-${slipNo}.html`);
}
