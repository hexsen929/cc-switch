//! GitHub keeps its legacy identity. Other HTTPS repository roots use Forgejo's
//! archive/API layout, including instances hosted below a URL path prefix.
use super::{SkillRepo, SkillService, SKILL_DOWNLOAD_USER_AGENT};
use anyhow::{anyhow, bail, Result};
use url::Url;

pub(super) struct RepoSource {
    pub url: Url,
    pub github: bool,
}

impl SkillRepo {
    pub(crate) fn canonical_repo_url(&self) -> Result<Option<String>> {
        Ok(RepoSource::new(&self.owner, &self.name, self.repo_url.as_deref())?.stored_url())
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let source = RepoSource::new(&self.owner, &self.name, self.repo_url.as_deref())?;
        if source.github {
            return SkillService::validate_repo_ref(&self.owner, &self.name, &self.branch);
        }
        if !SkillService::is_valid_git_branch(&self.branch) {
            bail!("Invalid Skill repository branch");
        }
        Ok(())
    }
}

impl RepoSource {
    pub fn new(owner: &str, name: &str, repo_url: Option<&str>) -> Result<Self> {
        let fallback = format!("https://github.com/{owner}/{name}");
        let raw = repo_url.unwrap_or(&fallback).trim().trim_end_matches('/');
        let raw = raw.strip_suffix(".git").unwrap_or(raw);
        // Check before URL normalization, which would otherwise erase dot segments.
        if raw.contains(['\\', '%'])
            || raw.chars().any(char::is_control)
            || raw.split('/').any(|part| matches!(part, "." | ".."))
        {
            bail!("Invalid Skill repository URL");
        }
        let url = Url::parse(raw)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!("Skill repositories require an HTTPS URL without credentials, query or fragment");
        }
        let parts: Vec<_> = url
            .path_segments()
            .ok_or_else(|| anyhow!("Invalid repository path"))?
            .collect();
        if parts.len() < 2
            || parts[parts.len() - 2] != owner
            || parts[parts.len() - 1] != name
            || parts.iter().any(|part| part.is_empty())
        {
            bail!("Skill repository URL does not match owner/name");
        }
        let github = url.host_str() == Some("github.com");
        if github {
            if parts.len() != 2 || url.port().is_some() {
                bail!("Invalid GitHub repository root");
            }
            SkillService::validate_repo_ref(owner, name, "main")?;
        } else {
            for part in &parts {
                if !part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                {
                    bail!("Invalid Forgejo repository path component");
                }
            }
        }
        Ok(Self { url, github })
    }

    pub fn stored_url(&self) -> Option<String> {
        (!self.github).then(|| self.url.to_string())
    }

    pub fn identity(&self, owner: &str, name: &str) -> String {
        if self.github {
            format!("{owner}/{name}")
        } else {
            format!("forgejo:{}", self.url)
        }
    }

    pub fn archive_url(&self, branch: &str) -> Result<String> {
        if !SkillService::is_valid_git_branch(branch) || branch.is_empty() {
            bail!("Invalid archive ref");
        }
        let mut url = self.url.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow!("Invalid repository URL"))?;
        if self.github {
            path.extend(["archive", "refs", "heads"]);
            // Preserve the established GitHub branch-with-slashes archive shape.
            let parts: Vec<_> = branch.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                path.push(part);
            }
            path.push(&format!("{}.zip", parts[parts.len() - 1]));
        } else {
            path.push("archive").push(&format!("{branch}.zip"));
        }
        drop(path);
        Ok(url.to_string())
    }

    pub fn document_url(&self, branch: &str, doc_path: &str) -> Option<String> {
        if !SkillService::is_valid_git_branch(branch)
            || branch.is_empty()
            || SkillService::sanitize_skill_source_path(doc_path).is_none()
        {
            return None;
        }
        let mut url = self.url.clone();
        let mut path = url.path_segments_mut().ok()?;
        if self.github {
            path.push("blob");
        } else {
            path.extend(["src", "branch"]);
        }
        if self.github {
            path.extend(branch.split('/'));
        } else {
            path.push(branch);
        }
        path.extend(doc_path.split('/'));
        drop(path);
        Some(url.to_string())
    }

    pub fn api_url(&self, owner: &str, name: &str) -> String {
        let mut url = self.url.clone();
        let mut path = url.path_segments_mut().expect("validated hierarchical URL");
        path.pop().pop().extend(["api", "v1", "repos", owner, name]);
        drop(path);
        url.to_string()
    }

    /// Preserve the configured proxy and TLS validation, but never follow a
    /// repository response onto a different host, port, scheme or URL path.
    pub async fn fetch(&self, initial: &str) -> Result<reqwest::Response> {
        let client =
            crate::proxy::http_client::no_redirect_for_url(initial).map_err(|e| anyhow!(e))?;
        let original = Url::parse(initial)?;
        let mut url = original.clone();
        for _ in 0..5 {
            let response = client
                .get(url.clone())
                .header(reqwest::header::USER_AGENT, SKILL_DOWNLOAD_USER_AGENT)
                .send()
                .await?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| anyhow!("Repository redirect has no Location"))?
                .to_str()?;
            let next = url.join(location)?;
            if next.origin() != original.origin()
                || next.path().trim_end_matches('/') != original.path().trim_end_matches('/')
                || next.query().is_some()
                || next.fragment().is_some()
                || !next.username().is_empty()
                || next.password().is_some()
            {
                bail!("Repository redirect left the selected source");
            }
            url = next;
        }
        bail!("Too many repository redirects")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgejo_supports_codeberg_and_self_hosted_path_prefixes() {
        for root in [
            "https://codeberg.org/owner/repo",
            "https://git.example:8443/forge/owner/repo",
        ] {
            let source = RepoSource::new("owner", "repo", Some(root)).unwrap();
            assert!(!source.github);
            assert_eq!(
                source.archive_url("main").unwrap(),
                format!("{root}/archive/main.zip")
            );
            assert_eq!(
                source.document_url("main", "skills/a/SKILL.md"),
                Some(format!("{root}/src/branch/main/skills/a/SKILL.md"))
            );
            assert!(source
                .archive_url("feature/a")
                .unwrap()
                .ends_with("/archive/feature%2Fa.zip"));
            assert!(source
                .api_url("owner", "repo")
                .ends_with("/api/v1/repos/owner/repo"));
        }
    }

    #[test]
    fn github_legacy_identity_and_missing_field_are_preserved() {
        let old: SkillRepo = serde_json::from_value(
            serde_json::json!({"owner":"owner","name":"repo","branch":"main","enabled":true}),
        )
        .unwrap();
        assert_eq!(old.canonical_repo_url().unwrap(), None);
        let source =
            RepoSource::new("owner", "repo", Some("https://github.com/owner/repo.git/")).unwrap();
        assert_eq!(source.stored_url(), None);
        assert_eq!(source.identity("owner", "repo"), "owner/repo");
        assert_eq!(
            source.archive_url("main").unwrap(),
            "https://github.com/owner/repo/archive/refs/heads/main.zip"
        );
    }

    #[test]
    fn repository_coordinates_and_url_boundaries_are_validated() {
        for url in [
            "http://codeberg.org/owner/repo",
            "https://user:pass@codeberg.org/owner/repo",
            "https://codeberg.org/owner/repo?x=1",
            "https://codeberg.org/owner/repo#ref",
            "https://codeberg.org/other/repo",
            "https://codeberg.org/a/../owner/repo",
            "https://codeberg.org/owner%2frepo",
            "https://github.com:8443/owner/repo",
        ] {
            assert!(
                RepoSource::new("owner", "repo", Some(url)).is_err(),
                "{url}"
            );
        }
    }
}
