import { render, screen } from "@testing-library/react";
import { parse as parseToml } from "smol-toml";
import { describe, expect, it, vi } from "vitest";
import { UniversalProviderFormModal } from "@/components/universal/UniversalProviderFormModal";
import type { UniversalProvider } from "@/types";

vi.mock("@/components/JsonEditor", () => ({
  default: ({ value }: { value: string }) => (
    <textarea aria-label="generated-config-preview" value={value} readOnly />
  ),
}));
vi.mock("@/hooks/useDarkMode", () => ({ useDarkMode: () => false }));

describe("UniversalProviderFormModal Codex preview", () => {
  it("omits ignored settings while preserving endpoint, model, protocol and authentication", () => {
    const provider: UniversalProvider = {
      id: "custom",
      name: "Custom relay",
      providerType: "custom",
      apps: { claude: false, codex: true, gemini: false },
      baseUrl: "https://relay.example",
      apiKey: "test-api-key",
      models: { codex: { model: "custom-model", reasoningEffort: "medium" } },
    };
    render(
      <UniversalProviderFormModal
        isOpen
        editingProvider={provider}
        onClose={vi.fn()}
        onSave={vi.fn()}
      />,
    );
    const preview = JSON.parse(
      (screen.getByLabelText("generated-config-preview") as HTMLTextAreaElement)
        .value,
    ) as { auth: Record<string, unknown>; config: string };
    const config = parseToml(preview.config);

    expect(preview.auth).toEqual({ OPENAI_API_KEY: "test-api-key" });
    expect(config).toMatchObject({
      model_provider: "custom",
      model: "custom-model",
      model_reasoning_effort: "medium",
      model_providers: {
        custom: {
          name: "NewAPI",
          base_url: "https://relay.example/v1",
          wire_api: "responses",
          requires_openai_auth: true,
        },
      },
    });
    expect(config).not.toHaveProperty("disable_response_storage");
    expect(config).not.toHaveProperty("preferred_auth_method");
    expect(config).not.toHaveProperty("features.remote_connections");
  });
});
