import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { IconButton } from "../../components/IconButton";
import { useStore } from "../../state/store";
import { loadedTerminalRegistry } from "./terminalLoader";

/** Find bar over the active terminal (search addon). Enter / Shift+Enter step through matches, Escape closes. */
export function TerminalFindBar({ termId }: { termId: string }) {
  const { t } = useTranslation();
  const setTerminalSearch = useStore((s) => s.setTerminalSearch);
  const inputRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [regex, setRegex] = useState(false);
  const [found, setFound] = useState<boolean | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  const run = (direction: "next" | "previous", incremental = false, q = query, opts = { caseSensitive, regex }) => {
    const registry = loadedTerminalRegistry();
    if (!registry || q === "") {
      registry?.clearFind(termId);
      setFound(null);
      return;
    }
    setFound(registry.findInTerminal(termId, q, opts, direction, incremental));
  };

  const close = () => {
    const registry = loadedTerminalRegistry();
    registry?.clearFind(termId);
    setTerminalSearch(null);
    registry?.focusTerminal(termId);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(e.shiftKey ? "previous" : "next");
    }
  };

  return (
    <div className="terminal-find" role="search" aria-label={t("terminal.find")}>
      <input
        ref={inputRef}
        className="input terminal-find__input"
        value={query}
        placeholder={t("terminal.findPlaceholder")}
        aria-label={t("terminal.find")}
        aria-invalid={found === false}
        onChange={(e) => {
          setQuery(e.target.value);
          run("next", true, e.target.value);
        }}
        onKeyDown={onKeyDown}
      />
      <button
        type="button"
        className={`terminal-find__toggle${caseSensitive ? " terminal-find__toggle--on" : ""}`}
        aria-label={t("terminal.findCase")}
        title={t("terminal.findCase")}
        aria-pressed={caseSensitive}
        onClick={() => {
          setCaseSensitive(!caseSensitive);
          run("next", false, query, { caseSensitive: !caseSensitive, regex });
        }}
      >
        {t("terminal.findCaseShort")}
      </button>
      <button
        type="button"
        className={`terminal-find__toggle${regex ? " terminal-find__toggle--on" : ""}`}
        aria-label={t("terminal.findRegex")}
        title={t("terminal.findRegex")}
        aria-pressed={regex}
        onClick={() => {
          setRegex(!regex);
          run("next", false, query, { caseSensitive, regex: !regex });
        }}
      >
        {t("terminal.findRegexShort")}
      </button>
      <IconButton icon="chevronDown" iconSize={13} size={26} label={t("terminal.findNext")} onClick={() => run("next")} />
      <IconButton
        icon="chevronDown"
        iconSize={13}
        size={26}
        label={t("terminal.findPrevious")}
        className="terminal-find__up"
        onClick={() => run("previous")}
      />
      <span className="terminal-find__status" role="status">
        {found === false ? t("terminal.findNone") : ""}
      </span>
      <IconButton icon="close" iconSize={12} size={26} label={t("common.close")} onClick={close} />
    </div>
  );
}
