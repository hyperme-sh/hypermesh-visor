/**
 * Shared instrument classes. Text on signal is fixed at #0E0F0C
 * (never paper --ground). No new brand hex here.
 */
export const onSignal = "text-on-signal";

export const btnPrimary =
  "bg-signal px-3 py-2 text-xs font-semibold text-on-signal disabled:opacity-50";

export const btnSecondary =
  "border border-rule px-3 py-2 text-xs font-semibold text-ink hover:border-signal disabled:opacity-50";

export const btnTabOn =
  "border border-signal bg-signal px-3 py-2 text-xs font-semibold text-on-signal";

export const btnTabOff =
  "border border-rule bg-panel px-3 py-2 text-xs font-semibold text-ink hover:border-signal";

export const chipOn =
  "border border-signal bg-signal px-3 py-1 text-xs font-medium text-on-signal";

export const chipOff =
  "border border-rule bg-panel px-3 py-1 text-xs font-medium text-ink hover:border-signal";

export const field =
  "border border-rule bg-ground px-3 py-2 text-sm text-ink disabled:opacity-50";

export const fieldMono =
  "border border-rule bg-ground px-3 py-2 font-mono text-xs text-ink disabled:opacity-50";

export const panel = "instrument-panel border border-rule bg-panel p-5";

export const failPanel =
  "border border-status-fail bg-[color-mix(in_srgb,var(--status-fail)_12%,var(--ground))] p-6 text-sm text-ink";
