import { EMPTY_JIRA, hostsOf, type CodeHostForm, type IssueHostForm, type JiraForm } from "../../hub/tracker";
import type { DocsForm } from "../../hub/docs";

export interface ProjectForm {
  /**
   * Repositório do código-fonte e de demandas (#348); o tipo do projeto sai
   * dos dois (`kindOf`): GitHub + GitHub ou GitLab + Jira.
   */
  code: CodeHostForm;
  issues: IssueHostForm;
  url: string;
  name: string;
  base_branch: string;
  /** Separadas por vírgula. */
  protected: string;
  jira: JiraForm;
  /**
   * Docs do projeto (#444), só na edição; `null` = cadastro novo ou hub
   * anterior à #440 (sem `docs`).
   */
  docs: DocsForm | null;
}

export const EMPTY_PROJECT_FORM: ProjectForm = {
  ...hostsOf("github"),
  url: "",
  name: "",
  base_branch: "",
  protected: "",
  jira: EMPTY_JIRA,
  docs: null,
};

export const branches = (s: string) =>
  s
    .split(",")
    .map((b) => b.trim())
    .filter(Boolean);

export const keyOf = (o: string, r: string) => `${o}/${r}`;

export type Tone = "ok" | "attention" | "error" | "neutral";

export const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
