/**
 * Shared assistant surface for RIGA-compatible clients.
 *
 * The package owns the whole chat UI — transcript, tool timeline, composer,
 * settings and the local-model manager — plus the stylesheet it is built from.
 * A container app imports `AssistantUI`, renders it, and needs nothing else;
 * `styles.css` is re-exported for hosts that want to control when the sheet is
 * loaded instead of letting the component pull it in.
 */

export { AssistantUI, type AssistantUIProps } from "./AssistantUI";
export { Markdown, normalizeFences } from "./Markdown";
export {
  RIGA_ASSISTANT_UI_VERSION,
  DEFAULT_ASSISTANT_UI_OPTIONS,
  type AssistantUiOptions,
} from "./options";
