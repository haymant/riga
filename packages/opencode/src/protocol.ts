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

export type OpenCodeClient = {
  config: {
    providers(): Promise<OpenCodeSdkResult<{ providers: OpenCodeProvider[] }>>;
  };
  session: {
    create(args: { body: { title: string } }): Promise<OpenCodeSdkResult<{ id: string }>>;
    prompt(args: {
      path: { id: string };
      body: {
        model: { providerID: string; modelID: string };
        parts: Array<{ type: "text"; text: string }>;
        system?: string;
      };
    }): Promise<OpenCodeSdkResult<unknown>>;
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
