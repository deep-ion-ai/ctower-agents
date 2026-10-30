// Ícone do provedor do projeto (#352, spec do Jira e GitLab §11): o do
// rastreador e, se a forja for outra, o dela (Jira + GitLab).
import { Icon } from "../shell/icons";
import { forgeLabel, isGitlabProject, isJiraProject, trackerLabel, type ProviderProject } from "../util/item";

/** Nome dos provedores do projeto: `GitHub` ou `Jira + GitLab`. */
export function providerText(p: ProviderProject): string {
  const t = trackerLabel(p);
  const f = forgeLabel(p);
  return t === f ? t : `${t} + ${f}`;
}

export function ProviderIcon({ project, size = 14 }: { project: ProviderProject; size?: number }) {
  const text = providerText(project);
  return (
    <span className="provider-icon" role="img" aria-label={text} title={text}>
      <Icon name={isJiraProject(project) ? "jira" : "github"} size={size} />
      {isGitlabProject(project) && <Icon name="gitlab" size={size} />}
    </span>
  );
}
