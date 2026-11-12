import { expect, test } from "vitest";
import {
  DEFAULT_RELEASE_CONFIG,
  releaseFormError,
  releaseOf,
  releasePatch,
  releasePrHead,
  releaseSupported,
  startBlocker,
  statusSummary,
} from "./release";
import { hubProject } from "../test/fixtures";
import type { HubReleaseStatus, ReleaseStatus } from "../protocol/hub-gen";

const ST: ReleaseStatus = {
  owner: "acme",
  repo: "acme-web",
  source: "develop",
  target: "main",
  source_sha: "d",
  target_sha: "m",
  content_diff: false,
  target_ahead_by: 0,
  source_ahead_by: 0,
  promotion_pr: null,
  release_pr: null,
  back_merge_pr: null,
  pending_runs: [],
  latest_release: null,
  next: "nothing",
};

test("só GitHub + GitHub; hub anterior sem o campo", () => {
  const gh = hubProject({});
  expect(releaseSupported(gh)).toBe(true);
  expect(releaseOf(gh)).toEqual(DEFAULT_RELEASE_CONFIG);
  const jira = hubProject({
    tracker: { kind: "jira" } as never,
    forge: { kind: "gitlab" } as never,
  });
  expect(releaseSupported(jira)).toBe(false);
  expect(releaseOf(jira)).toBeNull();
  const { release: _r, ...old } = gh;
  expect(releaseOf(old as never)).toBeNull();
  expect(releasePrHead(DEFAULT_RELEASE_CONFIG)).toBe("release-please--branches--main");
});

test("motivo do botão e resumo do estado", () => {
  const s = (p: Partial<HubReleaseStatus>): HubReleaseStatus => ({
    enabled: true,
    token_secret: "CT_RELEASE_GH_TOKEN",
    secret_present: true,
    status: ST,
    error: null,
    ...p,
  });
  expect(startBlocker(null, false)).toMatch(/carregando/);
  expect(startBlocker(s({ enabled: false }), false)).toMatch(/desligada/);
  expect(startBlocker(s({ secret_present: false }), false)).toMatch(/CT_RELEASE_GH_TOKEN/);
  expect(startBlocker(s({}), true)).toMatch(/viva/);
  expect(startBlocker(s({}), false)).toBeNull();
  expect(statusSummary(ST)).toMatch(/nada a lançar/);
  expect(statusSummary({ ...ST, content_diff: true, source_ahead_by: 4 })).toBe("4 commit(s) de develop a lançar em main");
  const pr = { number: 9, title: "chore: back-merge v1.2.3", url: "u", head: "main", base: "develop", head_sha: "x" };
  expect(statusSummary({ ...ST, back_merge_pr: pr })).toMatch(/back-merge pendente \(#9\)/);
  expect(statusSummary({ ...ST, release_pr: pr, next: "merge-release" })).toMatch(/mesclar o PR de release/);
});

test("patch e validação do formulário", () => {
  const cur = DEFAULT_RELEASE_CONFIG;
  expect(releasePatch(cur, { ...cur })).toBeNull();
  expect(releasePatch(cur, { ...cur, enabled: true, token_secret: " acme_token " })).toEqual({
    ...cur,
    enabled: true,
    token_secret: "ACME_TOKEN",
  });
  expect(releaseFormError(cur)).toBeNull();
  expect(releaseFormError({ ...cur, target: "develop" })).toMatch(/diferentes/);
  expect(releaseFormError({ ...cur, source: "a b" })).toMatch(/origem/);
  expect(releaseFormError({ ...cur, token_secret: "CTOWER_X" })).toMatch(/CTOWER_/);
  expect(releaseFormError({ ...cur, timeouts: { ...cur.timeouts, runs: 0 } })).toMatch(/prazos/);
});
