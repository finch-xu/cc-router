import { LoaderCircle } from "lucide-react";

interface Props {
  size?: number;
  className?: string;
}

export function Spinner({ size = 12, className }: Props) {
  return <LoaderCircle size={size} className={"spin" + (className ? " " + className : "")} />;
}
