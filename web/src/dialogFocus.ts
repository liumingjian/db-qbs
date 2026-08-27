import { useEffect, useRef } from "react";
import type { RefObject } from "react";

/**
 * Focus management for anything that overlays the page: the modal, the drawer.
 *
 * There was none. Both said `role="dialog"` and `aria-modal="true"`, and both
 * installed an Escape handler — but focus stayed wherever it was, Tab walked
 * straight out into the page behind the scrim, and closing left focus on
 * `<body>`. `aria-modal` is a claim about behaviour; without a trap it is a
 * false one, and a screen reader is told to ignore the page while the keyboard
 * is still free to roam it. This is the one shared implementation, so the two
 * overlays cannot drift apart.
 *
 * Four things, in order:
 *
 * 1. **Initial focus** on the first focusable element inside, or the container
 *    itself when there is none. Not on the primary action: in the confirmation
 *    dialogs that action deletes production rows, and Enter must not be one
 *    keystroke away from it. The close button coming first is the safe default.
 * 2. **Tab and Shift-Tab wrap** inside the container.
 * 3. **Escape closes**, unless the caller says it may not (a save in flight).
 * 4. **Focus returns to the opener** on unmount — otherwise the next Tab starts
 *    from the top of the document, nowhere near the row that was being worked on.
 *
 * Only the **topmost** overlay handles keys. Nesting is real — the cleanup
 * confirmation opens on top of the run drawer — and without the stack a single
 * Escape would close both, which reads as the dialog cancelling the whole screen.
 */
export function useDialogFocus(
  container: RefObject<HTMLElement | null>,
  {
    onEscape,
    escapable = true,
  }: {
    onEscape: () => void;
    /** `false` while something irreversible is in flight; Escape then does nothing. */
    escapable?: boolean;
  },
): void {
  // Read through a ref so a changing callback never re-runs the effect: doing so
  // would restore focus to the opener and re-grab it on every parent render.
  const latest = useRef({ onEscape, escapable });
  latest.current = { onEscape, escapable };

  useEffect(() => {
    const id = {};
    openDialogs.push(id);
    const opener =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const element = container.current;
    (focusableWithin(element)[0] ?? element)?.focus();

    function handleKeyDown(event: KeyboardEvent) {
      if (openDialogs[openDialogs.length - 1] !== id) {
        return;
      }
      if (event.key === "Escape") {
        if (latest.current.escapable) {
          latest.current.onEscape();
        }
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      const items = focusableWithin(container.current);
      if (items.length === 0) {
        event.preventDefault();
        container.current?.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      const inside =
        active instanceof Node && container.current?.contains(active) === true;
      if (!inside) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
      } else if (event.shiftKey && active === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      const index = openDialogs.indexOf(id);
      if (index >= 0) {
        openDialogs.splice(index, 1);
      }
      opener?.focus();
    };
    // Mount and unmount only. `container` is a ref, stable by construction.
  }, [container]);
}

/** Innermost first. Identity objects, so two overlays can never collide. */
const openDialogs: object[] = [];

/**
 * Whether an overlay currently owns the keyboard.
 *
 * For the one Escape listener that is *not* an overlay: the wizard's own, bound to
 * the wizard container (#242). Everything on this stack renders inside that
 * container, so its keydown bubbles through — and Escape there means "close the
 * topmost layer", never "leave the wizard". Asking the stack is how that listener
 * defers; the alternative is what the fullscreen SQL editor used to do, a
 * capture-phase `stopImmediatePropagation` racing to get in first.
 */
export function overlayOwnsKeyboard(): boolean {
  return openDialogs.length > 0;
}

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

/**
 * Focusable descendants in document order.
 *
 * Recomputed on every Tab rather than cached at mount: these dialogs grow and
 * shrink as they load (the wizard's entry dialog, an error line appearing), and
 * a cached list would trap Tab against elements that are gone.
 *
 * `getClientRects()` is the rendered test — `offsetParent` reports `null` for
 * anything inside a `position: fixed` ancestor, which is every one of these.
 */
function focusableWithin(container: HTMLElement | null): HTMLElement[] {
  if (container === null) {
    return [];
  }
  return Array.from(
    container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
  ).filter((element) => element.getClientRects().length > 0);
}
