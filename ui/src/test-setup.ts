// test-setup.ts - adds the jest-dom matchers (toBeInTheDocument, toBeDisabled, ...) to Vitest.
import '@testing-library/jest-dom/vitest';

// jsdom has no ResizeObserver (Svelte's bind:clientHeight uses it); a do-nothing stand-in is enough.
globalThis.ResizeObserver ??= class {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
} as unknown as typeof ResizeObserver;
