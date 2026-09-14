/**
 * Regression test for a dialog opened from a dropdown menu leaving the whole window
 * unclickable after it closes.
 *
 * Radix's `DismissableLayer` sets `pointer-events: none` on `<body>` while a modal layer is
 * open, and restores the value it snapshotted when the last layer of its own copy unmounts.
 * That snapshot is a module-level variable, so it is only correct while one copy of
 * `@radix-ui/react-dismissable-layer` is loaded. Two are: `@radix-ui/react-dialog` resolves
 * 1.1.19 under `ui/desktop`, while the dropdown menu that arrives with `@radix-ui/themes`
 * resolves 1.1.11, hoisted into `ui/node_modules`.
 *
 * A dropdown menu's close animation keeps its layer mounted for a beat after the item that
 * opened the dialog was selected, so the dialog mounts while the menu's `none` is still on
 * the body, snapshots it as the "original", and writes it back on close. Every click in the
 * window is then dead — the sidebar, the session title, the header links.
 *
 * The menu half of that is stood in for here (set the lock, later drop it) rather than
 * rendered: two modal Radix layers mounted at once put jsdom's focus scopes into an
 * infinite focus loop, and the menu's contribution is only ever "set none, restore what was
 * there before". The dialog half is the real component and the real 1.1.19 copy.
 */
import { describe, expect, it, beforeEach, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/react';

import * as DialogPrimitive from '@radix-ui/react-dialog';

import {
  Dialog,
  DialogContent,
  DialogOverlay,
  DialogPointerEventsRecovery,
  DialogPortal,
  DialogTitle,
} from './dialog';
import { IntlTestWrapper } from '../../i18n/test-utils';

function JsonDialog({ open }: { open: boolean }) {
  return (
    <IntlTestWrapper>
      <Dialog open={open}>
        <DialogContent>
          <DialogTitle>Session JSON</DialogTitle>
        </DialogContent>
      </Dialog>
    </IntlTestWrapper>
  );
}

/**
 * Overlay and content have separate presence lifecycles, and the content's exit animation
 * (`duration-200`) outlasts the overlay's (150ms by default). `forceMount` on the portal and
 * the content stands in for that here: closing the dialog unmounts only the overlay, and the
 * content is taken down in a later step, the way the real exit animations order it.
 */
function SplitExitDialog({ open, contentMounted }: { open: boolean; contentMounted: boolean }) {
  return (
    <IntlTestWrapper>
      <Dialog open={open}>
        <DialogPortal forceMount>
          <DialogOverlay />
          {contentMounted && (
            <DialogPrimitive.Content forceMount>
              <DialogPointerEventsRecovery />
              <DialogTitle>Session JSON</DialogTitle>
            </DialogPrimitive.Content>
          )}
        </DialogPortal>
      </Dialog>
    </IntlTestWrapper>
  );
}

async function nextFrames(): Promise<void> {
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
}

describe('a dialog opened while a dropdown menu still holds the body lock', () => {
  beforeEach(() => {
    document.body.style.removeProperty('pointer-events');
  });

  afterEach(() => {
    cleanup();
    document.body.style.removeProperty('pointer-events');
  });

  it('leaves the body clickable after the dialog closes', async () => {
    // The menu that was clicked is still animating out, so its copy of the layer still
    // holds the lock.
    document.body.style.pointerEvents = 'none';

    const { rerender } = render(<JsonDialog open />);

    // The menu finishes closing and its own copy restores what it found: an unlocked body.
    document.body.style.removeProperty('pointer-events');

    rerender(<JsonDialog open={false} />);
    await nextFrames();

    expect(document.body.style.pointerEvents).not.toBe('none');
  });

  it('still locks the body while the dialog is open', () => {
    render(<JsonDialog open />);

    expect(document.body.style.pointerEvents).toBe('none');
  });

  it('leaves the body clickable after a dialog opened on its own closes', async () => {
    const { rerender } = render(<JsonDialog open />);
    expect(document.body.style.pointerEvents).toBe('none');

    rerender(<JsonDialog open={false} />);
    await nextFrames();

    expect(document.body.style.pointerEvents).not.toBe('none');
  });

  it('leaves the body clickable when the content outlives the overlay', async () => {
    document.body.style.pointerEvents = 'none';

    const { rerender } = render(<SplitExitDialog open contentMounted />);
    document.body.style.removeProperty('pointer-events');

    // The overlay's exit animation ends first: only the overlay unmounts.
    rerender(<SplitExitDialog open={false} contentMounted />);
    await nextFrames();

    // Then the content's does, and its layer restores the stale `none` it snapshotted.
    rerender(<SplitExitDialog open={false} contentMounted={false} />);
    await nextFrames();

    expect(document.body.style.pointerEvents).not.toBe('none');
  });
});
