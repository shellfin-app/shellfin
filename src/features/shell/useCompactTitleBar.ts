import { useEffect, useState } from "react";
import { isCompactTitleBar } from "./layoutLimits";

/** Whether the window is narrow enough for the compact title bar; follows window resizes (never focus changes). */
export function useCompactTitleBar(): boolean {
  const [compact, setCompact] = useState(() => isCompactTitleBar(window.innerWidth));
  useEffect(() => {
    const onResize = () => setCompact(isCompactTitleBar(window.innerWidth));
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return compact;
}
