import { useEffect, useRef } from "react";
import JsBarcode from "jsbarcode";

interface Props {
  value: string;
  fgColor: string;
  bgColor: string;
  height: number;
}

/**
 * Code128 条形码,渲染为内联 SVG。
 * jsbarcode 只写 SVG attribute 不依赖外部 CSS,与小票「全 inline style、
 * outerHTML 导出可独立打开」的约束兼容,PNG/PDF/HTML 三种导出都能捕获。
 */
export function BarcodeSVG({ value, fgColor, bgColor, height }: Props) {
  const ref = useRef<SVGSVGElement>(null);

  useEffect(() => {
    if (!ref.current) return;
    JsBarcode(ref.current, value, {
      format: "CODE128",
      width: 1,
      height,
      displayValue: false,
      margin: 0,
      lineColor: fgColor,
      background: bgColor,
    });
  }, [value, fgColor, bgColor, height]);

  return <svg ref={ref} style={{ maxWidth: "100%" }} />;
}
