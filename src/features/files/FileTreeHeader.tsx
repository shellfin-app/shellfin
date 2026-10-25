import { useCallback, useLayoutEffect, useRef, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "../../components/Icon";
import type { FilesSortKey } from "../../state/layoutPrefs";
import { useStore } from "../../state/store";
import { chooseFilesSort } from "../actions/registry";

const KEYS: readonly FilesSortKey[] = ["name", "size", "mtime"];

/** Rendered (not `display: none` by a container query): a hidden column's button must not be a Tab stop. */
function shown(button: HTMLButtonElement): boolean {
  return button.getClientRects().length > 0;
}

function present(list: readonly (HTMLButtonElement | null)[]): HTMLButtonElement[] {
  return list.filter((b): b is HTMLButtonElement => b !== null);
}

/**
 * The column header above the file tree (§8.11): Name / Size / Modified sort buttons. It sits outside
 * `role="tree"` (a tree may only own treeitems) and shows with the columns (container queries in files.css).
 * The sort state is in each button's label (`aria-sort` is only valid on a columnheader); arrows move between them.
 */
export function FileTreeHeader() {
  const { t } = useTranslation();
  const sort = useStore((s) => s.filesSort);
  const rootRef = useRef<HTMLDivElement>(null);
  const refs = useRef<(HTMLButtonElement | null)[]>([]);

  const buttons = () => present(refs.current);
  const visibleButtons = () => buttons().filter(shown);

  // Roving focus over the buttons that are shown at this width: the active sort is the Tab stop, or the first shown
  // button when its column is hidden. Without layout information (nothing reports a box) the active one is used.
  const activeIndex = KEYS.indexOf(sort.key);
  const updateTabStops = useCallback(() => {
    const all = present(refs.current);
    const visible = all.filter(shown);
    const active = refs.current[activeIndex] ?? undefined;
    const stop = visible.length === 0 ? active : active && visible.includes(active) ? active : visible[0];
    for (const b of all) b.tabIndex = b === stop ? 0 : -1;
  }, [activeIndex]);

  // After every render (the props never change `tabIndex`, so React leaves the values set here alone).
  useLayoutEffect(() => updateTabStops());
  useLayoutEffect(() => {
    const el = rootRef.current;
    // The cells appear and disappear with the sidebar width, which also resizes the header.
    if (!el || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(() => updateTabStops());
    observer.observe(el);
    return () => observer.disconnect();
  }, [updateTabStops]);

  const column = (key: FilesSortKey): string =>
    key === "name" ? t("files.column.name") : key === "size" ? t("files.column.size") : t("files.column.modified");

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const visible = visibleButtons();
    const list = visible.length > 0 ? visible : buttons();
    const index = list.findIndex((b) => b === e.target);
    if (index < 0) return;
    let next = -1;
    if (e.key === "ArrowRight") next = (index + 1) % list.length;
    else if (e.key === "ArrowLeft") next = (index - 1 + list.length) % list.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = list.length - 1;
    if (next < 0) return;
    e.preventDefault();
    list[next]?.focus();
  };

  return (
    <div ref={rootRef} className="file-tree-header" role="toolbar" aria-label={t("files.sort.toolbar")} onKeyDown={onKeyDown}>
      {KEYS.map((key, i) => {
        const active = sort.key === key;
        // The label carries the state ("Size, sorted descending"); no aria-pressed on top of it.
        const label = active
          ? sort.dir === "asc"
            ? t("files.sort.activeAsc", { column: column(key) })
            : t("files.sort.activeDesc", { column: column(key) })
          : key === "name"
            ? t("files.sort.byName")
            : key === "size"
              ? t("files.sort.bySize")
              : t("files.sort.byModified");
        return (
          <div key={key} className={`file-tree-header__cell file-tree-header__cell--${key}`}>
            <button
              ref={(el) => {
                refs.current[i] = el;
              }}
              type="button"
              className={`file-tree-header__button${active ? " file-tree-header__button--active" : ""}`}
              aria-label={label}
              title={label}
              // The Tab stops are set by `updateTabStops` (they depend on which columns are shown).
              tabIndex={-1}
              onClick={() => chooseFilesSort(key)}
            >
              <span className="file-tree-header__text">{column(key)}</span>
              {key === "name" && !active ? (
                // Sorting by a column that is hidden at this width stays visible next to "Name" (files.css).
                <span className={`file-tree-header__hint file-tree-header__hint--${sort.key}`} aria-hidden="true">
                  {column(sort.key)}
                  <Icon name={sort.dir === "asc" ? "chevronUp" : "chevronDown"} size={10} />
                </span>
              ) : null}
              {active ? <Icon name={sort.dir === "asc" ? "chevronUp" : "chevronDown"} size={10} className="file-tree-header__arrow" /> : null}
            </button>
          </div>
        );
      })}
    </div>
  );
}
