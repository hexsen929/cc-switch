import type { TFunction } from "i18next";
import type { SkillRepoFailure } from "@/lib/api/skills";
import { skillErrorReason } from "@/lib/errors/skillErrorParser";
import { skillRepositoryKey, skillRepositoryLabel } from "@/lib/skillRepository";

export const repoFailureKey = skillRepositoryKey;

/** 「owner/name（原因）、owner2/name2（原因）」：放进「N 个仓库没有读到：…」横幅 */
export function describeRepoFailures(
  failures: readonly SkillRepoFailure[],
  t: TFunction,
): string {
  return failures
    .map((failure) =>
      t("skillsPage.repoFail.item", {
        repo: skillRepositoryLabel(failure),
        reason: skillErrorReason(failure.error, t),
      }),
    )
    .join(t("mcpPage.listSeparator"));
}
