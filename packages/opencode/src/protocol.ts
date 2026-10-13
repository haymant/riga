export type OpenCodeModel = {
  id: string;
  name: string;
  status?: string;
};

export type OpenCodeProvider = {
  id: string;
  name: string;
  models: Record<string, OpenCodeModel>;
};

/** Narrow structural view of the official SDK, injectable for isolated tests. */
export type OpenCodeSdkResult<T> = T | {
  data?: T;
  error?: unknown;
  request?: unknown;
  response?: unknown;
};

/**
 * Incremental progress emitted while an OpenCode turn runs. The Responses
 * adapter forwards these to its caller so a long tool-using turn is visible
 * instead of arriving as one silent, buffered blob.
 */
export type OpenCodeStreamEvent =
  | { type: "text"; delta: string }
  | { type: "reasoning"; delta: string }
  | { type: "tool_started"; callId: string; tool: string; input: unknown }
  | { type: "tool_finished"; callId: string; tool: string; ok: boolean; output: string };

/** A human-in-the-loop request OpenCode raised and is waiting on. */
export type OpenCodeApprovalRequest =
  | { kind: "permission"; requestId: string; action: string; resources: string[] }
  | { kind: "question"; requestId: string; questions: unknown }
  | { kind: "form"; requestId: string; form: unknown };

/** The caller's decision for a permission request or a question. */
export type OpenCodeApprovalReply = "once" | "always" | "reject";

export type OpenCodePromptArgs = {
  path: { id: string };
  body: {
    model: { providerID: string; modelID: string };
    parts: Array<{ type: "text"; text: string }>;
    system?: string;
  };
  signal?: AbortSignal;
  /** Called as stream events arrive; awaited so ordering is preserved. */
  onEvent?(event: OpenCodeStreamEvent): void | Promise<void>;
  /** Called for a permission/question request; resolves once a client answers. */
  onApproval?(request: OpenCodeApprovalRequest): Promise<OpenCodeApprovalReply>;
};

export type OpenCodeClient = {
  config: {
    providers(): Promise<OpenCodeSdkResult<{ providers: OpenCodeProvider[] }>>;
  };
  session: {
    create(args: { body: { title: string } }): Promise<OpenCodeSdkResult<{ id: string }>>;
    prompt(args: OpenCodePromptArgs): Promise<OpenCodeSdkResult<unknown>>;
  };
};

export type OpenAIModel = {
  id: string;
  object: "model";
  created: number;
  owned_by: string;
  name: string;
};

export type OpenAIResponse = {
  id: string;
  object: "response";
  created_at: number;
  status: "completed";
  model: string;
  conversation: { id: string };
  output: Array<{
    id: string;
    type: "message";
    status: "completed";
    role: "assistant";
    content: Array<{
      type: "output_text";
      text: string;
      annotations: [];
    }>;
  }>;
  output_text: string;
  usage: {
    input_tokens: number;
    input_tokens_details: { cached_tokens: number };
    output_tokens: number;
    output_tokens_details: { reasoning_tokens: number };
    total_tokens: number;
  };
};

export type OpenAIError = {
  error: {
    message: string;
    type: "invalid_request_error" | "authentication_error" | "server_error";
    param?: string;
    code?: string;
  };
};
