# Changelog

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
