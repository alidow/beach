/**
 * Shared button styles so primary tile actions (Connect, Save, etc.)
 * look consistent across agent and application tiles.
 */
export const TILE_PRIMARY_BUTTON_CLASS =
  'flex h-10 items-center justify-center rounded border border-indigo-500/70 dark:border-indigo-400/60 bg-indigo-600 px-4 text-sm font-semibold text-white shadow-sm transition hover:border-indigo-400 hover:bg-indigo-500 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-300/70 disabled:cursor-not-allowed disabled:opacity-50';

export const TILE_SECONDARY_BUTTON_CLASS =
  'flex h-10 items-center justify-center rounded border border-slate-300 dark:border-white/15 bg-transparent px-4 text-sm font-semibold text-slate-700 dark:text-slate-200 transition hover:border-slate-400 dark:hover:border-white/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-slate-400/50 dark:focus-visible:ring-slate-500/50 disabled:cursor-not-allowed disabled:opacity-50';

export const TILE_DANGER_BUTTON_CLASS =
  'flex h-10 items-center justify-center rounded border border-red-400/70 bg-red-500/5 px-4 text-sm font-semibold text-red-600 dark:border-red-400/50 dark:bg-red-500/10 dark:text-red-100 transition hover:border-red-400 hover:bg-red-500/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-400/60 disabled:cursor-not-allowed disabled:opacity-50';
