/**
 * Recover from a leftover `pointer-events: none` on `<body>`.
 *
 * Radix's `DismissableLayer` disables pointer events on the body while a modal layer is
 * open, and restores the value it snapshotted when the last layer of that copy unmounts.
 * The snapshot lives in a module-level variable, so it is only correct while a single copy
 * of `@radix-ui/react-dismissable-layer` is loaded. Two are: `@radix-ui/react-dialog`
 * resolves 1.1.19 under `ui/desktop`, and the dropdown menu that arrives with
 * `@radix-ui/themes` resolves 1.1.11, hoisted into `ui/node_modules`. A dialog opened from
 * a menu that is still animating out therefore snapshots the menu's own
 * `none`, and writes that back when the dialog closes - after which the window takes no
 * clicks at all: the sidebar, the session title and the header links all look frozen.
 *
 * Clearing the property is safe only once no modal layer is left, which is what the
 * selector below checks. Any still-open dialog, menu, select or popover keeps its own
 * lock and re-applies it on its own unmount.
 *
 * The check goes by role and open state, not by Radix's `[data-radix-popper-content-wrapper]`.
 * Tooltips ride the same popper wrapper without ever taking the body lock, and a tooltip can
 * still be showing while the dialog closes - its trigger keeps focus after keyboard
 * navigation, say. Nothing reschedules the release when that tooltip later goes away, so
 * letting it block the check would leave the body locked for good.
 */
const OPEN_LAYER_SELECTOR = [
  '[role="dialog"][data-state="open"]',
  '[role="alertdialog"][data-state="open"]',
  '[role="menu"][data-state="open"]',
  '[role="listbox"][data-state="open"]',
].join(', ');

export function releaseStuckBodyPointerEvents(doc: typeof document = document): void {
  if (doc.body.style.pointerEvents !== 'none') {
    return;
  }
  if (doc.querySelector(OPEN_LAYER_SELECTOR)) {
    return;
  }
  doc.body.style.removeProperty('pointer-events');
}

/**
 * Schedule the check for after the next paint, so a dialog that is being replaced by
 * another one - closing one modal to open the next - is not cleared out from under the
 * incoming layer.
 */
export function scheduleBodyPointerEventsRelease(doc: typeof document = document): void {
  const view = doc.defaultView;
  if (view?.requestAnimationFrame) {
    view.requestAnimationFrame(() => releaseStuckBodyPointerEvents(doc));
    return;
  }
  setTimeout(() => releaseStuckBodyPointerEvents(doc), 0);
}
