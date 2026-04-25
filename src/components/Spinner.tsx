import { Loader2 } from "lucide-react";

interface Props {
  size?: number;
  className?: string;
}

export function Spinner({ size = 12, className }: Props) {
  return <Loader2 size={size} className={"spin" + (className ? " " + className : "")} />;
}
