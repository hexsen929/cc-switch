import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Provider } from "@/types";
import {
  configuredRouteModels,
  fetchRoutingModels,
} from "@/lib/routingModelCatalog";
import { CODEX_OFFICIAL_PROVIDER_ID } from "@/utils/providerCapabilities";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const card = (
  settingsConfig: Provider["settingsConfig"],
  meta?: Provider["meta"],
): Provider => ({ id: "card", name: "Card", settingsConfig, meta });

describe("saved routing card model discovery", () => {
  beforeEach(() => {
    invoke.mockReset().mockResolvedValue([]);
  });
  it("uses the active Codex TOML table, bearer fallback, full URL and validated request headers", async () => {
    const p = card(
      {
        auth: { OPENAI_API_KEY: "PROXY_MANAGED" },
        config:
          'model_provider="chosen"\nmodel="a"\n[model_providers.other]\nbase_url="https://other.test"\n[model_providers.chosen]\nbase_url="https://chosen.test/v1/responses"\nexperimental_bearer_token="test-bearer"',
        modelCatalog: { models: [{ model: "a" }, { model: "b" }] },
      },
      {
        isFullUrl: true,
        customUserAgent: "test-agent",
        localProxyRequestOverrides: { headers: { "x-region": "east" } },
      },
    );
    expect(configuredRouteModels("codex", p).map((m) => m.id)).toEqual([
      "a",
      "b",
    ]);
    await fetchRoutingModels("codex", p);
    expect(invoke).toHaveBeenCalledWith(
      "fetch_models_for_config",
      expect.objectContaining({
        baseUrl: "https://chosen.test/v1/responses",
        apiKey: "test-bearer",
        isFullUrl: true,
        customUserAgent: "test-agent",
        requestHeaders: { "x-region": "east" },
      }),
    );
  });
  it("uses Claude's saved preset catalog and Gemini's native listing protocol", async () => {
    const p = card({
      env: {
        ANTHROPIC_BASE_URL: "https://api.deepseek.com/anthropic",
        ANTHROPIC_AUTH_TOKEN: "ds-key",
      },
    });
    await fetchRoutingModels("claude", p);
    expect(invoke).toHaveBeenCalledWith(
      "fetch_models_for_config",
      expect.objectContaining({
        apiKey: "ds-key",
        modelsUrl: "https://api.deepseek.com/models",
      }),
    );
    await fetchRoutingModels(
      "gemini",
      card({
        env: {
          GOOGLE_GEMINI_BASE_URL: "https://google.test",
          GEMINI_API_KEY: "google-key",
        },
      }),
    );
    expect(invoke).toHaveBeenLastCalledWith(
      "fetch_models_for_config",
      expect.objectContaining({
        baseUrl: "https://google.test",
        apiKey: "google-key",
        apiFormat: "google-generative-ai",
      }),
    );
  });
  it.each([
    ["codex_oauth", "get_codex_oauth_models"],
    ["xai_oauth", "get_xai_oauth_models"],
  ])("uses the selected %s account", async (kind, command) => {
    await fetchRoutingModels(
      "claude",
      card(
        {},
        {
          providerType: kind,
          authBinding: {
            source: "managed_account",
            authProvider: kind,
            accountId: "account-A",
          },
        },
      ),
    );
    expect(invoke).toHaveBeenCalledWith(command, { accountId: "account-A" });
  });
  it("filters Copilot catalog by the Codex card's protocol and account", async () => {
    invoke.mockResolvedValue({
      autoRoutingSupported: false,
      models: [
        {
          id: "chat",
          vendor: "one",
          supported_endpoints: ["/v1/chat/completions"],
        },
        {
          id: "responses",
          vendor: "two",
          supported_endpoints: ["/v1/responses"],
        },
      ],
    });
    const results = await fetchRoutingModels(
      "codex",
      card(
        {},
        {
          providerType: "github_copilot",
          githubAccountId: "alice",
          codexCopilotApiFormat: "openai_chat",
        },
      ),
    );
    expect(invoke).toHaveBeenCalledWith("copilot_get_models_for_account", {
      accountId: "alice",
    });
    expect(results.map((m) => m.id)).toEqual(["chat"]);
  });
  it("rejects missing credentials and never uses the default managed account for a native-login card", async () => {
    await expect(
      fetchRoutingModels(
        "claude",
        card({
          env: {
            ANTHROPIC_BASE_URL: "https://test.invalid",
            ANTHROPIC_AUTH_TOKEN: "PROXY_MANAGED",
          },
        }),
      ),
    ).rejects.toMatchObject({
      messageKey: "providerForm.fetchModelsNeedApiKey",
    });
    await expect(
      fetchRoutingModels("codex", {
        ...card({}),
        id: CODEX_OFFICIAL_PROVIDER_ID,
        category: "official",
      }),
    ).rejects.toMatchObject({
      messageKey: "providerForm.fetchModelsNotSupported",
    });
    expect(invoke).not.toHaveBeenCalled();
  });
});
