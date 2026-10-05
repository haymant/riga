/**
 * Public contract for the assistant surface.
 *
 * Kept out of `index.ts` so the component can import it without the package
 * re-exporting the component it is currently evaluating: `AssistantUI.tsx` and
 * `index.ts` would otherwise form an import cycle, and the defaults below are
 * read at module-evaluation time.
 */

export const RIGA_ASSISTANT_UI_VERSION = "0.1.0" as const;

export type AssistantUiOptions = {
  /** Show the compact session-history button in the chat header. */
  showSessionHistoryButton?: boolean;
  /** Let the chat surface expand to the full width of its parent container. */
  fullWidth?: boolean;
};

export const DEFAULT_ASSISTANT_UI_OPTIONS = {
  showSessionHistoryButton: true,
  fullWidth: false,
} as const;
