import { cn } from "@/lib/utils";

interface Props {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  className?: string;
  "aria-label"?: string;
}

export function Toggle({ checked, onChange, disabled, className, ...rest }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={rest["aria-label"]}
      className={cn("toggle", checked && "on", className)}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}
