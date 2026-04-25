import { ProviderIcon } from "./ProviderIcon";

interface Props {
  iconId?: string | null;
  /** 外层方块尺寸(px) */
  size?: number;
  /** 内部图标尺寸,默认按 outer-8 估算 */
  iconSize?: number;
  className?: string;
}

/** 把 ProviderIcon 包进 .logo 圆角小方块,用于表格/列表行的统一外观。 */
export function ProviderLogo({ iconId, size = 22, iconSize, className }: Props) {
  const inner = iconSize ?? Math.max(10, size - 8);
  return (
    <span
      className={"logo" + (className ? " " + className : "")}
      style={{ width: size, height: size }}
    >
      <ProviderIcon iconId={iconId} size={inner} />
    </span>
  );
}
