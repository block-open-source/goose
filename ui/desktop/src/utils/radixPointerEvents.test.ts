import { describe, expect, it, beforeEach, afterEach } from 'vitest';
import {
  releaseStuckBodyPointerEvents,
  scheduleBodyPointerEventsRelease,
} from './radixPointerEvents';

describe('releaseStuckBodyPointerEvents', () => {
  beforeEach(() => {
    document.body.style.removeProperty('pointer-events');
    document.body.innerHTML = '';
  });

  afterEach(() => {
    document.body.style.removeProperty('pointer-events');
    document.body.innerHTML = '';
  });

  it('clears a leftover lock once the last layer is gone', () => {
    document.body.style.pointerEvents = 'none';

    releaseStuckBodyPointerEvents(document);

    expect(document.body.style.pointerEvents).toBe('');
  });

  it('leaves the lock alone while a dialog is still open', () => {
    document.body.style.pointerEvents = 'none';
    document.body.innerHTML = '<div role="dialog" data-state="open"></div>';

    releaseStuckBodyPointerEvents(document);

    expect(document.body.style.pointerEvents).toBe('none');
  });

  it('leaves the lock alone while a popper-backed menu is still open', () => {
    document.body.style.pointerEvents = 'none';
    document.body.innerHTML =
      '<div data-radix-popper-content-wrapper><div role="menu" data-state="open"></div></div>';

    releaseStuckBodyPointerEvents(document);

    expect(document.body.style.pointerEvents).toBe('none');
  });

  it('leaves the lock alone while a select listbox is still open', () => {
    document.body.style.pointerEvents = 'none';
    document.body.innerHTML =
      '<div data-radix-popper-content-wrapper><div role="listbox" data-state="open"></div></div>';

    releaseStuckBodyPointerEvents(document);

    expect(document.body.style.pointerEvents).toBe('none');
  });

  it('is not blocked by a tooltip that is still showing', () => {
    // Tooltips ride the same popper wrapper as menus but never take the body lock, and
    // nothing re-runs the check when they hide.
    document.body.style.pointerEvents = 'none';
    document.body.innerHTML =
      '<div data-radix-popper-content-wrapper>' +
      '<div data-slot="tooltip-content" data-state="delayed-open"><span role="tooltip">Copy</span></div>' +
      '</div>';

    releaseStuckBodyPointerEvents(document);

    expect(document.body.style.pointerEvents).toBe('');
  });

  it('ignores a menu that is animating closed', () => {
    document.body.style.pointerEvents = 'none';
    document.body.innerHTML =
      '<div data-radix-popper-content-wrapper><div role="menu" data-state="closed"></div></div>';

    releaseStuckBodyPointerEvents(document);

    expect(document.body.style.pointerEvents).toBe('');
  });

  it('ignores a closed dialog left in the tree', () => {
    document.body.style.pointerEvents = 'none';
    document.body.innerHTML = '<div role="dialog" data-state="closed"></div>';

    releaseStuckBodyPointerEvents(document);

    expect(document.body.style.pointerEvents).toBe('');
  });

  it('does not touch a body that was never locked', () => {
    document.body.style.pointerEvents = 'auto';

    releaseStuckBodyPointerEvents(document);

    expect(document.body.style.pointerEvents).toBe('auto');
  });

  it('defers the check to the next frame', async () => {
    document.body.style.pointerEvents = 'none';

    scheduleBodyPointerEventsRelease(document);
    expect(document.body.style.pointerEvents).toBe('none');

    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    expect(document.body.style.pointerEvents).toBe('');
  });
});
