import { useSyncExternalStore } from "react";

/** Follows the `dark` class `lib/theme.ts` toggles on `<html>`: the OpenUI `ThemeProvider` mode for every app mount. */
export function useDark(): boolean {
  return useSyncExternalStore(
    (cb) => {
      const o = new MutationObserver(cb);
      o.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["class"],
      });
      return () => o.disconnect();
    },
    () => document.documentElement.classList.contains("dark"),
  );
}
