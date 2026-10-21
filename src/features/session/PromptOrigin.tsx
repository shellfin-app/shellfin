import { useTranslation } from "react-i18next";
import type { HopRef } from "../../ipc/types";
import { displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";

/**
 * "Session · hop i/N (label)": every prompt says which tab and which hop it belongs to, so a prompt
 * can never be mistaken for the session the user is working in.
 */
export function PromptOrigin({ sessionId, hop }: { sessionId: string; hop: HopRef }) {
  const { t } = useTranslation();
  const entry = useStore((s) => s.sessionsById[sessionId]);
  const total = entry?.info?.hops.length;
  const number = hop.index + 1;
  const session = displayServerText(entry?.label ?? "", 256);
  // A direct connection has no hops worth naming: the single hop IS the session's host.
  const direct = hop.index === 0 && (total === undefined || total === 1) && hop.label === entry?.label;
  if (direct) {
    return (
      <p className="prompt-origin" data-testid="prompt-origin">
        {t("prompt.originDirect")}
        <bdi className="prompt-origin__chip">{session}</bdi>
      </p>
    );
  }
  return (
    <p className="prompt-origin" data-testid="prompt-origin">
      {t("prompt.origin", {
        session,
        hop: total ? `${number}/${total}` : String(number),
        label: displayServerText(hop.label, 256),
      })}
    </p>
  );
}
