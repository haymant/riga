/**
 * OpenCode's built-in `build` agent allows every tool by default, so it runs
 * shell commands and file edits without ever asking. RIGA promises an approval
 * gate for exactly those mutations, so when the adapter owns the OpenCode server
 * it asks for them by default and forwards each ask to the caller.
 *
 * A caller-provided `OPENCODE_CONFIG_CONTENT` always wins, so anyone with a
 * stricter or looser policy keeps it. When the adapter attaches to an existing
 * server (`OPENCODE_BASE_URL`) it cannot inject this and honours that server's
 * own permission rules.
 */
export const DEFAULT_OPENCODE_CONFIG = JSON.stringify({
  permissions: [
    { action: "shell", resource: "*", effect: "ask" },
    { action: "edit", resource: "*", effect: "ask" },
  ],
});

export function opencodeConfigContent(configured: string | undefined): string {
  return configured ?? DEFAULT_OPENCODE_CONFIG;
}
