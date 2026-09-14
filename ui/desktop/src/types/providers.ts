export type ProviderType = 'Preferred' | 'Builtin' | 'Declarative' | 'Custom';

export type ThinkingEffort = 'off' | 'low' | 'medium' | 'high' | 'max';

export type ConfigKey = {
  default?: string | null;
  device_code_flow?: boolean;
  name: string;
  oauth_flow: boolean;
  primary?: boolean;
  required: boolean;
  secret: boolean;
};

export type ModelInfo = {
  context_limit?: number | null;
  currency?: string | null;
  input_token_cost?: number | null;
  name: string;
  output_token_cost?: number | null;
  reasoning?: boolean;
  resolved_model?: string | null;
  supports_cache_control?: boolean | null;
};

export type ProviderMetadata = {
  config_keys: ConfigKey[];
  default_model: string;
  description: string;
  display_name: string;
  known_models: ModelInfo[];
  model_doc_link: string;
  name: string;
  setup_steps?: string[];
};

export type ProviderDetails = {
  is_configured: boolean;
  is_available: boolean;
  is_refreshing?: boolean;
  last_refresh_error?: string | null;
  supports_refresh?: boolean;
  visible_in_setup: boolean;
  deprecated: boolean;
  replacement?: string | null;
  metadata: ProviderMetadata;
  name: string;
  provider_type: ProviderType;
  uses_acp: boolean;
  saved_model?: string | null;
};

export type UpdateCustomProviderRequest = {
  api_key: string;
  api_url: string;
  base_path?: string | null;
  catalog_provider_id?: string | null;
  display_name: string;
  engine: string;
  headers?: Record<string, string> | null;
  models: string[];
  preserves_thinking?: boolean | null;
  requires_auth?: boolean;
  supports_streaming?: boolean | null;
  toolshim: boolean;
};
