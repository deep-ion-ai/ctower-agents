import type { HubRankingItem, Priority } from "../protocol/hub-gen";
import { hubProject, hubProjectView, hubProposal } from "../test/fixtures";
import { placeProposals } from "./proposalOrder";

const PROJECTS = [
  hubProjectView(hubProject({ rank: 1 })),
  hubProjectView(hubProject({ id: 2, gh_repo: "demo", name: "demo", rank: 2 })),
];

function item(position: number, repo: string, number: number, priority: Priority | null): HubRankingItem {
  return {
    position,
    owner: "deep-ion-ai",
    repo,
    number,
    ref: `deep-ion-ai/${repo}#${number}`,
    title: `Issue ${number}`,
    kind: "issue",
    status: "backlog",
    priority,
    rank: priority ? 1 : null,
  };
}

const RANKING = [
  item(1, "ctower-agents", 10, "P0"),
  item(2, "ctower-agents", 11, "P1"),
  item(3, "demo", 20, "P1"),
  item(4, "ctower-agents", 12, "P2"),
  item(5, "ctower-agents", 13, "P2"),
  item(6, "ctower-agents", 14, null),
];

test("ordem resultante: classe, projeto e âncora; sobe, desce e novo", () => {
  const out = placeProposals(
    [
      hubProposal({
        id: 1,
        number: 13,
        created_at: 1,
        current: { priority: "P2" },
        proposed: { priority: "P1", before: 11 },
      }),
      hubProposal({
        id: 2,
        repo: "demo",
        number: 20,
        created_at: 2,
        current: { priority: "P1" },
        proposed: { priority: "P0" },
      }),
      hubProposal({
        id: 3,
        number: 30,
        title: null,
        created_at: 3,
        current: { priority: null },
        proposed: { priority: "P2", after: 12 },
      }),
      hubProposal({ id: 4, number: 10, created_at: 4, current: { priority: "P0" }, proposed: { priority: "P3" } }),
      hubProposal({
        id: 5,
        kind: "status",
        number: 11,
        current: { status: "backlog" },
        proposed: { status: "pausado" },
      }),
    ],
    RANKING,
    PROJECTS,
  );
  expect(out.map((x) => [x.proposal.id, x.from, x.to, x.move])).toEqual([
    [2, 3, 1, "up"],
    [1, 5, 2, "up"],
    [3, null, 5, "new"],
    [4, 1, 6, "down"],
  ]);
  // O título da proposta tem preferência; sem ele e fora do ranking, nulo.
  expect(out.find((x) => x.proposal.id === 1)!.title).toBe("prioridade e ordem");
  expect(out.find((x) => x.proposal.id === 3)!.title).toBeNull();
});

test("âncora fora da classe proposta: fim da classe; mesma posição; título do ranking", () => {
  const out = placeProposals(
    [
      // #11 é P1: a âncora não está em P2, então o item vai para o fim da P2.
      hubProposal({
        id: 1,
        number: 14,
        title: null,
        current: { priority: null },
        proposed: { priority: "P2", before: 11 },
      }),
      hubProposal({ id: 2, number: 12, current: { priority: "P2" }, proposed: { priority: "P2", before: 13 } }),
    ],
    RANKING,
    PROJECTS,
  );
  expect(out.map((x) => [x.proposal.id, x.from, x.to, x.move, x.title])).toEqual([
    [2, 4, 4, "same", "prioridade e ordem"],
    [1, 6, 6, "same", "Issue 14"],
  ]);
});

test("ordem entre projetos segue project.rank", () => {
  const projects = [
    hubProjectView(hubProject({ rank: 2 })),
    hubProjectView(hubProject({ id: 2, gh_repo: "demo", name: "demo", rank: 1 })),
  ];
  const out = placeProposals(
    [hubProposal({ id: 1, number: 11, current: { priority: "P1" }, proposed: { priority: "P1", after: 99 } })],
    RANKING,
    projects,
  );
  // demo vem antes: P0 de ctower-agents (1), P1 de demo (2), P1 de ctower-agents (3).
  expect(out[0]).toMatchObject({ from: 2, to: 3, move: "down" });
});
