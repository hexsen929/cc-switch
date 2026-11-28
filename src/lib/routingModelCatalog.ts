import type { AppId } from "@/lib/api";
import type { Provider } from "@/types";
import { claudePresetModelsUrl } from "@/config/claudeProviderPresets";
import {
  claudeModelFetchApiFormat,
  fetchCodexOauthModels,
  fetchModelsForConfig,
  fetchXaiOauthModels,
  modelFetchRequestHeaders,
  type FetchedModel,
} from "@/lib/api/model-fetch";
import {
  copilotGetModels,
  copilotGetModelsForAccount,
} from "@/lib/api/copilot";
import {
  copilotFetchedModels,
  isCopilotModelSupportedByCodex,
} from "@/lib/copilot-model-options";
import { resolveManagedAccountId } from "@/lib/authBinding";
import {
  extractCodexBaseUrl,
  extractCodexModelName,
  pickCodexApiKey,
} from "@/utils/codexConfigUtils";
import { parseGrokBuildConfig } from "@/utils/grokBuildConfig";
import { getApiKeyFromConfig } from "@/utils/providerConfigUtils";
import { resolveCodexOfficialIdentity } from "@/utils/providerCapabilities";

const text = (v: unknown) => (typeof v === "string" ? v.trim() : "");

export function configuredRouteModels(app: AppId, p: Provider): FetchedModel[] {
  const cfg = p.settingsConfig ?? {};
  const toml = text(cfg.config);
  let names: unknown[] = [];
  if (app === "codex") {
    const rows: unknown[] = Array.isArray(cfg.modelCatalog?.models)
      ? cfg.modelCatalog.models
      : [];
    names = [
      extractCodexModelName(toml),
      ...rows.map((row) =>
        row && typeof row === "object" && "model" in row ? row.model : "",
      ),
    ];
  } else if (app === "claude") {
    names = [
      ...(p.meta?.stackModels ?? []).map((r) => r.model),
      ...[
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
      ].map((k) => text(cfg.env?.[k]).replace(/\[1m\]$/i, "")),
    ];
  } else if (app === "gemini") {
    names = [cfg.env?.GEMINI_MODEL];
  } else if (app === "grokbuild" && toml) {
    names = [parseGrokBuildConfig(toml).upstreamModel];
  }
  return [...new Set(names.map(text).filter(Boolean))].map((id) => ({
    id,
    ownedBy: null,
  }));
}

export class CatalogConfigError extends Error {
  constructor(readonly messageKey: string) {
    super(messageKey);
  }
}

function claudeCopilotHost(baseUrl: string): boolean {
  try {
    const host = new URL(baseUrl).hostname;
    return host === "githubcopilot.com" || host.endsWith(".githubcopilot.com");
  } catch {
    return false;
  }
}

/** Read the saved card just as the corresponding provider form would. Only an
 * explicit user action may call this; selecting a card never contacts it. */
export async function fetchRoutingModels(
  app: AppId,
  p: Provider,
): Promise<FetchedModel[]> {
  const { meta } = p;
  const cfg = p.settingsConfig ?? {};
  const kind = meta?.providerType;
  const account = (type: string) => resolveManagedAccountId(meta, type);
  const official = resolveCodexOfficialIdentity(app, p);
  // The unbound native login may belong to a different user than the managed default.
  if (official === "native_login")
    throw new CatalogConfigError("providerForm.fetchModelsNotSupported");
  const claudeBase = text(cfg.env?.ANTHROPIC_BASE_URL);
  if (
    kind === "github_copilot" ||
    (app === "claude" && claudeCopilotHost(claudeBase))
  ) {
    const id = account("github_copilot");
    const options = id
      ? await copilotGetModelsForAccount(id)
      : await copilotGetModels();
    const format = meta?.codexCopilotApiFormat;
    const models =
      app === "codex"
        ? options.models.filter((m) =>
            isCopilotModelSupportedByCodex(
              m,
              format === "openai_chat" || format === "openai_responses"
                ? format
                : "auto",
            ),
          )
        : options.models;
    return copilotFetchedModels({ ...options, models }, "GitHub Copilot");
  }
  if (kind === "xai_oauth") return fetchXaiOauthModels(account("xai_oauth"));
  if (kind === "codex_oauth" || official === "managed_account") {
    return fetchCodexOauthModels(account("codex_oauth"));
  }
  const toml = text(cfg.config);
  let baseUrl: string;
  let apiKey: string;
  switch (app) {
    case "codex":
      baseUrl =
        extractCodexBaseUrl(toml) ||
        text(cfg.base_url ?? cfg.baseUrl) ||
        (official === "api_key" ? "https://api.openai.com/v1" : "");
      apiKey = pickCodexApiKey(cfg.auth, toml);
      break;
    case "claude":
      baseUrl = claudeBase;
      apiKey = getApiKeyFromConfig(JSON.stringify(cfg), app);
      break;
    case "gemini":
      baseUrl = text(cfg.env?.GOOGLE_GEMINI_BASE_URL);
      apiKey = text(cfg.env?.GEMINI_API_KEY);
      if (!baseUrl && p.category === "official" && apiKey)
        baseUrl = "https://generativelanguage.googleapis.com";
      break;
    case "grokbuild": {
      const grok = parseGrokBuildConfig(toml);
      baseUrl = grok.baseUrl;
      apiKey = grok.apiKey;
      break;
    }
    default:
      throw new CatalogConfigError("providerForm.fetchModelsNotSupported");
  }
  const requestHeaders = modelFetchRequestHeaders(
    JSON.stringify(meta?.localProxyRequestOverrides?.headers ?? {}),
  );
  if (!text(apiKey) || text(apiKey) === "PROXY_MANAGED") apiKey = "";
  if (!baseUrl && !apiKey && !requestHeaders)
    throw new CatalogConfigError("providerForm.fetchModelsNeedConfig");
  if (!baseUrl)
    throw new CatalogConfigError("providerForm.fetchModelsNeedEndpoint");
  if (!apiKey && !requestHeaders)
    throw new CatalogConfigError("providerForm.fetchModelsNeedApiKey");
  return fetchModelsForConfig(
    baseUrl,
    apiKey,
    meta?.isFullUrl,
    app === "claude" ? claudePresetModelsUrl(baseUrl) : undefined,
    meta?.customUserAgent,
    {
      apiFormat:
        app === "gemini"
          ? "google-generative-ai"
          : app === "claude"
            ? claudeModelFetchApiFormat(meta?.apiFormat)
            : undefined,
      requestHeaders,
    },
  );
}
