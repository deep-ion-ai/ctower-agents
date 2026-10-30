import { screenTail } from "./screen";

test("aplica movimentos de cursor em vez de mostrar o texto cru", async () => {
  // Escreve "rascunho", volta ao início da linha, apaga e reescreve.
  const raw = "linha 1\r\nrascunho\r\x1b[2Kfinal\r\n\x1b[1;31mvermelho\x1b[0m\r\n";
  expect(await screenTail(raw, 3)).toBe("linha 1\nfinal\nvermelho");
});

test("tira a caixa de entrada vazia e o rodapé do Claude Code", async () => {
  const rule = "─".repeat(60);
  const raw = [
    "● 9",
    "✻ Brewed for 5s · done 11:40 PM",
    rule,
    "❯ ",
    rule,
    "  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents",
  ].join("\r\n");
  expect(await screenTail(raw, 2)).toBe("● 9\n✻ Brewed for 5s · done 11:40 PM");
});

test("pedido de permissão continua visível", async () => {
  const raw = "Bash(npm install)\r\n  Do you want to proceed?\r\n❯ 1. Yes\r\n  2. No\r\n";
  expect(await screenTail(raw, 3)).toBe("  Do you want to proceed?\n❯ 1. Yes\n  2. No");
});

test("vazio", async () => {
  expect(await screenTail("", 4)).toBe("");
});
