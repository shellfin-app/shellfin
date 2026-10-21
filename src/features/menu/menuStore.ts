import type { ReactNode } from "react";
import { create } from "zustand";

export interface MenuItem {
  id: string;
  /** Already translated. */
  label: string;
  /** Already translated key hint, shown right-aligned. */
  shortcut?: string;
  disabled?: boolean;
  /** Already translated tooltip; for a disabled item it says why. */
  title?: string;
  /** Draw a divider above this item. */
  separatorBefore?: boolean;
  /** A toggle: rendered as a checkable item, the mark shown when true. */
  checked?: boolean;
  /** Decoration before the label (a status dot ...). */
  leading?: ReactNode;
  /** Destructive item (shown in the danger colour). */
  danger?: boolean;
  onSelect(): void;
}

export interface OpenMenu {
  x: number;
  y: number;
  items: MenuItem[];
  /** Puts focus back where it was (runs before `onSelect`, so an action may move focus elsewhere). */
  restoreFocus(): void;
}

interface MenuStore {
  menu: OpenMenu | null;
  open(menu: OpenMenu): void;
  close(): void;
}

/** The single open context menu (own tiny store: items hold functions, which do not belong in the app store). */
export const useMenuStore = create<MenuStore>((set) => ({
  menu: null,
  open: (menu) => set({ menu }),
  close: () => set({ menu: null }),
}));
