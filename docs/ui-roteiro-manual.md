# Roteiro de teste manual da UI (Windows)

Valida de ponta a ponta o que os testes automatizados não alcançam: WebView2, notificações do Windows, WSL de verdade e o Claude Code real. Leva uns 20 minutos.

Anote o resultado de cada passo (ok / falhou + o que apareceu) e cole no PR.

## Pré-requisitos

- Distro `control-tower` provisionada (`scripts/provision/provision.sh`), com `ctowerd` ativo: `wsl -d control-tower -- systemctl --user is-active ctowerd` responde `active`.
- `gh auth status` ok dentro da distro, o ambiente ligado a um hub (`cthubd`, `docs/hub.md`) e pelo menos um projeto atribuído a ele (pode ser feito no passo 3).
- Uma issue aberta de teste no repositório do projeto (ex.: "Teste manual da UI").

## 1. Instalar

1. Baixe o instalador da release mais recente (ou, para um PR `develop → main`, o artefato `control-tower-windows` da execução do workflow **ui**, se gerado). Com o app já instalado numa versão com updater, use o passo 11.6.
2. Rode o instalador `Control Tower_*_x64-setup.exe` (instalação por usuário, sem admin). O Windows SmartScreen pode avisar por falta de assinatura: "Mais informações → Executar assim mesmo".
3. **Esperado:** o app abre na **Sala de controle**.

## 2. Conectar

1. Com a distro desligada (`wsl --terminate control-tower`), abra o app.
2. **Esperado:** em poucos segundos a lateral mostra `control-tower (WSL)` com bolinha verde e "conectado · N sessões". O app subiu a distro sozinho (`wsl --exec true`).
3. Em **Configurações → Ambientes**, o token aparece como "lido da distro control-tower" (nunca o valor).
4. Em `%APPDATA%\ai.deep-ion.control-tower\envs.json`, confira que **não** há token gravado para o ambiente WSL.

## 3. Projeto e início de sessão

1. Se não houver projeto: configure o hub em **Configurações → Hub** e, em **Projetos**, **+ Novo projeto** pela URL do GitHub; depois **⋯ → Atribuir a ambiente** na linha do projeto (passos 9.2 a 9.6). Clique em **Sincronizar agora** no topo.
2. Vá em **Projetos** (barra de atividades). **Esperado:** o índice de projetos (não mais o primeiro projeto): uma linha por projeto, na ordem de prioridade do hub (`#1`, `#2`…), com o estado (atribuição com erro ou pendente, ou ambientes e sincronização), as sessões (em execução e aguardando), os ambientes atribuídos e a saúde (críticos e avisos, ou "sem riscos"), e à direita **⚙** (a aba **Configurações** do projeto) e **⋯** (Abrir, Configurações, Atribuir a ambiente, Sessão de planejamento, Abrir no GitHub/GitLab/Jira; o mesmo menu do botão direito na linha e da Sala de controle). Clique no projeto. **Esperado:** abre a aba do projeto, com a trilha **Projetos › <projeto>** no topo do painel lateral (no lugar do antigo seletor), e abaixo do título, as abas **Visão geral · Itens · Sessões · Configurações**, com **Itens** ativa; a árvore mostra os épicos com as sub-issues e as issues avulsas, cada uma com o status escrito.
3. Clique **▶ Iniciar** na issue de teste. **Esperado:** painel com a branch (`feat/<n>-...`), o worktree em `<raiz>/<projeto>/feat__<n>-...` e o prompt preenchido pelo template.
4. Clique **▶ Iniciar sessão**. **Esperado:** abre a aba do terminal com o Claude Code rodando no worktree.
5. Índice e abas (#16): na aba do projeto, clique **Projetos** na trilha (ou o ícone **Projetos** da barra de atividades). **Esperado:** volta ao índice, e a aba do projeto continua na faixa de abas; clicar nela volta ao projeto como estava. Abra outro projeto pelo índice. **Esperado:** abre noutra aba; abrir de novo um projeto já aberto (pelo índice, pela paleta ou pela Sala de controle) só ativa a aba dele. Com o índice na tela, clicar de novo em **Projetos** recolhe ou mostra o painel lateral.
6. No índice, digite parte do nome (ou de `owner/repo`) em **Filtrar projetos**. **Esperado:** só as linhas que casam; sem nenhuma, "Nenhum projeto com …". **+ Novo projeto** abre o formulário de cadastro no próprio índice (só no desktop, com o hub conectado).
7. Prioridade entre projetos: arraste um projeto para a metade de baixo de outro, ou foque a linha e tecle **Alt+↑**/**Alt+↓**. **Esperado:** "… desceu/subiu na ordem dos projetos." e a lista (e os `#n`) muda de ordem. Na PWA (só leitura) as linhas não arrastam e Alt+↑/↓ não faz nada.
8. Sem hub (hub fora do ar ou sem conexão): abra **Projetos**. **Esperado:** o aviso "Hub fora do ar: cada ambiente mostra os próprios projetos…", os projetos agrupados por ambiente (com a sincronização do ambiente e as sessões), sem **⚙**, sem **+ Novo projeto** e sem reordenar; clicar abre o projeto pelos dados locais daquele ambiente. Sem nenhum projeto, "Nenhum projeto cadastrado…" e **Configurar o hub**.

## 4. Responder permissão

1. No terminal, peça ao Claude algo que exija permissão (ex.: "rode `ls` no diretório").
2. **Esperado:** a Sala de controle mostra o cartão **Permissão** na faixa "Precisa de você", com "Bash: ls …", e o tile com o selo "aguardando você". Abaixo do console, a faixa mostra a mesma ferramenta, "terminal em 1:59" contando e **Negar**, **Sempre nesta sessão**, **Permitir** e **Responder no terminal**. O menu do Claude Code ainda não aparece no xterm.
3. Clique **Permitir**. **Esperado:** a faixa some, o agente roda o comando e o selo volta a "rodando".
4. Peça outro comando e clique **Sempre nesta sessão**. Peça o mesmo comando de novo. **Esperado:** na segunda vez não há pedido.
5. Peça outro comando e clique **Responder no terminal**. **Esperado:** a faixa some, o foco vai para o console e o menu do Claude Code aparece no xterm; responda nele.
6. Peça outro comando e não responda. **Esperado:** depois de 2 min a faixa some sozinha e o menu aparece no xterm. Com o app fechado, o menu aparece na hora.
7. Barra de status: com a sessão ativa, **Esperado:** "Contexto N% · Xk/200k", atualizado ao fim de cada resposta.
8. Redimensione a janela. **Esperado:** o terminal acompanha o tamanho, sem quebrar o desenho do Claude Code.

## 5. Queda do daemon e reconexão

1. Com a aba do terminal aberta, rode no PowerShell: `wsl -d control-tower -- systemctl --user restart ctowerd` (se pedir `--force`, use `ctower restart --force` dentro da distro).
2. **Esperado:** a lateral mostra "desconectado"; a faixa mostra o cartão **Ambiente**; notificação "Ambiente desconectado" (se a janela estiver minimizada, ainda assim aparece).
3. **Esperado:** em até ~30 s o app reconecta sozinho. Como o daemon reiniciou, a sessão vira **interrompida**: a aba mostra a mensagem de encerramento e a **Recuperação** ganha o contador.
4. Repita derrubando só a rede (ex.: `wsl --shutdown` e reabrir): o comportamento de reconexão deve ser o mesmo, sem saída duplicada no terminal.

## 6. Recuperar

1. Feche e reabra o app. **Esperado:** a tela **Recuperação** abre sozinha.
2. Confira o cartão: branch, worktree, "Transcript: original" ou "restaurável do backup", `git status` do worktree.
3. **Esperado:** o cartão **Últimas linhas** mostra o fim da conversa (você, agente, ferramenta), com "do backup" se o original se perdeu.
4. Edite o prompt de retomada e clique **Retomar**. **Esperado:** nova aba com a mesma conversa do Claude (`--resume`) no mesmo worktree.
5. Com outra sessão interrompida, clique **Descartar**. **Esperado:** ela sai de **Interrompidas** e aparece em **Descartadas recentemente**; aberta, mostra as últimas linhas e só **Recomeçar com nota**.

## 7. Revisar

1. Peça ao Claude para registrar uma decisão (`ledger_record_decision`) sem você orientar, abrir o PR para `develop` e então pare a sessão.
2. Clique **Sincronizar agora**. **Esperado:** o item aparece em **Em revisão** no rodapé, com "1 decisão pendente".
3. Abra a **Revisão**: o cartão "autônoma" com contexto, decisão e consequências. **Abrir PR #n** abre o navegador no PR.
4. **Comentar no PR**: o campo vem com a decisão em Markdown. Edite e **Enviar comentário**. **Esperado:** "Comentário publicado no PR" com **Ver no GitHub**, e o comentário está no PR.
5. Com duas decisões pendentes, clique **Revisada** numa. **Esperado:** só ela sai da lista. **Marcar 1 como revisada** marca a outra; "Nenhuma decisão pendente."
6. Num épico com decisões nas sub-issues, filtre **Revisadas** ou **Orientadas**. **Esperado:** as das sub-issues aparecem com "sub-issue #n".

## 8. Encerrar e preferências

1. Num terminal com sessão viva, **Encerrar sessão → Confirmar**. **Esperado:** a aba mostra "sessão encerrada (sinal)" e para de aceitar teclado; nenhuma notificação de erro (encerrar não é erro).
2. Numa sessão do Claude Code, digite `/exit`. **Esperado:** a aba mostra "sessão encerrada (código 0)" e fecha sozinha cerca de 1 s depois, mesmo em split ou em segundo plano; a sessão continua na Projetos como encerrada. Repita teclando algo logo após o `/exit`: a aba fica. Com **Configurações → Terminal → Fechar a aba quando a sessão sair sem erro** desmarcado, a aba também fica. Saída com erro (código ≠ 0), por sinal ou interrompida mantém a aba aberta com a mensagem.
3. Em **Configurações → Notificações**, desmarque "Sessão aguardando você" e repita o passo 4.2. **Esperado:** sem notificação.
4. **Abrir no VS Code** (barra do terminal) abre o VS Code conectado à distro no worktree.

## 9. Hub (plano de controle)

Pré-requisito: um `cthubd` no ar (`docs/hub.md`, seção 1) com o token do GitHub configurado, e dois ambientes ligados a ele (`cthubd add-env` + `ctower hub set-token`), os dois cadastrados em **Configurações → Ambientes**. Tenha à mão o token de UI do hub (`~/.config/cthub/ui.token` na máquina do hub).

1. Antes de configurar o hub na UI. **Esperado:** a faixa "Conexão com o hub não configurada" com o botão **Configurar o hub**; a lateral mostra **Hub · conexão não configurada**. Na lateral, cada ambiente mostra "configuração rev N · há …" (ou "ambiente não ligado a um hub", em vermelho, num ambiente sem `hub_url`).
2. Em **Configurações → Hub**, **Configurar hub**: URL `http://<máquina do hub>:7879` e o token de UI do hub; **Salvar hub**. **Esperado:** estado "conectado" e a linha "cthubd X · protocolo 1.0 · configuração rev N"; o token aparece só como "•••• definido" (em **Editar conexão** o campo vem vazio). As conexões em **Configurações → Ambientes** não mudam.
3. Abra a tela **Hub**. **Esperado:** versão, revisão da configuração e o último snapshot; **Avisos** lista o que o `hub.status` trouxer (ex.: com `backup_dir` no mesmo disco do `hub.db`, "backup no mesmo disco"); o menu **Hub** mostra o número de avisos e a Sala de controle mostra os mesmos avisos na faixa **Precisa de você**, com **Ver hub**. **GitHub do hub** mostra a conta do token do hub.
4. **+ Cadastrar projeto** (em **Hub → Projetos**, ou **+ Novo projeto** no índice de **Projetos**, que abre o mesmo formulário) com a URL de um repositório de teste (`https://github.com/<owner>/<repo>`), sem mais nada. **Esperado:** "… cadastrado no hub"; o projeto aparece com base `develop` e protegidas `develop, main, master`. Repita com uma URL inválida ou uma base que não existe. **Esperado:** a mensagem do hub em vermelho, com o formulário aberto.
5. No índice de **Projetos**, **⚙** na linha do projeto (abre a aba **Configurações** do projeto; o mesmo vale em **Hub → Projetos**). **Editar** o projeto, troque as protegidas e **Salvar projeto**. **Esperado:** a lista atualiza; num ambiente atribuído, a revisão da configuração na lateral sobe em até ~1 min.
6. **+ Atribuir a um ambiente**: escolha um ambiente e deixe o caminho vazio. **Esperado:** a atribuição aparece como **pendente** e vira **aplicada** depois do pull e do clone no ambiente (alguns segundos a 1 min); em **Ambientes** da tela Hub, a linha do ambiente lista o projeto. Atribua a outro ambiente um caminho que não é clone do repositório (ex.: um diretório com outro repo). **Esperado:** **com erro: …** com a mensagem do ambiente. A mensagem só aparece no estado de erro.
7. Abra o projeto pelo índice de **Projetos** (ou pela lateral da Sala de controle). **Esperado:** a aba do projeto, com a trilha **Projetos › <projeto>** e o selo **visão do hub**; com o projeto atribuído aos dois ambientes, **Iniciar em** lista primeiro os aplicados (os outros com "· pendente"/"· com erro"). **▶ Iniciar** numa issue sobe a sessão no ambiente escolhido; terminal, sessões e Recuperação continuam por ambiente.
7a. Abas do projeto (#17). Clique **Visão geral**. **Esperado:** a aba de editor passa a "<repo> · Visão geral" (a mesma aba, sem abrir outra), com o estado, a sincronização, as atribuições (**Gerenciar** leva a Configurações › Ambientes e atribuições) e as sessões vivas com **Abrir terminal**. Clique **Sessões**. **Esperado:** um cartão por ambiente com as **Vivas**, as **Interrompidas** (**Abrir na Recuperação**) e as **Encerradas com erro (últimas 24 h)**; um ambiente que só o hub conhece aparece "só pelo hub", sem botões. Clique **Configurações do projeto** no cabeçalho. **Esperado:** a aba **Configurações** do próprio projeto (não o Hub), com o índice **Geral · Repositório e rastreador · Ambientes e atribuições · Regras do agente · Secrets e variáveis · Docs · Zona de perigo**; cada seção troca o título da aba ("<repo> · Configurações › Docs"). Na paleta, `>projeto sessões <repo>` e `>regras <repo>` abrem a aba e a seção. Feche e reabra pelo atalho de antes (lateral, Sala de controle, "Abrir árvore" no Hub): cai em **Itens**, como antes. Na PWA, abra `/?go=project&project=<owner>/<repo>&view=settings&section=rules`. **Esperado:** o projeto direto em Configurações › Regras do agente, só leitura; o índice não tem **Secrets e variáveis** nem **Zona de perigo**, e **Geral** não tem **Editar**.
8. Com uma sessão viva no projeto, **Remover** a atribuição desse ambiente. **Esperado:** o hub pede confirmação listando as sessões vivas ("não encerra as sessões"); **Remover mesmo assim** remove, e a sessão continua rodando. Em **Configurações → Projetos** desse ambiente, depois do pull, o projeto sai da tabela e aparece em **Fora do hub** como "atribuição removida há …", com o caminho do clone para limpar à mão.
9. **Excluir** o projeto. **Esperado:** a confirmação lista o impacto que o hub informa (atribuições ativas, sessões vivas) e os secrets do projeto; confirmar exclui num clique e mostra o caminho do snapshot `hub-…-pre-delete.db`. Se algo mudar entre abrir e confirmar, o hub recusa e **Excluir mesmo assim** aparece.
10. Quarentena: num ambiente com sessões de um projeto que não está no hub (ex.: depois do passo 9, ou um projeto local legado), rode `ctower hub resync` e `ctower hub sync`. **Esperado:** em **Quarentena**, o projeto com as linhas por ambiente e um aviso em **Avisos**. **Cadastrar projeto** abre o formulário com a URL preenchida (cadastrar reaplica as linhas); **Descartar → Descartar linhas** descarta e o aviso some.
11. Pare o hub (`systemctl --user stop cthubd`). **Esperado:** em até ~15 s, a faixa "hub indisponível: visão parcial", a lateral **Hub · indisponível há …**, o cartão **Hub indisponível** na faixa de atenção e, na tela Hub, "Hub indisponível: cadastro, atribuições e a visão consolidada voltam quando ele responder". Sessões e terminais seguem; iniciar sessão num projeto atribuído continua funcionando (a tela Projeto volta aos dados locais, e o índice de **Projetos** lista os projetos de cada ambiente, sem a ordem de prioridade); na lateral, "configuração rev N · há … · pull falhou" e "hub: N pendentes · falhou há …".
12. Suba o hub de novo. **Esperado:** o aviso some sozinho, a visão do hub volta e as pendências voltam a 0 em até alguns minutos (backoff de até 5 min).
13. Em **Configurações → Hub**, **Remover conexão**. **Esperado:** estado "não configurado", a tela Hub pede a configuração e as telas dos ambientes seguem funcionando.

## 10. Console (terminal embutido)

1. Abra três sessões (de preferência já com saída longa) e troque entre as abas sem digitar nada. **Esperado:** cada aba aparece no tamanho da janela, sem texto embaralhado nem linhas cortadas; o Claude Code redesenha a tela inteira ao aparecer a primeira vez.
2. Feche o app com sessões vivas, abra de novo e reabra as abas. **Esperado:** o histórico aparece no tamanho certo.
3. Peça ao Claude um link (ex.: "mostre o link do PR"). **Esperado:** clique simples não abre; Ctrl+clique abre no navegador, sem alerta. Vale para links OSC 8 e para URLs em texto puro. Clique direito sobre o link copia o endereço.
4. Selecione texto com o mouse (Shift+arrastar, se o Claude Code estiver capturando o mouse) e tecle Ctrl+C. **Esperado:** o texto vai para a área de transferência e a sessão não recebe ^C. Sem seleção, Ctrl+C interrompe como antes. Ctrl+Shift+C também copia.
5. Copie várias linhas de outro programa e tecle Ctrl+V (ou Ctrl+Shift+V, ou clique direito sem seleção). **Esperado:** cola tudo no prompt sem enviar.
6. No prompt do Claude Code, tecle Ctrl+Enter e Shift+Enter. **Esperado:** quebra a linha sem enviar a mensagem.
7. Tecle Ctrl+F. **Esperado:** abre a barra de busca sobre o terminal; Enter e Shift+Enter vão para a próxima e a anterior; o contador mostra "n de N"; **Aa** diferencia maiúsculas; Esc fecha e devolve o foco ao terminal.
8. Tecle Ctrl+= algumas vezes, depois Ctrl+- e Ctrl+0. **Esperado:** a fonte cresce e diminui em todas as abas, o Claude Code se redesenha no novo tamanho, e Ctrl+0 volta ao tamanho das Configurações.
9. Em **Configurações → Terminal**, troque a fonte (ex.: `'Cascadia Code', monospace`) e o tamanho, depois salve. **Esperado:** os terminais abertos mudam na hora, e a escolha continua valendo depois de reabrir o app. Cores ANSI (diff, erros) aparecem com a paleta do app, e emojis e símbolos do Claude Code não desalinham as colunas.
10. Com várias sessões vivas, peça uma saída longa (ex.: `git log -p` de um repositório grande) e role o histórico. **Esperado:** a UI não trava e as outras abas continuam respondendo; a rolagem é fluida (renderizador WebGL). Se o WebGL falhar (driver), o console continua funcionando com o renderizador padrão.

### 10.1 Shell livre no worktree (#423)

Rode no Windows (WSL) e no macOS com o Mac mini remoto.

1. Numa sessão viva de uma issue, clique **Abrir shell no worktree** (ícone `>_`) no cabeçalho do painel. **Esperado:** abre a aba **Shell · #n** com o prompt do seu shell de login (o Oh My Zsh, se você usa), na pasta do worktree. O cabeçalho mostra só a pasta, o programa (`zsh`) e a duração, com Buscar, Abrir no VS Code, dividir e **Fechar shell**. Na árvore, `zsh · #n` aparece logo abaixo da linha da sessão do item.
2. Rode `env | grep CTOWER`. **Esperado:** nada (o shell não recebe as variáveis do Control Tower).
3. Volte à aba da sessão, **Dividir à direita** com o shell. **Esperado:** o agente à esquerda e o shell à direita, cada um no tamanho da coluna; os dois aceitam digitação.
4. Tecle Ctrl+Shift+` com o foco no console e, depois, use a paleta (**Shell: novo no worktree da sessão ativa**). **Esperado:** cada vez abre um shell novo, em aba própria. Na tela Projeto, no detalhe do item, **Abrir shell no worktree** faz o mesmo.
5. Feche o app (ou recarregue a UI) com o shell aberto e reabra. **Esperado:** a árvore lista o shell vivo; o clique reabre a aba com o histórico.
6. No shell, rode `sleep 100` e clique no × da aba. **Esperado:** a confirmação "Há um comando rodando no shell. Fechar encerra o processo."; **Cancelar** mantém o shell, **Fechar** encerra e fecha a aba. Sem comando rodando, o × fecha direto.
7. No shell, digite `exit 3`. **Esperado:** a aba mostra "shell encerrado (código 3)", com **Abrir novo shell** e **Fechar**; o shell some da árvore.
8. Na sessão do projeto (#0), abra um shell. **Esperado:** a aba é **Shell · sessão do projeto**, a pasta é o clone principal e o cabeçalho avisa **clone principal**.
9. Abra shells até passar de 8 no ambiente. **Esperado:** o nono é recusado com a mensagem do limite, no topo da tela Terminal.
10. Na PWA (#205), abra uma sessão. **Esperado:** nenhum ponto de entrada do shell aparece (botão, paleta, tela Projeto).

### 10.2 Ações do console (#424)

Rode no Windows (WSL) e no macOS com o Mac mini remoto.

1. Numa sessão viva de uma issue, abra o menu **⋯** (Mais ações) do cabeçalho. **Esperado:** Abrir issue no GitHub, Abrir PR, Abrir item no projeto, Abrir no VS Code (desabilitado fora do WSL) e Reiniciar. Com **Dividir à direita**, os dois cabeçalhos cabem sem quebrar a linha dos botões.
2. Clique **Abrir issue no GitHub**. **Esperado:** abre `https://github.com/<owner>/<repo>/issues/<n>` no navegador (no projeto Jira, a issue no Jira).
3. Na sessão do projeto (#0), abra o **⋯**. **Esperado:** **Abrir repositório no GitHub** no lugar de Abrir issue, sem Abrir PR nem Abrir item no projeto. No gestor, só Revisar decisões do gestor e Reiniciar.
4. **Retomar uma sessão encerrada:** numa sessão de issue, digite `/exit` no Claude Code (ou encerre pelo botão). Divida à direita com outra sessão antes, deixando a encerrada numa das colunas. **Esperado:** o cabeçalho da encerrada mostra **Retomar**; o clique, sem modal, abre a conversa de volta (`claude --resume`) com o mesmo modelo e modo, na mesma aba (mesma posição na faixa) e na mesma coluna; a outra coluna não muda.
5. Retome uma sessão cujo transcript foi apagado (apague o `.jsonl` e o backup). **Esperado:** o erro do daemon aparece no cabeçalho com **Abrir Recuperação**, que leva à tela Recuperação.
6. **Reiniciar a sessão viva:** **⋯ → Reiniciar**. **Esperado:** a confirmação "Reiniciar encerra o processo atual do agente…"; **Continuar** abre a modal de início com o template do tipo (issue, épico ou projeto), o modelo e o modo da sessão anterior, e o aviso de que a atual será encerrada. **Cancelar** na modal não mata nada. **Iniciar sessão** encerra a antiga e a nova ocupa a mesma aba e coluna. No gestor, a modal é a do gestor (mesmo ambiente e escopo) e chama `session.start_gestor`.
7. **Reiniciar a sessão encerrada:** **⋯ → Reiniciar** abre a modal direto, sem confirmação; a nova sessão fica na aba da antiga.
8. Com a sessão ativa, abra a paleta (Ctrl+K / ⌘K) e digite `Sessão:`. **Esperado:** Sessão: Retomar (só na encerrada), Sessão: Abrir issue no GitHub, Sessão: Abrir PR, Sessão: Reiniciar…, aplicadas à sessão ativa; na tela Terminal fechada (outra aba ativa), elas não aparecem.

### 10.3 Prompts prontos (#425)

Rode no Windows (WSL) e no macOS com o Mac mini remoto.

1. Em **Configurações › Sessões › Prompts prontos**, confira os cinco padrões (Continuar, Rodar os testes e corrigir, Registrar as decisões no ledger, Abrir o PR, Resumir o estado atual). Adicione um prompt **Duas linhas** com o texto abaixo e clique **Salvar prompts**:

   ```
   Leia a issue {{key}} de novo.
   Depois diga em uma frase o que falta na branch {{branch}}.
   ```

2. **Mandar um prompt de várias linhas:** numa sessão viva de issue, com o Claude Code parado no prompt, clique no botão **Enviar prompt pronto** (avião de papel) do cabeçalho e escolha **Duas linhas**. **Esperado:** as duas linhas chegam juntas na caixa de entrada do Claude Code, com `{{key}}` e `{{branch}}` trocados pelos da issue, e só então o Enter envia (uma mensagem só, não duas); o foco volta ao console.
3. Repita com **Continuar** e com **Rodar os testes e corrigir** pela paleta (`Sessão: Enviar prompt — …`). **Esperado:** o mesmo, aplicado à sessão ativa.
4. Peça ao agente algo que exija permissão (ex.: um `git push`) e, com a faixa **Permissão** aberta, abra o menu de prompts. **Esperado:** o aviso "O agente está esperando a resposta do pedido de permissão…", os prompts desabilitados e nada enviado; na paleta, os `Sessão: Enviar prompt — …` somem até o pedido ser respondido.
5. Encerre a sessão. **Esperado:** o botão **Enviar prompt pronto** fica desabilitado ("A sessão está encerrada").
6. De volta às Configurações, **Restaurar padrão** do item volta os cinco padrões; o de cada prompt padrão editado volta só aquele (e precisa de **Salvar prompts**).

### 10.4 Painel de contexto (#426)

Rode no Windows (WSL) e no macOS com o Mac mini remoto.

1. Numa sessão viva de issue, clique **Contexto** no cabeçalho (ícone de painel à direita). **Esperado:** um painel de uns 320 px à direita do console, com **Work item** (status, branch, worktree e sessões anteriores), **PR**, **Decisões** e **Eventos**; o console encolhe e se reajusta (sem cortar linhas).
2. Feche o app e abra de novo. **Esperado:** o painel volta aberto (o estado fica na máquina). Ctrl+Alt+B (⌘⌥B no macOS), também com o foco no console, fecha e abre; Ctrl+B / ⌘B continua só com o painel lateral. Na paleta, **Sessão: Mostrar contexto** / **Ocultar contexto**.
3. **PR:** num item com PR, o número, o estado e o link abrem o PR. Num item com branch e sem PR, **Criar PR** abre a página de compare (no GitLab, a do MR novo).
4. **Decisões:** a mais nova primeiro, com **autônoma**/**orientada**, "pendente de revisão" ou "revisada no PR #n"; **Revisar decisões** abre a tela de revisão do item.
5. **Eventos:** cinco por vez; **Mostrar mais** traz os seguintes até acabar. Num ambiente anterior ao protocolo 2.9, a seção avisa "atualize o ambiente" e o resto do painel funciona.
6. **Recarga:** com o painel aberto, faça o agente pedir algo (needs_human) ou registre uma decisão. **Esperado:** em cerca de 1 s o painel mostra o status, a decisão e o evento novos, sem piscar a cada push.
7. **Status pelo hub:** com o ambiente enviando ao hub e a UI conectada a ele, **Mudar status ▾** no Work item é o mesmo seletor da tela Projeto (com o motivo); a mudança aparece no painel. Sem o hub, só o selo do status.
8. **Sessões anteriores:** uma sessão interrompida do item mostra **Recuperação ↗**, que abre a Recuperação; com o hub, as sessões de outros ambientes também aparecem.
9. **Split:** divida à direita com outra sessão. **Esperado:** o painel mostra a sessão da coluna em que você clicou no console; o **Contexto** da outra coluna passa o painel para ela (clicar de novo na mesma fecha).
10. **Janela estreita:** diminua a janela (ou abra o painel lateral largo) até o console ficar com menos de ~480 px (dois consoles abaixo de ~400 px cada no split). **Esperado:** o painel passa a ficar por cima do console, com sombra, e o PTY não muda de largura.
11. **Sessão do projeto (#0):** sem a seção PR; a pasta é o clone principal (com o aviso), e as decisões e eventos são os do #0. No gestor, só Decisões e Eventos.
12. Abra um shell no worktree (aba de shell) com o painel aberto. **Esperado:** o painel some na aba do shell e volta ao voltar para a sessão.

### 10.5 Muitas abas e sessão do projeto (#427)

Rode no Windows (WSL) e no macOS com o Mac mini remoto.

1. **Ordem por projeto:** abra sessões de dois projetos alternando (A, B, A). **Esperado:** a segunda de A entra logo depois da primeira de A, antes da de B. Passe o mouse numa aba: o tooltip mostra o título, o projeto e o ambiente.
2. **"Fechar encerradas" com dez abas:** abra dez abas (sessões e shells, de dois projetos). Encerre três sessões normalmente (`/exit`), mate uma quarta com `kill -9` (fica **interrompida · recuperável**) e saia de um shell com `exit`. Clique com o botão direito numa aba › **Fechar encerradas**. **Esperado:** saem as três encerradas e o shell encerrado; a interrompida recuperável fica. Até fechar, as abas encerradas mostram o título do item (não "Sessão"). O botão **Fechar encerradas** do cabeçalho da árvore faz o mesmo e fica desabilitado sem abas encerradas.
3. **Fechar outras / à direita:** no menu de uma aba do meio. **Esperado:** "outras" deixa só ela (ativa); "à direita" fecha as seguintes. As sessões do agente cujas abas fecharam seguem vivas na árvore (fechar a aba não encerra o agente).
4. **Shell ocupado:** deixe `sleep 60` rodando num shell e use **Fechar outras** a partir de outra aba. **Esperado:** uma confirmação única abaixo da faixa de abas ("1 shell tem um comando rodando. Fechar encerra os processos.") com **Fechar n abas** e **Cancelar**; confirmando, o shell encerra (some da árvore). Sem comando rodando, fecha direto.
5. **Fechar as do projeto:** no menu da aba (**Fechar as do projeto <nome>**) e no menu de contexto do grupo na árvore (botão direito ou Shift+F10: **Fechar as abas deste projeto**). **Esperado:** só as abas de sessão e de shell daquele projeto fecham.
6. **Lista de abas:** abra abas até a faixa rolar. **Esperado:** aparece o ⌄ no fim da faixa; ele abre a lista com o filtro em foco e o projeto de cada aba. Digite parte do título ou do projeto, use ↑/↓ e Enter: a aba ativa e aparece na faixa. Esc fecha e devolve o foco ao ⌄. Com a janela larga o bastante, o ⌄ some.
7. **Sessão do projeto (#0):** abra uma sessão do projeto. **Esperado:** a aba é "Sessão do projeto · <projeto>"; na árvore, a linha fica no topo do grupo, antes das issues. O menu ⋯ tem **Abrir repositório no GitHub** (sem Abrir PR nem Abrir item no projeto).
8. Conclua um work item com a aba aberta. **Esperado:** a aba não fecha sozinha.

## 11. Atualizações

Pré-requisito: um commit novo no `update_ref` do ambiente (padrão `main`). Para testar sem mexer em `main`, use `update_ref = "develop"` no `~/.config/ctower/config.toml` da distro e rode `ctower restart`.

1. Em **Configurações → Atualizações**, clique **Verificar agora**. **Esperado:** cada ambiente mostra a versão e o commit instalados e "X · N commit(s) novo(s)". O detalhe **Commits novos** lista os commits.
2. Sem sessões vivas, clique **Atualizar** no ambiente e confirme. **Esperado:** a linha passa por "buscando o código…", "compilando…" e assim por diante. A primeira vez leva alguns minutos, porque é um build completo. Depois aparece "reconectando…" e, sem você fazer nada, "atualizado para X · <commit>". Na distro, `ctower status` mostra a versão nova.
3. Com uma sessão viva, clique **Atualizar**. **Esperado:** a confirmação avisa que "1 sessão(ões) viva(s) serão encerradas" e o botão vira **Atualizar e encerrar sessões**. Cancele. No terminal da distro, `ctower update` recusa e diz quantas sessões há.
4. Falha: na distro, troque `update_ref` para um ref que não compila (ou que não existe) e atualize. **Esperado:** a linha mostra "falhou: …" com o **Fim do log**, e o daemon segue na versão anterior, conectado.
5. Rollback: em **Configurações → Ambiente**, **Reverter** e confirme. **Esperado:** volta à versão anterior e a UI reconecta. Com sessão viva, a confirmação avisa e o botão vira **Reverter e encerrar sessões**.
5a. Em **Configurações → Ambiente**, troque o **Canal** (`update_ref`) para `develop` e o **Intervalo de sincronização** para 60. **Esperado:** aplica sem restart (o `config.toml` da distro mostra os valores). Intervalo 10 é recusado na hora; **Restaurar padrão** volta a `main`.
6. App: com uma release mais nova que a instalada, abra o app. **Esperado:** "App X disponível" no cabeçalho. Em **Atualizações → Atualizar o app → Atualizar e reiniciar**, o download mostra a porcentagem, o instalador roda sem perguntas e o app reabre na versão nova.
7. Incompatível: com um daemon de protocolo major diferente, **Esperado:** o aviso no topo oferece **Atualizar <ambiente>** (daemon defasado) ou **Atualizar o app** (app defasado).

## 12. Gestor de projetos

Pré-requisito: um ambiente ligado ao hub e com o token de agente gravado (`cthubd agent-token <nome>` no hub e `ctower hub set-agent-token` no ambiente; `provision.sh --hub --env` já faz os dois). O hub precisa ter pelo menos um projeto com issues no espelho do GitHub.

1. Na tela **Hub**, clique **Gestor de projetos**. **Esperado:** o diálogo lista só os ambientes com token de agente. Sem nenhum, ele explica como gravar o token e **▶ Iniciar gestor** fica desabilitado.
2. Escolha **Um projeto** e o projeto. **Esperado:** o prompt muda para "Foco: owner/repo…" e cita o ambiente. Edite o prompt e troque o escopo de novo: o seu texto não é sobrescrito.
3. **▶ Iniciar gestor**. **Esperado:** abre a aba do terminal com o rótulo **Gestor de projetos**. A barra da sessão mostra a pasta (`~/.local/share/ctower/gestor`) e o botão **Revisar decisões do gestor**, sem **Abrir PR**. Na **Sala de controle**, o tile diz "Gestor de projetos · hub inteiro". De volta à tela Hub, o botão virou **Abrir gestor · <ambiente>**.
4. Peça ao gestor "proponha P0 para a issue N de owner/repo". **Esperado:** ele usa `portfolio_propose`. Na tela Hub, **Propostas do gestor** mostra a proposta em até alguns segundos, com o badge de contagem, "prioridade: não triado → P0", a justificativa, o ambiente e a idade. Nada muda no projeto antes da aprovação.
5. **Aprovar**. **Esperado:** "Proposta #N aprovada." A proposta vai para **Decididas e obsoletas**. Na tela Projeto, o item ganha o selo `P0` na árvore e no Kanban e vai para o topo da coluna. No painel do item, a linha do tempo mostra `priority_set` e a seção de decisões traz a decisão orientada "Proposta #N aprovada…".
6. Peça duas propostas e, antes de decidir, mude à mão a prioridade de um dos alvos (painel do item → **Prioridade**). **Esperado:** essa proposta some das pendentes e aparece como obsoleta, com "valor atual mudou…". Marque a outra, escreva uma nota e clique **Rejeitar marcadas (1)**. **Esperado:** a contagem volta a 0 e nada é aplicado.
7. Peça ao gestor para fechar uma issue ou comentar num PR. **Esperado:** o comando é bloqueado ("o gestor não faz `gh issue close`…"). Peça para criar uma issue. **Esperado:** o Claude Code pede a sua confirmação no terminal antes de rodar `gh issue create`.
8. **Saúde e riscos** na tela Hub: com um item bloqueado há mais de 2 dias, ou um PR aberto parado há mais de 3, **Esperado:** o sinal aparece com a gravidade e o limiar, e o link leva ao projeto. **mostrar informativos** traz os informativos (ex.: issues não triadas). Os cards de **Projetos** mostram a contagem de críticos e avisos.
9. No índice de **Projetos**, arraste um projeto ou use **Alt+↑**/**Alt+↓** na linha para trocar a ordem de dois projetos (passo 3.7). **Esperado:** a lista muda de ordem. No ranking, a classe vem primeiro: um P1 do projeto de cima vem depois de qualquer P0 do de baixo.
10. No painel de um item com prioridade, **Prioridade → Subir na ordem de P1**. **Esperado:** o item troca de lugar com o vizinho da mesma classe no Kanban. Na árvore, **Ordenar → prioridade** mostra a mesma ordem.
11. Feche o app com o gestor vivo e reabra. **Esperado:** a sessão segue; se o ambiente cair, a recuperação retoma o gestor na mesma pasta, com os mesmos limites.
12. Em **Configurações → Templates**, edite **Gestor de projetos**, salve e abra o diálogo do gestor de novo. **Esperado:** o prompt vem do template editado, com `{{scope}}` e `{{env}}` trocados.
13. Com o gestor tendo registrado ao menos duas decisões autônomas, clique **Revisar decisões do gestor**. **Esperado:** a tela diz "O gestor não abre PR…", não tem **Abrir PR** nem **Comentar no PR**, e **Marcar as N como revisadas** está habilitado.
14. Em uma decisão, clique **Revisada**. **Esperado:** só essa sai da lista e o contador da barra diminui em 1.
15. Clique **Marcar … como revisadas** para o resto. **Esperado:** a lista fica vazia ("Nenhuma decisão pendente"). Com o hub conectado, a revisão chega a ele sem número de PR.
16. Abra a revisão de uma issue comum sem PR no espelho. **Esperado:** **Marcar** continua desabilitado.

## 13. Docs dos projetos (épico #445)

Pré-requisito: hub 1.9 ou maior com `docs_dir` configurado (o `cthubd` cria o marcador `.ct-docs-root`), um ambiente WSL 2.8 ou maior com `docs_root` apontado para o compartilhamento montado (veja "Docs dos projetos" em `docs/hub.md`) e um projeto atribuído a ele, com pelo menos um épico no espelho.

1. Na tela **Projeto** (ou **⚙** na linha do projeto no índice de **Projetos**), abra **Configurações › Docs** e clique **Editar docs** (ou, em **Hub → Projetos**, abra o projeto e clique **Editar**, que edita tudo junto). **Esperado:** o grupo **Docs do projeto (fora do repositório)** mostra **Pasta de docs do projeto** desmarcada e as listas de pastas desabilitadas. Marque, escreva `analises` e `specs/telas` (uma por linha) em **Pastas do projeto** e `prompts` em **Pastas de cada épico**, e salve. **Esperado:** o detalhe mostra "Docs: ligados · _projeto: analises, specs/telas · épico: prompts" e o caminho `<docs_dir>/<owner>__<repo>`. Editar só o nome não mexe nos docs.
2. Com um hub anterior (1.6 ou menor), abra **Editar**. **Esperado:** no lugar dos campos, o aviso "Este hub não tem docs dos projetos…", e salvar funciona sem mandar `docs`.
3. Na tela **Projeto**, clique **Abrir pasta de docs** no cabeçalho. **Esperado:** o Explorador do Windows abre em `\\wsl.localhost\<distro>\<docs_root>\<owner>__<repo>`, com `_projeto\analises` e `_projeto\specs\telas` criadas.
4. Na árvore, clique com o botão direito num épico e escolha **Abrir pasta de docs do épico**. **Esperado:** abre `epicos\<n>-<slug>` com o `README.md` e `prompts`. Renomeie a pasta para `<n>-outro-nome` e repita: abre a pasta renomeada (não cria outra). Numa sub-issue, o menu não tem a ação.
4a. No Mac, com o `ctowerd` local (LaunchAgent, `docs/ambiente-macos.md`, registrado no app como remoto em `ws://127.0.0.1:7878/ws`) e `docs_root = "/Volumes/ctdocs"`, clique **Abrir pasta de docs**. **Esperado:** o Finder abre `/Volumes/ctdocs/<owner>__<repo>`, e o do épico abre `epicos/<n>-<slug>`. Um ambiente remoto apontado para outro host (ex.: `ws://mac-mini:7878/ws`) deixa o botão desabilitado.
5. Desligue os docs do projeto no hub e espere o pull do ambiente. **Esperado:** **Abrir pasta de docs** fica desabilitado, e passar o mouse mostra "Docs desligados neste projeto…". Com um ambiente remoto de verdade só (URL de outra máquina), a dica diz que só ambientes desta máquina abrem a pasta; sem `docs_root` no ambiente, "docs_root não configurado…".
6. Derrube a montagem (ou renomeie o `.ct-docs-root`). **Esperado:** em até um ciclo do `env.info`, o botão fica desabilitado com "docs_root … inacessível…", o ambiente ganha o selo **docs inacessíveis** no painel lateral, e o aviso "<ambiente>: pasta de docs inacessível" aparece na faixa **Precisa de você** e na aba **Avisos** do painel inferior. **Ver documentação** abre a seção "Docs dos projetos" do `docs/hub.md` no navegador. Restaure a montagem: o aviso some.
7. Em **Manutenção → Backup e snapshots**, veja o bloco **Docs dos projetos**. **Esperado:** a pasta dos docs, o último pacote (há quanto tempo, nome e tamanho), o último envio remoto, a agenda, a retenção e o limite.
8. Clique **Fazer backup agora** no bloco dos docs. **Esperado:** "Backup dos docs feito." e o último pacote atualizado; o **Fazer backup agora** do topo roda só o hub.db. Troque **Pacotes dos docs mantidos** para 10 e salve. **Esperado:** "Configuração do backup dos docs salva." e a retenção mostra 10. Limite 5000 MB não deixa salvar.
9. Com `docs_backup_max_mb` abaixo do tamanho dos docs, rode o backup dos docs. **Esperado:** "Backup dos docs não rodou…", a linha **Última rodada** com o tamanho, e o aviso "Docs acima do limite do backup" no bloco e na faixa **Precisa de você** (com **Configurar backup** levando ao bloco).
10. Na PWA (celular), abra **Hub → Visão geral**. **Esperado:** o cartão **Backup dos docs** com o mesmo estado, só leitura, sem botões; **Abrir pasta de docs** não aparece na tela Projeto.

## 14. Ver issue (#434)

Pré-requisito: hub 1.10 ou maior; para projeto Jira, o ambiente de sync 2.9 ou maior e online.

1. Na tela **Projeto**, abra uma issue do GitHub com descrição (títulos, checklist, código, tabela, imagem). Clique **Ver issue**, ao lado de **Abrir issue no GitHub ↗**. **Esperado:** a modal com o número, o título e a descrição renderizada; checklist só de leitura; segue o tema claro/escuro.
2. Na descrição, clique numa referência `#n` do mesmo repositório. **Esperado:** a modal fecha e a aba daquela issue abre no app. Clique num link externo: abre no navegador.
3. Feche com **Esc**, de novo com **✕** e com clique fora. Edite a descrição no GitHub e abra de novo. **Esperado:** a versão nova (nada fica guardado entre aberturas).
4. Numa issue sem descrição. **Esperado:** "Sem descrição.".
5. Num projeto Jira + GitLab, **Ver issue** numa issue com wiki markup (títulos `h2.`, `*negrito*`, `{code}`, tabela `||`). **Esperado:** convertido para Markdown; chaves `PROJ-n` do projeto abrem a aba do item. Pare o `ctowerd` do ambiente de sync e tente de novo: **Esperado:** "ambiente offline…" e o botão **Abrir no Jira ↗**.
6. Com um hub 1.9, passe o mouse em **Ver issue**. **Esperado:** desabilitado com "Atualize o hub…". Com o hub fora do ar (tela local), desabilitado com "…fora do ar".
7. Na PWA, repita o passo 1. **Esperado:** o mesmo comportamento.

