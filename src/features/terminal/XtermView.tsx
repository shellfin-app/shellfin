import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { loadTerminalRegistry } from "./terminalLoader";

/**
 * Mounts the registry's xterm host for `termId` into a container (loading the terminal chunk on demand) and
 * parks it again on unmount. A failing chunk load throws during render, so the surrounding ErrorBoundary shows
 * the failure instead of the shell going blank.
 */
export function XtermView({ termId }: { termId: string }) {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement>(null);
  const [failure, setFailure] = useState<unknown>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return undefined;
    let cancelled = false;
    let detach: (() => void) | null = null;
    loadTerminalRegistry().then(
      (registry) => {
        if (!cancelled) detach = registry.attachTerminal(termId, container);
      },
      (e: unknown) => {
        if (!cancelled) setFailure(e);
      },
    );
    return () => {
      cancelled = true;
      detach?.();
    };
  }, [termId]);

  if (failure) throw failure;
  // `aria-label` is ignored on a generic div, so the container is a named group (xterm labels its own textarea).
  return <div ref={containerRef} className="xterm-view" role="group" aria-label={t("terminal.title")} />;
}
