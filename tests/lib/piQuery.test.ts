import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import {
  invalidatePiDirectoryCaches,
  invalidatePiProviderCaches,
} from "@/lib/query/pi";

describe("Pi-family cache isolation", () => {
  it.each(["pi", "omp"] as const)(
    "invalidates only %s provider membership and provider rows",
    async (app) => {
      const client = new QueryClient();
      const other = app === "pi" ? "omp" : "pi";
      for (const id of [app, other]) {
        client.setQueryData([id, "currentState"], { enabledProviderIds: [] });
        client.setQueryData(["providers", id], {});
      }

      await invalidatePiProviderCaches(client, app);

      expect(client.getQueryState([app, "currentState"])?.isInvalidated).toBe(
        true,
      );
      expect(client.getQueryState(["providers", app])?.isInvalidated).toBe(
        true,
      );
      expect(client.getQueryState([other, "currentState"])?.isInvalidated).toBe(
        false,
      );
      expect(client.getQueryState(["providers", other])?.isInvalidated).toBe(
        false,
      );
      client.clear();
    },
  );

  it("invalidates Oh My Pi directory resources without touching Pi's native state", async () => {
    const client = new QueryClient();
    const keys = [
      ["omp", "currentState"],
      ["providers", "omp"],
      ["skills", "installed"],
      ["sessions"],
      ["pi", "currentState"],
      ["providers", "pi"],
    ];
    for (const key of keys) client.setQueryData(key, {});

    await invalidatePiDirectoryCaches(client, "omp");

    for (const key of keys.slice(0, 4)) {
      expect(client.getQueryState(key)?.isInvalidated).toBe(true);
    }
    for (const key of keys.slice(4)) {
      expect(client.getQueryState(key)?.isInvalidated).toBe(false);
    }
    client.clear();
  });
});
