# Checklist do Maestro

Trabalho em etapas. Uma etapa só começa quando a anterior passou no **Pronto quando**.
Cada item é marcado `[x]` só depois de verificado, não quando o código compila.
Detalhes de cada item: [docs/DESIGN.md](docs/DESIGN.md). Protótipo de UX: [prototype/maestro.html](prototype/maestro.html).

Regras:
- Menor diff que funciona. Nada de abstração "para depois".
- Toda lógica não trivial deixa um teste que quebra se ela quebrar.
- Commits sem coautoria do Claude.
- Tudo que o Maestro cria (worktree, branch, ref, porta, projeto do compose) fica registrado e tem caminho de limpeza.

---

## Etapa 0: Fundação

- [x] `rustup` estável instalado (rustc 1.99.0)
- [x] ambiente reproduzível: `scripts/deps.sh` aqui + `install.sh` do [dotfiles](https://github.com/rafaelmaestro/dotfiles)
- [x] libs de sistema do GPUI instaladas (`scripts/deps.sh`); `vulkaninfo` responde (Intel RPL-S, Mesa)
- [x] crate com `gpui-kit =0.7.0` (traz o gpui e o gpui-component já casados) e `Cargo.lock` versionado
- [x] janela vazia abre com o fundo Dracula
- [x] baseline (`MAESTRO_STARTUP=1`, build dev com dependências em opt-level 2): primeiro frame em ~100 ms (915 ms na primeira execução, com o cache de shaders frio); RSS ~100 MB
- [x] CI no GitHub Actions: `cargo fmt --check`, `clippy -D warnings`, `cargo test` (1ª execução sem cache: 26 min)
- [ ] ~~config.toml~~ adiado: entra quando existir a primeira opção configurável

**Pronto quando:** `cargo run` abre a janela em menos de 300 ms e o CI passa. ✅ concluída em 2026-10-15

## Etapa 1: Terminal (o maior risco)

- [x] PTY rodando `$SHELL` com o env do usuário (o `tty` + `EventLoop` do próprio `alacritty_terminal`; sem `portable-pty`)
- [x] `alacritty_terminal` + renderer de grid no GPUI: 16, 256 e truecolor; bold, italic e underline; cursor block (verificado com screenshot: starship, eza com ícones, ligaduras)
- [x] input: teclas e modificadores, colar (Ctrl+Shift+V, Shift+Insert, botão do meio) com bracketed paste, rolagem do histórico (roda e Shift+PageUp/Down), seleção (simples, palavra com 2 cliques, linha com 3) e cópia (Ctrl+Shift+C e copy-on-select na PRIMARY)
- [x] mouse reporting SGR e X10 para os apps, com arrasto (1002) e movimento sem botão (1003); validado com clique real no herdr
- [x] acentos e teclas mortas no layout `us intl` (ç, é, ã): exigiu patch em `vendor/xim-ctext` (o ibus manda a pré-edição em Latin-1 e a crate derrubava o processo)
- [x] resize propaga para o PTY (SIGWINCH): a grade se ajusta ao tamanho da janela
- [x] parser do `~/.config/ghostty/config` com teste: `font-family`, `font-size`, `font-feature`, `palette`, `background`, `foreground`, `selection-*`, `cursor-*`, `window-padding-*` e `background-opacity`. O `keybind` fica para a Etapa 2 (binds de split viram ações do herdr). O `unfocused-split-opacity` não se aplica: os splits são do herdr, dentro de um terminal só
- [x] grade em pixels inteiros como a do Ghostty (13 pt a 96 DPI: célula 10×22, glifo no tamanho real); cursor vazado sem foco. Conferido com screenshot lado a lado de um Ghostty isolado: colunas alinhadas
- [x] ligaduras e glifos da Nerd Font
- [x] regra de atalhos: com o foco no terminal, Ctrl+tecla vai para o PTY; Ctrl+Shift+letra (e enter, `\`, setas) é do Maestro. Ctrl+Shift+- e Ctrl+Shift+2 seguem como `^_` e `^@`. Conferido com uma sonda de bytes no PTY
- [x] `herdr`, `vim`, `btop` e `lazygit` rodam sem artefato visual (screenshot contra um Ghostty isolado; o braille do btop é desenhado como no Ghostty, não pela fonte)
- [x] respostas ao app: cores (OSC 10/11/4), tamanho em pixels (CSI 14 t), cópia por OSC 52 (só escrita); variáveis `HERDR_*` removidas para o herdr rodar mesmo com o Maestro aberto de dentro de um pane
- [ ] (por último, opcional) aproximação do `cursor_blaze`

**Pronto quando:** num screenshot lado a lado, Ghostty e Maestro mostram `ls`, `git diff`, o prompt do starship e o `btop` iguais, com diferença só no shader do cursor. Digitar não tem atraso perceptível (medir tecla → pixel).

**Resultado (2026-10-15):** `ls`, `git diff`, prompt do starship, `btop` e `lazygit` iguais ao Ghostty, com duas diferenças conhecidas: o shader do cursor e o tamanho de alguns ícones da Nerd Font, porque o Ghostty encolhe o ícone para caber na célula (ex.: o logo do Ubuntu no starship). Latência medida no build release: da tecla até o frame montado, com eco do zsh, mediana de 5,4 ms (máx. 9,3 ms), mais até um frame para aparecer na tela.

## Etapa 2: herdr + lateral de Features + paleta

- [x] cliente do socket do herdr (protocolo 19): uma conexão por request (o servidor fecha depois de responder). Status de agente não tem evento global, só `pane.agent_status_changed` por pane; a lateral faz polling de `agent.list` a cada 500 ms (`ponytail:`), o que já cobre a reconexão
- [x] shell igual ao protótipo aprovado (conferido lado a lado): barra de título própria com os três pontos (fechar, minimizar, maximizar; arrastar move), modos e Fila, abas do herdr desenhadas pelo Maestro (`1 claude ◐`, `+`, indicador PREFIX), dicas e status (`hoje: US$`). IBM Plex Sans embutida. O que ainda não existe aparece apagado no lugar dele
- [x] lateral na ordem do protótipo: repo · Auditoria · FEATURES · cards (com `US$ · ctx %` lidos do JSONL do Claude) · Nova feature · TERMINAIS · Mudanças/Arquivos (diff da branch contra a base)
- [x] lateral: um card por agente (o seu uso real é um workspace com vários Claudes em abas), agrupado por repo, com branch, glifo de status, tempo, linha de status e filete de atenção; o clique faz `agent.focus`. Ctrl+Shift+1…9 e Ctrl+Shift+Espaço (próximo da fila)
- [x] "Terminais" = workspaces do herdr (`$ ~ · N abas · só terminal`), clique faz `workspace.focus`
- [x] herdr sem o chrome dele: o Maestro usa a sessão própria `maestro` com `~/.config/maestro/herdr.toml` (seu `config.toml` + lateral oculta e barra de abas fixa na linha 0, que o Maestro esconde). A sessão `default` do Ghostty não muda
- [x] binds de split do `ghostty/config` viram as teclas do herdr (prefixo do seu `config.toml`) escritas no PTY, para valer no cliente do Maestro
- [x] reconexão quando o server do herdr reinicia: a lateral retoma sozinha ("herdr: fora do ar" no status) e o cliente é reaberto enquanto sair com erro (servidor caiu); detach (Ctrl+A d) fecha o Maestro como antes
- [x] paleta de comandos (`Ctrl+Shift+P`, ou clique em "comandos"): o `CMDS` do protótipo na mesma ordem, com o atalho ao lado, o mesmo fuzzy e "Feature: <branch>" na busca; Esc ou clique fora fecham. Os comandos de etapas futuras aparecem apagados. Ctrl+Shift+B mostra/oculta a lateral
- [x] tutorial da área (primeira vez): os 4 passos do `TOURS.feat` do protótipo, com destaque e balão; Esc fecha; estado em `~/.config/maestro/state.toml`. O passo 4 termina em "Concluir" até a Nova feature existir
- [x] testes sempre na sessão `MAESTRO_HERDR_SESSION=maestro-test`, nunca na `default`

**Pronto quando:** um `claude` pedindo permissão muda o status na lateral em menos de 1 s, e o clique leva ao pane certo.

## Etapa 3: Features em worktrees, multi-repo e limpeza

- [ ] Nova feature: branch, base e prompt → `herdr worktree create` → copiar os `.env*` ignorados → setup do repo (`.maestro.toml`) → `claude` com o prompt
- [ ] **inventário da feature** em `~/.local/share/maestro/features/<id>.json`: worktrees, branches, refs, porta, projeto do compose e MRs que ela criou
- [ ] `PORT` e `docker compose -p <repo>-<slug>` únicos por worktree
- [ ] issue do Jira opcional preenche a branch e o prompt
- [ ] melhor de N: 2 ou 3 tentativas `-a/-b/-c`, comparação e "ficar com esta" (remove as outras pelo inventário)
- [ ] **multi-repo sob demanda:** "+ repo" (ex.: `k8s-configmap`, `vault-roleset`, `pipeline`) cria a worktree só quando pedido, manda `/add-dir` ao agente e registra no inventário; os MRs ficam ligados entre si
- [ ] **concluir feature:** remove worktrees, branches locais mergeadas, `refs/maestro/<feature>/*`, para o compose e libera a porta. Avisa antes se houver algo não commitado ou não mergeado. Volume de banco do compose só sai com confirmação à parte
- [ ] worktree extra que terminou sem nenhuma mudança é removida na conclusão sem perguntar (está limpa)
- [ ] comando **Limpeza**: lista órfãos (worktree sem feature, `refs/maestro` de feature removida, compose parado, branch local já mergeada) e apaga com confirmação
- [ ] tutorial da área

**Pronto quando:** você cria uma feature em `bots-api` + `k8s-configmap`, conclui, e `git worktree list`, `git for-each-ref refs/maestro`, `git branch` e `docker compose ls` voltam exatamente ao estado de antes, nos dois repos.

## Etapa 4: Revisão local (multibuffer)

- [ ] diff da worktree contra a base, incluindo não commitados
- [ ] multibuffer: um excerpt por bloco sobre o buffer real do arquivo; editar salva no arquivo
- [ ] aceitar e desfazer por bloco (`git apply -R` do bloco), com desfazer
- [ ] comentários por linha (`C`) + suas edições → "Enviar ao Claude" (`agent.prompt`) num prompt só
- [ ] checkpoints por turno: hook `UserPromptSubmit` grava `refs/maestro/<feature>/t<n>`; filtro por turno; "voltar para antes do turno N"
- [ ] testes afetados (ripwire + `--findRelatedTests`) num split
- [ ] editor de arquivo simples (gpui-component) e `Ctrl+Shift+L` (seleção → `@arquivo:linhas` no Claude)
- [ ] arquivo alterado pelo Claude recarrega; com edição local, banner de conflito
- [ ] tutorial da área

**Pronto quando:** você comenta 2 linhas e edita 1 e o prompt chega no pane certo (conferir com `herdr agent read`), e "voltar turno" restaura os arquivos byte a byte.

## Etapa 5: Pré-voo e ciclo de vida do MR

- [ ] **pré-voo**, com resultado por item:
  - branch atualizada com a base
  - testes afetados verdes
  - lint e typecheck do repo
  - Sonar local (MCP do Sonar); em repo com vitest, também `ctbz-check sonar-vitest`
  - nenhum segredo no diff
  - nenhum arquivo proibido (`.env`, build, `node_modules`)
- [ ] o `/mr` só roda com o pré-voo verde; um item vermelho exige override explícito, registrado na descrição do MR
- [ ] aba MR da feature: pipeline, quality gate do Sonar e comentários (API do GitLab com o token do glab)
- [ ] botões que chamam a skill certa: `/pipe <job>` e `/comentario`
- [ ] merge detectado → oferece a conclusão da etapa 3
- [ ] tutorial da área

**Pronto quando:** um MR de teste com um token falso no diff é barrado no pré-voo, e um CI vermelho aparece na aba MR com "Diagnosticar com /pipe".

## Etapa 6: Atenção

- [ ] fila de atenção (permissão > CI/Sonar > comentários e prontas > contexto alto e MR alheio > daily) e `Ctrl+Shift+Espaço`
- [ ] mascote no app com os sprites de `claude-mascot/overlay/poses.py` e os sons do config; fora do foco, o claude-reminder continua cuidando do desktop
- [ ] permissões que aprendem (hook `PermissionRequest`): sugestão para o time ou só para você
- [ ] custo e contexto por feature lidos do JSONL da sessão; aviso com contexto ≥ 80%

**Pronto quando:** uma permissão numa feature de fundo mostra o mascote, e o clique foca o pane certo.

## Etapa 7: Vault

- [ ] parser do `~/vault`: frontmatter, `[[links]]` e backlinks, atualizado por `notify`
- [ ] modo Vault: árvore, nota renderizada e editável, "Links desta nota", "Mencionado em", Hoje (daily) e Atividade
- [ ] toda feature recebe `repos/<repo>.md` e o que ele linka (`--append-system-prompt`)
- [ ] escritas permitidas pelo `vault/CLAUDE.md` (Histórico, `decisions/` de gotcha, `[x]` na daily), registradas na Atividade com desfazer
- [ ] grafo (estilo Obsidian)
- [ ] tutorial da área

**Pronto quando:** uma feature nova traz a nota do repo no contexto, e um merge escreve a linha certa no Histórico.

## Etapa 8: MRs de outras pessoas + revisor

- [ ] inbox: para revisar, meus e grupo; badge de CI; "novo", "revisado" e "+N commits"
- [ ] diff do MR com as threads; rascunho manual
- [ ] revisor (port do `~/mr-review.sh`), com as 11 armadilhas cobertas por teste:
  - [ ] `old_line` em linha de contexto
  - [ ] header `Content-Type`
  - [ ] sucesso = status ok **e** `.id`
  - [ ] fetch de `refs/merge-requests/N/head`
  - [ ] colisão de basename
  - [ ] gate de pipeline
  - [ ] dedup pelas notas existentes
  - [ ] fallback para nota geral quando a linha está fora do diff
  - [ ] MR fechado ou merged em dry-run
  - [ ] pular sha já revisado
  - [ ] rubrica + ferramentas restritas
- [ ] saída via `--json-schema`, progresso por `stream-json`, achados como rascunhos
- [ ] publicar como uma review só (Draft Notes API + `bulk_publish`)
- [ ] tutorial da área

**Pronto quando:** num MR de teste, um comentário em linha adicionada e outro em linha de contexto aparecem inline no GitLab, e a review gera uma notificação só.

## Etapa 9: Auditoria

- [ ] relatório salvo por sha; abrir o projeto nunca roda auditoria
- [ ] incremental (diff desde a última + vizinhos) e completa, com estimativa e `--max-budget-usd`
- [ ] verificação adversarial + dedup contra work items deste repo, de repos irmãos e as decisões do vault
- [ ] achados marcáveis, prévia editável, "abrir N work items" com confirmação, "não é problema" persistido
- [ ] "Corrigir em feature" a partir do work item
- [ ] tutorial da área

**Pronto quando:** a incremental no email-service desde um sha antigo não devolve nenhum dos #15 a #50 como "novo".

---

## Backlog (não entra sem decisão)

- Conflitos entre features (ripwire branch conflicts)
- Do merge à produção (rollout + logs Datadog)
- Resumo de ausência
- Notificação no celular quando você está fora
- LSP no editor
