# Changelog

## [1.11.1](https://github.com/deep-ion-ai/ctower-agents/compare/v1.11.0...v1.11.1) (2026-11-27)


### Bug Fixes

* **agent:** imports que faltaram na extração do ClaudeRuntime ([b889df6](https://github.com/deep-ion-ai/ctower-agents/commit/b889df699a94c3520335bc57a897d96dc717a3f7))
* **hub:** execução de épico ponta a ponta — wip, token de agente, retomada e tela ([#341](https://github.com/deep-ion-ai/ctower-agents/issues/341)) ([c3c978b](https://github.com/deep-ion-ai/ctower-agents/commit/c3c978b26079ab7fea8aac4970003b4ee80d884b))
* **hub:** execução de épico ponta a ponta — wip, token de agente, retomada e tela ([#341](https://github.com/deep-ion-ai/ctower-agents/issues/341)) ([6451c5f](https://github.com/deep-ion-ai/ctower-agents/commit/6451c5f46d51b3f6b3486087f4ce015e4688c009))

## [1.11.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.10.0...v1.11.0) (2026-11-27)


### Features

* **core:** modelo Connection, CredentialSource e descritores de provider ([#234](https://github.com/deep-ion-ai/ctower-agents/issues/234)) ([#254](https://github.com/deep-ion-ai/ctower-agents/issues/254)) ([80078b9](https://github.com/deep-ion-ai/ctower-agents/commit/80078b96719c46181ada5a515437486d3700eb6b))
* **credentials:** fontes Secret e File no resolvedor do ambiente ([#274](https://github.com/deep-ion-ai/ctower-agents/issues/274)) ([#307](https://github.com/deep-ion-ai/ctower-agents/issues/307)) ([efde998](https://github.com/deep-ion-ai/ctower-agents/commit/efde998a6cd12d460ef3d8ec0048fa2b2e8fad90))
* **env:** sobrescrita por conexão editável e migração do github.token ([#276](https://github.com/deep-ion-ai/ctower-agents/issues/276)) ([#311](https://github.com/deep-ion-ai/ctower-agents/issues/311)) ([28df15a](https://github.com/deep-ion-ai/ctower-agents/commit/28df15aafa58fc661e068f157041d38df42fe541))
* **gh:** ct-gh usa o token da conexão github no hub e no ambiente ([#275](https://github.com/deep-ion-ai/ctower-agents/issues/275)) ([#308](https://github.com/deep-ion-ai/ctower-agents/issues/308)) ([2752743](https://github.com/deep-ion-ai/ctower-agents/commit/275274312df96b260735c64d21783f4f76d0719f))
* **hub-store:** execução de épico, nós e versão do plano ([#296](https://github.com/deep-ion-ai/ctower-agents/issues/296)) ([d4bd48e](https://github.com/deep-ion-ai/ctower-agents/commit/d4bd48e2259c13e9895a081e213738cded325d86))
* **hub-store:** impressão digital congelada, rebase pendente e reagendamento ([#296](https://github.com/deep-ion-ai/ctower-agents/issues/296)) ([21be509](https://github.com/deep-ion-ai/ctower-agents/commit/21be50985885426a7ed27f54354dcc49b2438390))
* **hub-store:** sessão de plano em andamento na execução ([#296](https://github.com/deep-ion-ai/ctower-agents/issues/296)) ([d679f2f](https://github.com/deep-ion-ai/ctower-agents/commit/d679f2f80d9f4d1a0386f93d8a7e9ced729acac6))
* **hub:** cadastro de conexões e referência no projeto ([#235](https://github.com/deep-ion-ai/ctower-agents/issues/235)) ([#256](https://github.com/deep-ion-ai/ctower-agents/issues/256)) ([cbf96f6](https://github.com/deep-ion-ai/ctower-agents/commit/cbf96f6d9ff827d81ed2264731bffbe5b5aa5a6c))
* **hub:** decisão pura do motor de execução de épico ([#297](https://github.com/deep-ion-ai/ctower-agents/issues/297)) ([499faef](https://github.com/deep-ion-ai/ctower-agents/commit/499faef9fbba4848985854412e01f91b8625a49f))
* **hub:** handshake de versão no canal reverso e no push (versão igual obrigatória) ([#250](https://github.com/deep-ion-ai/ctower-agents/issues/250)) ([afc8e63](https://github.com/deep-ion-ai/ctower-agents/commit/afc8e63a5c843861fb842dc710f2f990c2d9fade))
* **hub:** migração dos cadastros para conexões ([#237](https://github.com/deep-ion-ai/ctower-agents/issues/237)) ([#259](https://github.com/deep-ion-ai/ctower-agents/issues/259)) ([4746875](https://github.com/deep-ion-ai/ctower-agents/commit/474687512c2dc3363cfc7b254c81a21128cba65d))
* **hub:** projeto GitHub referencia a conexão github ([#275](https://github.com/deep-ion-ai/ctower-agents/issues/275)) ([#309](https://github.com/deep-ion-ai/ctower-agents/issues/309)) ([a7878db](https://github.com/deep-ion-ai/ctower-agents/commit/a7878db874834cfb80667a80ed47b3628420abba))
* **hub:** registro tipado de métodos RPC do hub ([#229](https://github.com/deep-ion-ai/ctower-agents/issues/229)) ([#251](https://github.com/deep-ion-ai/ctower-agents/issues/251)) ([a97713e](https://github.com/deep-ion-ai/ctower-agents/commit/a97713ebc5d20afe0d14b82c797117ade943fb3b))
* **hub:** runner e RPC da execução de épico ([#298](https://github.com/deep-ion-ai/ctower-agents/issues/298)) ([36e23a3](https://github.com/deep-ion-ai/ctower-agents/commit/36e23a39f593b238ccb86adf89aa4493189af73d))
* **hub:** sub-issue em execução de épico fora do piloto automático ([#303](https://github.com/deep-ion-ai/ctower-agents/issues/303)) ([94cc1b1](https://github.com/deep-ion-ai/ctower-agents/commit/94cc1b1e144e7dfb0392045fa5a16b764609283b))
* **protocol:** registro tipado de métodos RPC do ambiente ([#248](https://github.com/deep-ion-ai/ctower-agents/issues/248)) ([70f442b](https://github.com/deep-ion-ai/ctower-agents/commit/70f442b688a82267f5aec3dbabde852a78950588))
* **sessions,hub:** pedidos de decisão e replanejamento do épico ([#300](https://github.com/deep-ion-ai/ctower-agents/issues/300)) ([babd00f](https://github.com/deep-ion-ai/ctower-agents/commit/babd00f29dfbb38ff18fd99301ba14429940b06d))
* **sessions,hub:** sessão de integração do épico com PR que fecha as issues ([#302](https://github.com/deep-ion-ai/ctower-agents/issues/302)) ([26b1412](https://github.com/deep-ion-ai/ctower-agents/commit/26b1412cff0fe808843983cba9447a62c2010043))
* **sessions:** resolvedor de credencial com cache e mascaramento ([#236](https://github.com/deep-ion-ai/ctower-agents/issues/236)) ([#257](https://github.com/deep-ion-ai/ctower-agents/issues/257)) ([6e142bc](https://github.com/deep-ion-ai/ctower-agents/commit/6e142bc941dc412f307f596ffa55db1639d21b40))
* **sessions:** sessão de plano do épico ([#299](https://github.com/deep-ion-ai/ctower-agents/issues/299)) ([a0a377b](https://github.com/deep-ion-ai/ctower-agents/commit/a0a377b1422cd1fb3d0b2ab7230babfa542a003a))
* **sessions:** sub-issue assumida pelo desenvolvedor na execução de épico ([#301](https://github.com/deep-ion-ai/ctower-agents/issues/301)) ([c66b7da](https://github.com/deep-ion-ai/ctower-agents/commit/c66b7da000ffd266c7ca337cbb5c19a89901aec2))
* **sessions:** sub-issue da execução de épico sai da wip e worktree.remove ([#298](https://github.com/deep-ion-ai/ctower-agents/issues/298)) ([32288b1](https://github.com/deep-ion-ai/ctower-agents/commit/32288b19f601504192f09dbce08907b587103a45))
* **tauri:** allowlist de métodos expostos à WebView ([#231](https://github.com/deep-ion-ai/ctower-agents/issues/231)) ([#252](https://github.com/deep-ion-ai/ctower-agents/issues/252)) ([39c10e8](https://github.com/deep-ion-ai/ctower-agents/commit/39c10e863fba1f7ccd5cea867ec0a7ffd135d56e))
* **ui:** credenciais das conexões no ambiente e remoção do GithubSettings ([#276](https://github.com/deep-ion-ai/ctower-agents/issues/276)) ([#314](https://github.com/deep-ion-ai/ctower-agents/issues/314)) ([2efe700](https://github.com/deep-ion-ai/ctower-agents/commit/2efe700d20696ea2472922313854456bececaa9b))
* **ui:** faixa de primeiros passos sem ambiente nem hub ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([d25201f](https://github.com/deep-ion-ai/ctower-agents/commit/d25201f6d737f8186016535326615a8323c5c22e))
* **ui:** painel de execução de épico ([#304](https://github.com/deep-ion-ai/ctower-agents/issues/304)) ([ba30b63](https://github.com/deep-ion-ai/ctower-agents/commit/ba30b63c1ae0fd8d303c21c16e761ae3034d7bf3))
* **ui:** tela de Conexões, seleção no projeto e credencial por ambiente ([#238](https://github.com/deep-ion-ai/ctower-agents/issues/238)) ([#258](https://github.com/deep-ion-ai/ctower-agents/issues/258)) ([5ee4d65](https://github.com/deep-ion-ai/ctower-agents/commit/5ee4d65f802e01bb893eef9abd6c84d475e1dc82))


### Bug Fixes

* correções da revisão do épico [#224](https://github.com/deep-ion-ai/ctower-agents/issues/224) ([#262](https://github.com/deep-ion-ai/ctower-agents/issues/262)) ([ce887d6](https://github.com/deep-ion-ai/ctower-agents/commit/ce887d6a84b30e98098d0f3739e388e42ae41709))
* **credentials:** erro de token diz a conexão, quem leu e o que fazer ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([fa5e316](https://github.com/deep-ion-ai/ctower-agents/commit/fa5e316c6316a27671d67ede55215f04739f3a5f))
* **gitlab:** clone por HTTPS usa o token da conexão do GitLab ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([a6e8db9](https://github.com/deep-ion-ai/ctower-agents/commit/a6e8db94aac555a002fc62a60f873718d22dc1c1))
* **hub:** "Testar" confere se o secret da conexão existe no store ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([135e6c1](https://github.com/deep-ion-ai/ctower-agents/commit/135e6c1a78fa7dd576f6d503695e3d0f76f06496))
* **hub:** host do GitLab colado como URL é gravado como host ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([20193da](https://github.com/deep-ion-ai/ctower-agents/commit/20193da6829ffbc1eee7bc926cb05a79f75675fd))
* **hub:** recusa renomear conexão em uso ([#271](https://github.com/deep-ion-ai/ctower-agents/issues/271)) ([#285](https://github.com/deep-ion-ai/ctower-agents/issues/285)) ([960e2e2](https://github.com/deep-ion-ai/ctower-agents/commit/960e2e29fe9c1f06b75cf4b2c2b3a6f7945c2ab4))
* **hub:** token do GitHub do hub pela conexão github em Hub › Conexões ([#263](https://github.com/deep-ion-ai/ctower-agents/issues/263)) ([#310](https://github.com/deep-ion-ai/ctower-agents/issues/310)) ([d8cae97](https://github.com/deep-ion-ai/ctower-agents/commit/d8cae97030797493f22214ab827e7757b278bfa6))
* **hub:** visão efetiva do projeto com a conexão no hub ([#264](https://github.com/deep-ion-ai/ctower-agents/issues/264)) ([#289](https://github.com/deep-ion-ai/ctower-agents/issues/289)) ([085f084](https://github.com/deep-ion-ai/ctower-agents/commit/085f0841889cd5506991f82477e29004ac195559))
* **jira,gitlab:** 403 não é mais credencial recusada ([#265](https://github.com/deep-ion-ai/ctower-agents/issues/265)) ([#287](https://github.com/deep-ion-ai/ctower-agents/issues/287)) ([7efdf0e](https://github.com/deep-ion-ai/ctower-agents/commit/7efdf0eb784a50bdfd6518d99327755c1928194b))
* **jira:** ordem dos erros da validação do Jira de antes da [#234](https://github.com/deep-ion-ai/ctower-agents/issues/234) ([#272](https://github.com/deep-ion-ai/ctower-agents/issues/272)) ([#283](https://github.com/deep-ion-ai/ctower-agents/issues/283)) ([e2feeba](https://github.com/deep-ion-ai/ctower-agents/commit/e2feebadbd65e919cfc752725f76a1ded94a22d8))
* **ledger:** sobrescrita de conexão sem projeto não é órfã ([#270](https://github.com/deep-ion-ai/ctower-agents/issues/270)) ([#291](https://github.com/deep-ion-ai/ctower-agents/issues/291)) ([2c0be4b](https://github.com/deep-ion-ai/ctower-agents/commit/2c0be4b4d1db50484c51261a7415130d36b26ff8))
* **proc:** prazo e modo não interativo em todo subprocesso ([#249](https://github.com/deep-ion-ai/ctower-agents/issues/249)) ([a4e9661](https://github.com/deep-ion-ai/ctower-agents/commit/a4e9661c414f028b8298f8fd30992f429fb9c0e3))
* **server:** config.toml não é regravado a cada pull quando sobra [credentials] ([#269](https://github.com/deep-ion-ai/ctower-agents/issues/269)) ([#290](https://github.com/deep-ion-ai/ctower-agents/issues/290)) ([6ff9d43](https://github.com/deep-ion-ai/ctower-agents/commit/6ff9d437ffd0a2737dfc3dd547b3518e82b902b3))
* **server:** conversão de [credentials] atualiza as credenciais em memória ([#267](https://github.com/deep-ion-ai/ctower-agents/issues/267)) ([#284](https://github.com/deep-ion-ai/ctower-agents/issues/284)) ([a9d6179](https://github.com/deep-ion-ai/ctower-agents/commit/a9d61799bfb1b2a38a574e53bf22c4dd607f16e2))
* **server:** máscara das sessões cobre o secret renovado antes do cache ([#320](https://github.com/deep-ion-ai/ctower-agents/issues/320)) ([#321](https://github.com/deep-ion-ai/ctower-agents/issues/321)) ([64db244](https://github.com/deep-ion-ai/ctower-agents/commit/64db2448f54cf7777198a3332709c4c9376744ac))
* **sessions:** abrir sessão não resolve a credencial das conexões ([#266](https://github.com/deep-ion-ai/ctower-agents/issues/266)) ([#286](https://github.com/deep-ion-ai/ctower-agents/issues/286)) ([0f8fb79](https://github.com/deep-ion-ai/ctower-agents/commit/0f8fb7948e0ac3fc8222afeed6d7a0799eca7998))
* **sessions:** encerrar a sessão do agente escala para SIGKILL ([#312](https://github.com/deep-ion-ai/ctower-agents/issues/312)) ([#315](https://github.com/deep-ion-ai/ctower-agents/issues/315)) ([76e2c75](https://github.com/deep-ion-ai/ctower-agents/commit/76e2c75723c5a712946c6b30dee0d41b05e47abb))
* **sessions:** epic_plan no registro do session.start_project ([#299](https://github.com/deep-ion-ai/ctower-agents/issues/299)) ([37021e3](https://github.com/deep-ion-ai/ctower-agents/commit/37021e3983f4a024072da25cefc299e48fc4e419))
* **sessions:** pré-aceitar a confiança na pasta de docs do projeto ([c770b8b](https://github.com/deep-ion-ai/ctower-agents/commit/c770b8b752abd499cc64db8bc7145b1e739a7948))
* **sessions:** pré-aceitar a confiança na pasta de docs do projeto ([58e7be8](https://github.com/deep-ion-ai/ctower-agents/commit/58e7be8f11af4d0aef1069bf10138d334b34d3fc)), closes [#222](https://github.com/deep-ion-ai/ctower-agents/issues/222)
* **sessions:** prontidão diz quando a credencial vem do cache ([#268](https://github.com/deep-ion-ai/ctower-agents/issues/268)) ([#292](https://github.com/deep-ion-ai/ctower-agents/issues/292)) ([21f239f](https://github.com/deep-ion-ai/ctower-agents/commit/21f239f597a322a89611bba1788f0d71d6354e45))
* **sessions:** shell.close escala para SIGKILL se o shell não sair ([#294](https://github.com/deep-ion-ai/ctower-agents/issues/294)) ([#306](https://github.com/deep-ion-ai/ctower-agents/issues/306)) ([909ce09](https://github.com/deep-ion-ai/ctower-agents/commit/909ce09bcde8394890064f4601a1e91551c3625c))
* **tauri:** liberar hub.connections.* na allowlist da ponte ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([532745a](https://github.com/deep-ion-ai/ctower-agents/commit/532745a997f5219c56f6a560d5bd15dfb1dac984))
* **tauri:** preservar o código de erro na ponte ([#247](https://github.com/deep-ion-ai/ctower-agents/issues/247)) ([b270687](https://github.com/deep-ion-ai/ctower-agents/commit/b270687aad7b69781bd5e1286592ad2d0ad1f5dd))
* textos de credencial sem "comando" nem "senha" fixos ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([414b5d7](https://github.com/deep-ion-ai/ctower-agents/commit/414b5d78401f72a31519fe0da65740a78d541a74))
* **ui:** reler o dashboard do ambiente em projects_changed ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([6adfc8c](https://github.com/deep-ion-ai/ctower-agents/commit/6adfc8c2f9f34c046aa930f1a4fa410c30d1857f))
* **ui:** secret do hub como fonte padrão e Excluir travado em uso ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([2600068](https://github.com/deep-ion-ai/ctower-agents/commit/260006804dfc83806ecb656ee87376e17e5dd3c8))
* **ui:** status do hub não fica em "conectando…" depois de configurar ([#281](https://github.com/deep-ion-ai/ctower-agents/issues/281)) ([d419130](https://github.com/deep-ion-ai/ctower-agents/commit/d419130a07d971702305b44aeb0a0b6495166a92))
* **ui:** sub-issues de sub-épico na árvore, na busca e no Kanban ([bc376e6](https://github.com/deep-ion-ai/ctower-agents/commit/bc376e6c8dc135f72fa17594f10d9c259ddbe964))
* **update:** não enviar o token do GitHub ao baixar releases ([#246](https://github.com/deep-ion-ai/ctower-agents/issues/246)) ([36045be](https://github.com/deep-ion-ai/ctower-agents/commit/36045be49eda9af7c851d785eaa3fffcc3fa2ec7))

## [1.10.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.9.0...v1.10.0) (2026-11-16)


### Features

* **hub:** análise automática de dependências antes das execuções do piloto ([4975ffe](https://github.com/deep-ion-ai/ctower-agents/commit/4975ffe0f9154fd495b97b37173af2331039da31))
* **hub:** análise automática de dependências antes das execuções do piloto ([b57afc8](https://github.com/deep-ion-ai/ctower-agents/commit/b57afc822680b211a71210b70f7ce42b05d0e48f)), closes [#215](https://github.com/deep-ion-ai/ctower-agents/issues/215)

## [1.9.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.8.0...v1.9.0) (2026-11-12)


### Features

* **core,hub:** política de orçamento do plano por conta ([80daf3c](https://github.com/deep-ion-ai/ctower-agents/commit/80daf3c97a326199524984a2052bb0671def63cd)), closes [#180](https://github.com/deep-ion-ai/ctower-agents/issues/180)
* **core,sessions:** prompt inicial da sessão autônoma montado no ambiente ([2a3af16](https://github.com/deep-ion-ai/ctower-agents/commit/2a3af1631733c92a09e0d94271aae4971a56caf3)), closes [#191](https://github.com/deep-ion-ai/ctower-agents/issues/191)
* **ct-core,ct-gh:** etapas determinísticas da release e perfil release do guarda-corpo ([1a695d8](https://github.com/deep-ion-ai/ctower-agents/commit/1a695d862dc4198662b9d85a6032ee6c0f255ca6)), closes [#204](https://github.com/deep-ion-ai/ctower-agents/issues/204)
* **ct-gh:** checks do head do PR no espelho ([a147cd0](https://github.com/deep-ion-ai/ctower-agents/commit/a147cd0b4dd8318498d12c53dd05bb33fbdc8145)), closes [#129](https://github.com/deep-ion-ai/ctower-agents/issues/129)
* **ct-gh:** threads de review não resolvidas no espelho ([4893a12](https://github.com/deep-ion-ai/ctower-agents/commit/4893a12911ffdc35ce9106e6e69b7242b4ddf662)), closes [#130](https://github.com/deep-ion-ai/ctower-agents/issues/130)
* **ct-server:** session.start_release e rotas do ctower release ([5583251](https://github.com/deep-ion-ai/ctower-agents/commit/55832516b9d4b134c6a8f315a9571d4e44555f4d)), closes [#204](https://github.com/deep-ion-ai/ctower-agents/issues/204)
* **ctower:** ctower release e perfil release no hook ([001e009](https://github.com/deep-ion-ai/ctower-agents/commit/001e009a3f8d93b4ea81a4d3fd87b81757874653)), closes [#204](https://github.com/deep-ion-ai/ctower-agents/issues/204)
* hub e ambiente de teste em Docker, com marca de teste na UI ([bc71a5d](https://github.com/deep-ion-ai/ctower-agents/commit/bc71a5df0f2876be4aa34a048c833bba6b472b97))
* hub e ambiente de teste em Docker, com marca de teste na UI ([2f56fca](https://github.com/deep-ion-ai/ctower-agents/commit/2f56fca0ba6ef8cd5f71f8e03bf0a73866c70bd4)), closes [#176](https://github.com/deep-ion-ai/ctower-agents/issues/176)
* **hub,ledger:** configuração de release no cadastro do projeto ([9db2c7f](https://github.com/deep-ion-ai/ctower-agents/commit/9db2c7f67f5b5a073552573d7720ec03a2318f2c)), closes [#204](https://github.com/deep-ion-ai/ctower-agents/issues/204)
* **hub,sessions:** agendador do piloto automático ([de9a379](https://github.com/deep-ion-ai/ctower-agents/commit/de9a379aca2a88ea5bff3d212e4939ccd6048a11)), closes [#183](https://github.com/deep-ion-ai/ctower-agents/issues/183)
* **hub,sessions:** modo autônomo no projeto e branch wip/autonoma ([647def5](https://github.com/deep-ion-ai/ctower-agents/commit/647def5e29c81f03e6fee42a6df30a43eb1c2ec2)), closes [#181](https://github.com/deep-ion-ai/ctower-agents/issues/181)
* **hub,sessions:** rótulo de aprovação por projeto, exigido para iniciar sessão ([4cef668](https://github.com/deep-ion-ai/ctower-agents/commit/4cef6682a0a66370c2bf73769fa10d79e1e84896)), closes [#124](https://github.com/deep-ion-ai/ctower-agents/issues/124)
* **hub,ui:** limite de trabalho em andamento por projeto ([dd1213c](https://github.com/deep-ion-ai/ctower-agents/commit/dd1213cf6122e76f390bc3d967e612474ca30689)), closes [#126](https://github.com/deep-ion-ai/ctower-agents/issues/126)
* **hub,ui:** modelo e esforço por issue, sugeridos pelo gestor ([191f7a4](https://github.com/deep-ion-ai/ctower-agents/commit/191f7a404621b85888c5a55c6eed33f45a19b057)), closes [#182](https://github.com/deep-ion-ai/ctower-agents/issues/182)
* **hub,ui:** painel do piloto automático e resumo da manhã ([7dfe21e](https://github.com/deep-ion-ai/ctower-agents/commit/7dfe21e26b25b8a5acc440238ab1a37044260adb)), closes [#185](https://github.com/deep-ion-ai/ctower-agents/issues/185)
* **hub:** hub.release.status para a prévia do Criar release ([1f52236](https://github.com/deep-ion-ai/ctower-agents/commit/1f52236ad12becf8ebeaf3d6077d4edfe643fed3)), closes [#204](https://github.com/deep-ion-ai/ctower-agents/issues/204)
* **ledger:** ferramentas MCP pr_feedback e pr_check_log ([7c4ce90](https://github.com/deep-ion-ai/ctower-agents/commit/7c4ce9006e8f120ad18c7ecf8a81cfe3f383c780)), closes [#133](https://github.com/deep-ion-ai/ctower-agents/issues/133)
* **packaging:** ambiente autônomo em container no Linux ([3439381](https://github.com/deep-ion-ai/ctower-agents/commit/3439381d7e1a14cb814b111b9c27d232c3ca7c9e)), closes [#179](https://github.com/deep-ion-ai/ctower-agents/issues/179)
* piloto automático por projeto, com orçamento do plano e branch wip/autonoma (épico [#178](https://github.com/deep-ion-ai/ctower-agents/issues/178)) ([ed0e102](https://github.com/deep-ion-ai/ctower-agents/commit/ed0e10205a481f27fc4fd55acf128926c08c08f1))
* **sessions,hub:** fechamento autônomo da issue na wip/autonoma ([41e8a4e](https://github.com/deep-ion-ai/ctower-agents/commit/41e8a4e9639a66629bb03bfe8b9295c846c861e8)), closes [#184](https://github.com/deep-ion-ai/ctower-agents/issues/184)
* **sessions,hub:** sessão de release com merges determinísticos (GitHub, release-please) ([09039f0](https://github.com/deep-ion-ai/ctower-agents/commit/09039f0e21eecda6848a25dc8169c2d6f205ca78))
* **sessions:** sessão de release com o token exclusivo e rotas do ctower release ([42dd48a](https://github.com/deep-ion-ai/ctower-agents/commit/42dd48a664e8e795249651f3ae657cb7fa6c5793)), closes [#204](https://github.com/deep-ion-ai/ctower-agents/issues/204)
* **ui:** Criar release, seção Release e template de release ([a95edfe](https://github.com/deep-ion-ai/ctower-agents/commit/a95edfe1c142f18364c485225e7b6281508818f7)), closes [#204](https://github.com/deep-ion-ai/ctower-agents/issues/204)
* **ui:** sessão de release como item sem issue nos rótulos e no Reiniciar ([8e0c97e](https://github.com/deep-ion-ai/ctower-agents/commit/8e0c97e4f3eebd5be9f42d103f6b5313a326abb7)), closes [#204](https://github.com/deep-ion-ai/ctower-agents/issues/204)
* visualizador de diff de código da sessão ([be96408](https://github.com/deep-ion-ai/ctower-agents/commit/be96408c9301d873ad211d24060179d2b84264fe))
* visualizador de diff de código da sessão ([ac50d7e](https://github.com/deep-ion-ai/ctower-agents/commit/ac50d7ee3e3cf2acaec821551445aa849d7b5972)), closes [#168](https://github.com/deep-ion-ai/ctower-agents/issues/168)


### Bug Fixes

* ambiente autônomo cala a oferta do auto mode do Claude Code ([c47308f](https://github.com/deep-ion-ai/ctower-agents/commit/c47308feae48906386d6c6e05d2b938c225571a8))
* ambiente autônomo no host x86 (systemd, glibc, mais de um dono) ([d592195](https://github.com/deep-ion-ai/ctower-agents/commit/d592195139ac152e66d20684e7ab412f452ab017)), closes [#179](https://github.com/deep-ion-ai/ctower-agents/issues/179) [#178](https://github.com/deep-ion-ai/ctower-agents/issues/178)
* **autonomo:** PR rascunho pendente sem commits e Closes #n no template ([233c897](https://github.com/deep-ion-ai/ctower-agents/commit/233c89710c8c083087cde5edeee2dd2b52dbf669))
* **hub,ui:** integra modelo e esforço com o modo autônomo ([89ab5b8](https://github.com/deep-ion-ai/ctower-agents/commit/89ab5b86138f56664f94cd3cfe5c6e6d904e1250)), closes [#182](https://github.com/deep-ion-ai/ctower-agents/issues/182)
* **hub,ui:** sonda sem leitura do plano e erro da wip bloqueando o piloto ([66eba4d](https://github.com/deep-ion-ai/ctower-agents/commit/66eba4d2c25a75eb26f4872dbc29e554f1826d4d))
* **hub:** piloto encerra a sessão autônoma ociosa e a unidade systemd acompanha o container ([19725f3](https://github.com/deep-ion-ai/ctower-agents/commit/19725f332e7d3ada4a24d855abcdff60f0beecba))
* **status:** pedido de atenção não volta issue em revisão para aguardando você ([ea25016](https://github.com/deep-ion-ai/ctower-agents/commit/ea25016826ee8214cc17851271fb8fcb632b8aee))
* **status:** pedido de atenção não volta issue em revisão para aguardando você ([14965b2](https://github.com/deep-ion-ai/ctower-agents/commit/14965b200e83e6a437617697c937253adecf45fd))

## [1.8.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.7.1...v1.8.0) (2026-11-10)


### Features

* **secrets,sessions:** ler o valor atual de um secret durante a sessão ([82eaa1c](https://github.com/deep-ion-ai/ctower-agents/commit/82eaa1ce279cb0ba95bf89d17f9bca309b72fbac))
* **secrets,sessions:** ler o valor atual de um secret durante a sessão ([49597c7](https://github.com/deep-ion-ai/ctower-agents/commit/49597c7d602fe19cbb4aa0b3dc5ce3dec93feeca)), closes [#165](https://github.com/deep-ion-ai/ctower-agents/issues/165)
* templates de prompt inicial configuráveis por projeto ([#169](https://github.com/deep-ion-ai/ctower-agents/issues/169)) ([cba7ec0](https://github.com/deep-ion-ai/ctower-agents/commit/cba7ec01583f40cbb275ecd72390c8e0a1542173)), closes [#166](https://github.com/deep-ion-ai/ctower-agents/issues/166)
* visualizador de markdown dos arquivos da sessão ([#171](https://github.com/deep-ion-ai/ctower-agents/issues/171)) ([6f1ddf6](https://github.com/deep-ion-ai/ctower-agents/commit/6f1ddf672b913c1b66d166aba9b138f64a3171b3)), closes [#167](https://github.com/deep-ion-ai/ctower-agents/issues/167)

## [1.7.1](https://github.com/deep-ion-ai/ctower-agents/compare/v1.7.0...v1.7.1) (2026-11-09)


### Bug Fixes

* **ui:** "Recuperar" na Sala de controle abre a sessão clicada ([3b97924](https://github.com/deep-ion-ai/ctower-agents/commit/3b97924044b0277d7041e76c0399e5a0afcd61e3))
* **ui:** "Recuperar" na Sala de controle abre a sessão clicada ([c51c852](https://github.com/deep-ion-ai/ctower-agents/commit/c51c8528146bfc7ffe0fcf79203f18d64ec85260)), closes [#160](https://github.com/deep-ion-ai/ctower-agents/issues/160)

## [1.7.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.6.0...v1.7.0) (2026-11-05)


### Features

* **jira,sessions,hub:** adicionar issue do Jira pelo ID ([d1cead3](https://github.com/deep-ion-ai/ctower-agents/commit/d1cead3926803ec685bb02eabef973f75caceced)), closes [#148](https://github.com/deep-ion-ai/ctower-agents/issues/148)
* **ledger,jira,gh:** responsável identificável na issue e "eu" do rastreador ([#149](https://github.com/deep-ion-ai/ctower-agents/issues/149)) ([387148a](https://github.com/deep-ion-ai/ctower-agents/commit/387148aaec6f1cff4ca9c2f553d68284a546cda9))
* **ui:** "Adicionar issue pelo ID" na tela do projeto e na paleta ([04e494d](https://github.com/deep-ion-ai/ctower-agents/commit/04e494d5072701fe465e8a77220443a44798e8cc)), closes [#150](https://github.com/deep-ion-ai/ctower-agents/issues/150)
* **ui:** filtro "Só minhas" na Árvore e no Quadro ([ee46e23](https://github.com/deep-ion-ai/ctower-agents/commit/ee46e238418d5bfb235f627f044f306a4053c69c)), closes [#151](https://github.com/deep-ion-ai/ctower-agents/issues/151)

## [1.6.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.5.0...v1.6.0) (2026-11-03)


### Features

* **ui:** descartar uma sessão interrompida sem retomá-la ([#138](https://github.com/deep-ion-ai/ctower-agents/issues/138)) ([ea7345a](https://github.com/deep-ion-ai/ctower-agents/commit/ea7345a74002d463861924776b69ce0888f69d94))
* **ui:** descartar uma sessão interrompida sem retomá-la ([#138](https://github.com/deep-ion-ai/ctower-agents/issues/138)) ([1a4932c](https://github.com/deep-ion-ai/ctower-agents/commit/1a4932c3c2c995db677355142e0387193edc524f))
* **ui:** propostas do gestor em lote e largura ajustável na Visão geral ([#139](https://github.com/deep-ion-ai/ctower-agents/issues/139)) ([e59d166](https://github.com/deep-ion-ai/ctower-agents/commit/e59d16656921ab5295581442c70a1e78df9314e4))
* **ui:** propostas do gestor em lote e largura ajustável na Visão geral ([#139](https://github.com/deep-ion-ai/ctower-agents/issues/139)) ([85a55c6](https://github.com/deep-ion-ai/ctower-agents/commit/85a55c61a3fce59ea46af461c2a1b7a33f374faa))


### Bug Fixes

* **jira:** credencial do tracker por ambiente e épico garantido nas sessões de issue ([#143](https://github.com/deep-ion-ai/ctower-agents/issues/143)) ([ebbecf0](https://github.com/deep-ion-ai/ctower-agents/commit/ebbecf0f2cc454bcb55b06f0b8deea49dfd3bd1a))
* **jira:** credencial do tracker por ambiente e épico garantido nas sessões de issue ([#143](https://github.com/deep-ion-ai/ctower-agents/issues/143)) ([733cf73](https://github.com/deep-ion-ai/ctower-agents/commit/733cf73ee837d3566868cffaf05c205d4fa815f5))

## [1.5.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.4.0...v1.5.0) (2026-11-02)


### Features

* **ui:** cartão Propostas do gestor na Visão geral do projeto ([#100](https://github.com/deep-ion-ai/ctower-agents/issues/100)) ([#103](https://github.com/deep-ion-ai/ctower-agents/issues/103)) ([552f85f](https://github.com/deep-ion-ai/ctower-agents/commit/552f85f6310a7354798c98eb7b138ae799be450e))
* **ui:** cartão Revisões na Visão geral do projeto ([#101](https://github.com/deep-ion-ai/ctower-agents/issues/101)) ([#104](https://github.com/deep-ion-ai/ctower-agents/issues/104)) ([84f745c](https://github.com/deep-ion-ai/ctower-agents/commit/84f745cda3aec7f78d2c324ca7c174e1304ac2b9))
* **ui:** Visão geral como aba padrão do projeto ([#99](https://github.com/deep-ion-ai/ctower-agents/issues/99)) ([#102](https://github.com/deep-ion-ai/ctower-agents/issues/102)) ([735a515](https://github.com/deep-ion-ai/ctower-agents/commit/735a515707c6a53b8bd8a0ce5c9bed234d518fcc))
* **ui:** Visão geral como aba padrão do projeto, com propostas do gestor e revisões ([#98](https://github.com/deep-ion-ai/ctower-agents/issues/98)) ([638512c](https://github.com/deep-ion-ai/ctower-agents/commit/638512cca53ccfa61620cddf563c9955439f358a))

## [1.4.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.3.0...v1.4.0) (2026-10-31)


### Features

* **ui:** cores de texto e fundo da aba por tipo e por tema e ícone na aba de sessão ([6a28396](https://github.com/deep-ion-ai/ctower-agents/commit/6a283963d56f34dc813784ef9f8cffe19f681b16))
* **ui:** cores de texto e fundo da aba por tipo e por tema e ícone na aba de sessão ([40746d4](https://github.com/deep-ion-ai/ctower-agents/commit/40746d4dcdbb6979e7f55a74c256a2469e69310f)), closes [#73](https://github.com/deep-ion-ai/ctower-agents/issues/73)
* **ui:** sincronizar um projeto de vários lugares e mostrar o erro de sync por projeto ([e937410](https://github.com/deep-ion-ai/ctower-agents/commit/e9374108d2f699670bbebf2ab82390ba43eb27e5))
* **ui:** sincronizar um projeto de vários lugares e mostrar o erro de sync por projeto ([#90](https://github.com/deep-ion-ai/ctower-agents/issues/90)) ([539b703](https://github.com/deep-ion-ai/ctower-agents/commit/539b703c26b47c4dcec387dbdf27637f78657e33))


### Bug Fixes

* **ct-gh:** issue apagada ou transferida não trava mais o sync ([#89](https://github.com/deep-ion-ai/ctower-agents/issues/89)) ([84a27c0](https://github.com/deep-ion-ai/ctower-agents/commit/84a27c01ca7a2dba877b9f1a4200105f2616eee4))
* **ct-gh:** issue apagada ou transferida não trava mais o sync ([#89](https://github.com/deep-ion-ai/ctower-agents/issues/89)) ([ed57903](https://github.com/deep-ion-ai/ctower-agents/commit/ed57903ece94275da75a619542a2d8a946a7c303))

## [1.3.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.2.0...v1.3.0) (2026-10-31)


### Features

* tela Logs com os logs do hub, dos ambientes e da própria UI ([#65](https://github.com/deep-ion-ai/ctower-agents/issues/65)) ([725f002](https://github.com/deep-ion-ai/ctower-agents/commit/725f0025245f2af4c1e30ff56f533f9948f071d6))
* tela Logs com os logs do hub, dos ambientes e da própria UI ([#65](https://github.com/deep-ion-ai/ctower-agents/issues/65)) ([89d054e](https://github.com/deep-ion-ai/ctower-agents/commit/89d054e0fa1877be4f22f7f29bf21271d9399f08))
* **ui:** ações só do desktop aparecem desabilitadas com explicação na web ([6278804](https://github.com/deep-ion-ai/ctower-agents/commit/6278804e2cf1c95519de8bfc895d80987a810f0e))
* **ui:** ações só do desktop aparecem desabilitadas com explicação na web ([cfe95f0](https://github.com/deep-ion-ai/ctower-agents/commit/cfe95f0ca36c592683da8c6a4a711a7adbc86e4e)), closes [#26](https://github.com/deep-ion-ai/ctower-agents/issues/26)
* **ui:** folha de atalhos (?) e dicas de gestos ([8541c6c](https://github.com/deep-ion-ai/ctower-agents/commit/8541c6cbe17813d693d9e8b468003841174e033f))
* **ui:** folha de atalhos (?) e dicas de gestos ([067ecdd](https://github.com/deep-ion-ai/ctower-agents/commit/067ecddde9d6a97bc464cb99bb04d2a268b89bd1)), closes [#27](https://github.com/deep-ion-ai/ctower-agents/issues/27)
* **ui:** índice e itens do projeto no conteúdo principal no celular ([8a37311](https://github.com/deep-ion-ai/ctower-agents/commit/8a373117ecb22aa398b200e19504a55de09eafa8))
* **ui:** índice e itens do projeto no conteúdo principal no celular ([fdb331b](https://github.com/deep-ion-ai/ctower-agents/commit/fdb331b19d1e6df57327f491d8d1b37f9923efea)), closes [#25](https://github.com/deep-ion-ai/ctower-agents/issues/25)
* **ui:** tela de detalhe do ambiente ([89070de](https://github.com/deep-ion-ai/ctower-agents/commit/89070de9e64cd2e4e9928ff761b81fac449c9493))
* **ui:** tela de detalhe do ambiente ([2e06ca0](https://github.com/deep-ion-ai/ctower-agents/commit/2e06ca081f0958b5cbead94c8a987ded1481d536)), closes [#23](https://github.com/deep-ion-ai/ctower-agents/issues/23)


### Bug Fixes

* **ui:** nomes distintos para as ações de "Sincronizar" ([cfe6999](https://github.com/deep-ion-ai/ctower-agents/commit/cfe699925d3cff00ceaee49cca7309ad9d6b421f))
* **ui:** nomes distintos para as ações de "Sincronizar" ([0bf8ce1](https://github.com/deep-ion-ai/ctower-agents/commit/0bf8ce196190de728ac2bb31d38179614a461b9b))
* **ui:** renomeia Shortcuts.tsx para ShortcutsSheet.tsx e barra colisões de caixa no CI ([#63](https://github.com/deep-ion-ai/ctower-agents/issues/63)) ([713a034](https://github.com/deep-ion-ai/ctower-agents/commit/713a034067914c5dd9ce6474f8f3e4787aa54f2d))
* **ui:** renomeia Shortcuts.tsx para ShortcutsSheet.tsx e barra colisões de caixa no CI ([#63](https://github.com/deep-ion-ai/ctower-agents/issues/63)) ([8dab977](https://github.com/deep-ion-ai/ctower-agents/commit/8dab977541dbe876c6a75decfd820bf9c3e64186))

## [1.2.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.1.0...v1.2.0) (2026-10-30)


### Features

* **ui:** índice de projetos na atividade Projetos ([#42](https://github.com/deep-ion-ai/ctower-agents/issues/42)) ([1061544](https://github.com/deep-ion-ai/ctower-agents/commit/10615446a54fbe233be041d911efcb4ec7bd35a9)), closes [#16](https://github.com/deep-ion-ai/ctower-agents/issues/16)
* **ui:** saúde, sincronização e gestor na Visão geral do projeto ([#43](https://github.com/deep-ion-ai/ctower-agents/issues/43)) ([2f3277a](https://github.com/deep-ion-ai/ctower-agents/commit/2f3277a749fb85862d2b450a0242c7233f55abd0)), closes [#19](https://github.com/deep-ion-ai/ctower-agents/issues/19)


### Bug Fixes

* alerta de sync atrasado de projeto desatribuído ([3a9fdc6](https://github.com/deep-ion-ai/ctower-agents/commit/3a9fdc600df65be69e286d7f9c4d4713e4fc70ac))

## [1.1.0](https://github.com/deep-ion-ai/ctower-agents/compare/v1.0.0...v1.1.0) (2026-10-30)


### Features

* **cthubd,ui:** "Ver issue" mostra a descrição no app, sem guardar ([44f8ee0](https://github.com/deep-ion-ai/ctower-agents/commit/44f8ee0395af999a2af1899afc15ebc19d2846ab))
* **cthubd,ui:** "Ver issue" mostra a descrição no app, sem guardar ([5a1f0ca](https://github.com/deep-ion-ai/ctower-agents/commit/5a1f0ca15f9f052019343d5123a644458bba09e5)), closes [#3](https://github.com/deep-ion-ai/ctower-agents/issues/3)
* **ui:** "+" e menu de contexto no grupo Projetos da Sala de controle ([da9d52f](https://github.com/deep-ion-ai/ctower-agents/commit/da9d52f78e2a449d2c02666194163e4c0918e343)), closes [#10](https://github.com/deep-ion-ai/ctower-agents/issues/10)
* **ui:** "Atribuir a um ambiente" no aviso do projeto sem ambiente ([8bc782c](https://github.com/deep-ion-ai/ctower-agents/commit/8bc782c9c326591422fda033360caf8f5097ecb1)), closes [#7](https://github.com/deep-ion-ai/ctower-agents/issues/7)
* **ui:** atalhos para o template e as regras na modal de início ([121d326](https://github.com/deep-ion-ai/ctower-agents/commit/121d326a5d0bba85129d5a297450571747e46b56)), closes [#11](https://github.com/deep-ion-ai/ctower-agents/issues/11)
* **ui:** botão "Configurações do projeto" no cabeçalho do projeto ([19153c1](https://github.com/deep-ion-ai/ctower-agents/commit/19153c1e37e0db3415762e092052baba94b3e582)), closes [#8](https://github.com/deep-ion-ai/ctower-agents/issues/8)
* **ui:** comandos de projeto na paleta ([0a12d41](https://github.com/deep-ion-ai/ctower-agents/commit/0a12d4146360c5dd64610f16f4410a08631bcf87)), closes [#9](https://github.com/deep-ion-ai/ctower-agents/issues/9)


### Bug Fixes

* **ui:** estado vazio de Projetos aponta para o cadastro no Hub ([399d2a3](https://github.com/deep-ion-ai/ctower-agents/commit/399d2a30b42d2397ea9dcc37ac20b0c6ff94605f)), closes [#6](https://github.com/deep-ion-ai/ctower-agents/issues/6)

## [1.0.0](https://github.com/deep-ion-ai/ctower-agents/compare/v0.15.0...v1.0.0) (2026-10-30)


### Features

* versão inicial do Control Tower ([23b7c74](https://github.com/deep-ion-ai/ctower-agents/commit/23b7c741f2a726878f3bacead9278ea1ff3ba979))


### Bug Fixes

* **ci:** Python 3.12 no check-versions e teste do asset sem release publicada ([31419e7](https://github.com/deep-ion-ai/ctower-agents/commit/31419e7612d89847192fb0935487b028ef5a94c7))
* **ci:** uma nova tentativa do cargo test no macOS hospedado ([e458ff6](https://github.com/deep-ion-ai/ctower-agents/commit/e458ff603d3323f6bff0d539f5031495af16037f))

## Changelog
