import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';

/**
 * Global test setup.
 *
 * Everything here exists because the app talks to a Tauri host that does not
 * exist under jsdom. Without these shims a component that merely renders — let
 * alone one that calls a command — throws before any assertion runs.
 */

afterEach(cleanup);

// `invoke` is the single door to the backend (see src/ipc/index.ts). Mocked at
// the module level so tests can drive it with `mockInvoke` below rather than
// each one re-mocking the transport.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => undefined),
}));

// Tauri's event bus. Components that subscribe to backup/restore progress call
// `listen` on mount and await the returned unlisten function on unmount.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

// The dialog plugin opens native file pickers, which jsdom has no notion of.
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
  message: vi.fn(async () => {}),
  confirm: vi.fn(async () => true),
}));

vi.mock('@tauri-apps/plugin-updater', () => ({
  check: vi.fn(async () => null),
}));

vi.mock('@tauri-apps/plugin-process', () => ({
  relaunch: vi.fn(async () => {}),
}));

// jsdom does not implement <dialog>'s methods, and `Modal` deliberately uses
// the native element so focus trapping and Escape come from the platform. The
// shim keeps `open` in sync and fires `cancel` on Escape, which is the part the
// component's behaviour actually depends on.
if (!HTMLDialogElement.prototype.showModal) {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.open = true;
  };
  HTMLDialogElement.prototype.show = function show(this: HTMLDialogElement) {
    this.open = true;
  };
  HTMLDialogElement.prototype.close = function close(
    this: HTMLDialogElement,
    returnValue?: string,
  ) {
    this.open = false;
    if (returnValue !== undefined) this.returnValue = returnValue;
    this.dispatchEvent(new Event('close'));
  };
}

// jsdom implements no scrolling at all. The command palette and the grid both
// call this to keep the active row visible.
Element.prototype.scrollIntoView ??= () => {};

// jsdom implements neither of these, and CodeMirror and the virtualized grid
// both reach for them during layout.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

if (!window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
}

// The grid measures its scroll container to decide how many rows to render.
// jsdom reports every element as 0×0, which would virtualize down to nothing.
// @tanstack/virtual-core reads offsetWidth/offsetHeight for this (not
// clientWidth/clientHeight), so both pairs are stubbed to be safe.
for (const prop of ['clientHeight', 'offsetHeight'] as const) {
  Object.defineProperty(HTMLElement.prototype, prop, {
    configurable: true,
    get() {
      return 600;
    },
  });
}
for (const prop of ['clientWidth', 'offsetWidth'] as const) {
  Object.defineProperty(HTMLElement.prototype, prop, {
    configurable: true,
    get() {
      return 1000;
    },
  });
}
