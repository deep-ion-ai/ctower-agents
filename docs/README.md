# Documentação do Control Tower

Um documento por linha, com quem deve ler. A visão geral do projeto está no [README da raiz](../README.md).

## Para quem instala e usa

| Documento | Para quem | O que tem |
| --- | --- | --- |
| [`ambiente.md`](ambiente.md) | quem instala e opera um ambiente (`ctowerd`) | instalação, `config.toml`, permissões pela UI, sync com GitHub, Jira e GitLab, logs, atualização e limpeza |
| [`ambiente-macos.md`](ambiente-macos.md) | quem roda um ambiente nativo no macOS | LaunchAgent, login automático, montagem dos docs dos projetos |
| [`app.md`](app.md) | quem usa o app desktop | instalação no macOS e compilação no Linux e no Windows |
| [`acesso-remoto.md`](acesso-remoto.md) | quem quer a UI web (PWA) no celular | `tailscale serve` e Cloudflare Tunnel com Access |

## Para quem opera o hub

| Documento | Para quem | O que tem |
| --- | --- | --- |
| [`hub.md`](hub.md) | quem opera o hub (`cthubd`) | instalação, projetos e atribuições, ligação dos ambientes, secrets, backup, migração, gestor de projetos e segurança |

## Para quem contribui

| Documento | Para quem | O que tem |
| --- | --- | --- |
| [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | quem propõe mudanças | fluxo de branches, commits, checks antes do PR e desenvolvimento da `ui/` |
| [`ci.md`](ci.md) | quem mexe no CI | workflows, matriz de jobs e runners |
| [`release.md`](release.md) | quem mantém o repositório | como uma versão é publicada pelo release-please |
| [`ui-roteiro-manual.md`](ui-roteiro-manual.md) | quem testa a UI à mão | roteiro de teste manual do app |
| `superpowers/specs/` e `superpowers/plans/` | quem quer entender o desenho | specs, planos e roadmap |
