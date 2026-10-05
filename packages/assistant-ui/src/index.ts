/** Public package marker; runtime and components begin in Phase 4. */
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
