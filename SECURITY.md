# Política de segurança

## Reportando uma vulnerabilidade

**Não abra uma issue pública** para vulnerabilidades. Use o relato privado do GitHub: aba **Security → Report a vulnerability** deste repositório.

Inclua o componente afetado (`ctowerd`, `cthubd`, `ctower`, app), a versão, os passos para reproduzir e o impacto. Respondemos em até 7 dias e coordenamos a divulgação depois da correção.

## Versões suportadas

Apenas a última versão publicada recebe correções de segurança.

## Pontos sensíveis

O `ctowerd` escuta só em `127.0.0.1` e o hub exige token. Segredos de projetos ficam no store criptografado do hub. Relatos sobre vazamento de tokens, escape do guarda-corpo de comandos dos agentes ou acesso sem token são tratados como prioridade.
