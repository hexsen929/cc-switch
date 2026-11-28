import type { FetchedModel } from "@/lib/api/model-fetch";
import type { CopilotModel, CopilotModelOptions } from "@/lib/api/copilot";
import type { CodexCatalogModel, CodexCopilotApiFormat } from "@/types";

export const COPILOT_AUTO_MODEL_ID = "copilot-fusion-auto";

export function isCopilotModelSupportedByCodex(
  model: CopilotModel,
  format: CodexCopilotApiFormat = "auto",
): boolean {
  const endpoints =
    format === "openai_responses"
      ? ["/responses", "/v1/responses"]
      : format === "openai_chat"
        ? ["/chat/completions", "/v1/chat/completions"]
        : [
            "/responses",
            "/v1/responses",
            "/chat/completions",
            "/v1/chat/completions",
          ];
  return (model.supported_endpoints ?? []).some((endpoint) =>
    endpoints.includes(
      endpoint.split("?")[0].replace(/\/+$/, "").toLowerCase(),
    ),
  );
}

export function copilotFetchedModels(
  options: CopilotModelOptions,
  autoDisplayName: string,
): FetchedModel[] {
  return [
    ...(options.autoRoutingSupported
      ? [
          {
            id: COPILOT_AUTO_MODEL_ID,
            ownedBy: autoDisplayName,
            isCopilotAuto: true,
          },
        ]
      : []),
    ...options.models.map((model: CopilotModel) => ({
      id: model.id,
      ownedBy: model.vendor || null,
      billing: model.billing,
    })),
  ];
}

export function copilotAutoCatalogModel(
  displayName: string,
): CodexCatalogModel {
  return {
    model: COPILOT_AUTO_MODEL_ID,
    displayName,
    supportsParallelToolCalls: false,
    inputModalities: ["text"],
  };
}
