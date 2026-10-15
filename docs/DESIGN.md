# Maestro: design

IDE pessoal, nativa em Rust, construída em volta do terminal com o Claude Code. O editor existe para revisar o que o Claude fez, não para ser o lugar principal de escrever código.

O que vale aqui é o estado atual; o histórico das decisões está no git. A ordem de construção está no [CHECKLIST.md](../CHECKLIST.md). O protótipo clicável fica em [prototype/maestro.html](../prototype/maestro.html).

## 1. Princípios

- **Terminal no centro.** O Claude real, com as skills, os MCPs e o CLAUDE.md de sempre, roda num terminal de verdade. O Maestro não reimplementa o Claude.
- **Não reimplementar o que já existe.** O herdr faz abas, splits, worktrees e status dos agentes. O Ghostty define o visual. O vault guarda a memória. O claude-reminder cuida do alerta fora da janela.
- **Nada sai da máquina sem você ver.** Comentário no GitLab, work item, prompt montado pelo Maestro: tudo passa por um rascunho visível e por um clique.
- **Tudo que é criado tem limpeza.** Worktree, branch, ref, porta e projeto do compose entram num inventário por feature.
- **Atalhos:** com o foco no terminal, Ctrl+tecla vai para o PTY. Ctrl+Shift+tecla é do Maestro, como no Ghostty.

## 2. Stack

- `gpui` + `gpui-component`, ambos pinados no mesmo rev de gpui. O gpui-component já traz editor com tree-sitter e LSP, docks, tabs e painéis.
- `alacritty_terminal` (estado do terminal e parser VT) + renderer de grid próprio no GPUI, com `portable-pty`. O `terminal_view` do Zed é GPL: serve só de referência de leitura.
- `reqwest` + `serde` para o GitLab (token do glab) e para o socket do herdr. `notify` para recarregar arquivos e o vault.
- Um crate binário, com módulos. Vira workspace só se o tempo de compilação doer.
- Limite conhecido: o GPUI não roda shader GLSL. O `cursor_blaze` vira um rastro de cursor desenhado à mão.

```
src/
├─ main.rs         janela, layout raiz, keymap
├─ terminal/       PTY + alacritty_terminal + renderer + config do Ghostty
├─ herdr/          cliente do socket (requests + events.subscribe)
├─ features/       nova feature, inventário, multi-repo, conclusão e Limpeza
├─ review/         diff local, multibuffer, checkpoints, envio ao Claude
├─ mr/             GitLab: aba MR, pré-voo, inbox, revisor de MRs alheios
├─ audit/          auditoria
├─ vault/          parser, modo Vault, grafo
└─ attention/      fila, mascote, permissões que aprendem, custo
```

## 3. Terminal

- PTY rodando o cliente `herdr`, que faz attach na sessão persistente. Abas e splits são do herdr; dentro do Maestro, a lateral do herdr fica oculta (`sidebar_collapsed_mode = "hidden"`).
- Do `~/.config/ghostty/config` vêm: fonte, `font-feature` (ligaduras), paleta de 16 cores, fg/bg, seleção, cursor (block, sem blink), padding, opacidade e `unfocused-split-opacity` (0.7).
- O prompt (starship), `ls` (eza) e `cat` (bat) rodam de verdade. O Maestro só precisa emular o terminal direito, e não desenha nada disso.
- Os binds de split do `ghostty/config` (`ctrl+shift+enter`, `ctrl+shift+backslash`, setas, `z`, `t`) viram ações do herdr pelo socket.
- **Aceitação:** screenshot lado a lado com o Ghostty. Fora o shader do cursor, qualquer diferença conta como bug.

## 4. herdr e a lateral

- Socket `~/.config/herdr/herdr.sock`, protocolo 19.
- Eventos usados: `pane.agent_status_changed` (`idle | working | blocked | done`), `pane.focused` e `pane.created`.
- Ações usadas: `workspace.focus`, `worktree create/remove`, `agent.start`, `agent.prompt`, `agent.focus` e `pane.send_text`.
- A lateral lista as features (worktrees com agente) e, separados, os "Terminais" (pastas fora de git).
- Cada card mostra estado, porta, custo, contexto e o resumo do MR.

## 5. Features

### 5.1 Nova feature
Branch e base, prompt opcional, issue do Jira opcional (preenche a branch e o prompt) e de 1 a 3 tentativas.

Ao criar, o Maestro:
1. cria a worktree com `git worktree add` em `<repo>.wt/<slug>` e abre um workspace do herdr nela (`workspace.create`, que aceita env; o `worktree.create` do herdr não aceita);
2. copia os `.env*` ignorados pelo git;
3. roda o setup do repo (`.maestro.toml`, ex.: `npm ci`);
4. injeta `PORT` e `COMPOSE_PROJECT_NAME` únicos no ambiente do pane;
5. abre o `claude` com a nota do repo no contexto (seção 9) e o prompt.

### 5.2 Inventário
`~/.local/share/maestro/features/<id>.json` registra tudo que a feature criou: worktrees (por repo), branches, `refs/maestro/<id>/*`, porta, projeto do compose e MRs.

É a fonte da verdade para concluir a feature e para a Limpeza. O que não está no inventário não é apagado.

### 5.3 Multi-repo sob demanda
A feature nasce num repo só. "+ repo" (ex.: `k8s-configmap`, `vault-roleset`, `pipeline`) cria a worktree no repo extra **só quando pedido**, manda `/add-dir <path>` ao agente e registra a worktree no inventário. Os MRs dos vários repos se citam na descrição.

### 5.4 Concluir e Limpeza
- **Concluir:** remove as worktrees e as branches locais já mergeadas, apaga `refs/maestro/<id>/*`, para o compose e libera a porta.
  - Se houver algo não commitado ou não mergeado, avisa e pede confirmação item a item.
  - Worktree extra que terminou sem nenhuma mudança sai sem perguntar, porque está limpa.
  - Volume de banco do compose só é apagado com confirmação à parte.
- **Limpeza** (comando "Feature: Limpeza…"): compara o disco com os inventários e lista os órfãos: worktree sem feature (só em `<repo>.wt/`), inventário sem worktree, ref de feature removida, compose parado de uma worktree do Maestro, branch local mergeada. Apaga o que estiver marcado; branch local e worktree com mudanças vêm desmarcadas.

### 5.5 Melhor de N
2 ou 3 worktrees `-a/-b/-c` com o mesmo prompt, opcionalmente com modelos diferentes (`claude --model`). A comparação mostra arquivos, testes, custo e abordagem. "Ficar com esta" conclui as outras pelo inventário.

## 6. Revisão local

- **Multibuffer:** um excerpt por bloco do diff (worktree contra a base, mais os não commitados), sobre o buffer real do arquivo. Editar ali salva no arquivo; editar uma linha de contexto vira `-`/`+`.
- Aceitar ou desfazer por bloco (`git apply -R` do bloco), com desfazer.
- Comentários (`C`) e as suas edições vão num prompt só para o agente da feature: "Revisei suas mudanças… eu editei X; mantenha".
- **Checkpoints por turno:** o hook `UserPromptSubmit` grava `git stash create` + `update-ref refs/maestro/<id>/t<n>`, sem tocar no índice nem na branch. A revisão filtra por turno, e "voltar para antes do turno N" faz `git restore --source=<ref>`.
- **Testes afetados:** ripwire `change-check` + `--findRelatedTests`, num split do terminal.

## 7. Pré-voo e ciclo de vida do MR

### 7.1 Pré-voo (antes do `/mr`)
| Item | Como |
|---|---|
| Branch atualizada | `git fetch` + contagem de commits atrás da base |
| Testes afetados | seção 6 |
| Lint e typecheck | scripts do repo (`.maestro.toml`) |
| Sonar local | MCP do Sonar (`analyze_code_snippet`) nos arquivos mudados; repo com vitest também roda `ctbz-check sonar-vitest` (decisão do vault `ctbz-check-sonar-vitest`) |
| Segredos | `gitleaks` quando instalado; senão regex de token/chave no diff |
| Arquivos proibidos | `.env*`, build, `node_modules` no diff |

Com tudo verde, o `/mr` roda. Item vermelho exige override explícito, registrado na descrição do MR. A descrição nunca menciona Claude nem "Generated by".

### 7.2 Aba MR da feature
Mostra pipeline, quality gate do Sonar e comentários de revisores, com os botões `/pipe <job>` e `/comentario`. Merge detectado → oferece concluir a feature (5.4) e escreve no Histórico do vault (seção 9).

## 8. MRs de outras pessoas e revisor

O revisor é um port do `~/mr-review.sh`. As armadilhas que o script já resolveu viram teste:
1. Linha de contexto exige `old_line`, calculado pelo hunk. Sem isso: 400 `line_code can't be blank`.
2. POST com `Content-Type: application/json`, senão 415.
3. Sucesso = status ok **e** resposta com `.id`.
4. Fetch de `refs/merge-requests/N/head`, porque a branch de origem pode já ter sido apagada.
5. Pasta do repo pelo caminho completo quando o basename colide.
6. Gate de pipeline: success, none, skipped e manual passam; o resto pede confirmação.
7. Todas as notas existentes (humanas e de IA) entram no prompt para dedup.
8. Linha fora do diff → nota geral com `arquivo:linha` no corpo.
9. MR fechado ou merged roda em dry-run.
10. Pular MR já revisado no mesmo sha.
11. `--append-system-prompt` com a rubrica + ferramentas restritas (git diff/log/blame/show, Read, Grep, Glob, Skill).

Melhorias sobre o script:
- saída via `claude -p --output-format stream-json --json-schema`;
- achados viram rascunhos;
- publicação como uma review só (Draft Notes API + `bulk_publish`);
- revisão incremental por sha;
- worktree persistente por MR.

## 9. Vault

Usa o mesmo contrato do `~/vault/CLAUDE.md`:
- **Modo Vault** no topo (Features · MRs · Vault):
  - lateral: busca, Grafo, Hoje (daily), Atividade e as notas por pasta;
  - centro: nota renderizada e editável, "Links desta nota" e "Mencionado em".
- **Contexto:** toda feature recebe `repos/<repo>.md` e o que ele linka.
- **Escritas sem pedir** (as que o contrato permite): Histórico em MR aberto ou mergeado, `decisions/` para gotcha de debug, `[x]` na daily. Cada escrita vai para a Atividade, com desfazer. O resumo do dia é só prévia, até você salvar.
- **Grafo:** nós são as notas (exceto `templates/`); arestas são os `[[links]]`. Links quebrados aparecem tracejados, e features e MRs ativos se ligam ao repo. Cor por pasta, tamanho pelo número de links, filtros. Barnes-Hut a partir de ~500 nós.
- A auditoria lê `decisions/` para não sugerir o que já foi decidido contra.

## 10. Atenção

- **Fila** (`Ctrl+Shift+Espaço` = próximo), nesta prioridade:
  1. permissão;
  2. CI ou Sonar vermelho;
  3. comentários e features prontas para revisar ou comparar;
  4. contexto ≥ 80% e MR alheio;
  5. daily.
- **Mascote:** os sprites de `claude-mascot/overlay/poses.py` e os sons do config, dentro da janela. Fora do foco, o claude-reminder continua igual, e o clique volta ao Maestro e ao pane certo.
- **Permissões que aprendem:** o hook `PermissionRequest` conta aprovações por padrão de comando e repo, e sugere `.claude/settings.json` (time) ou `.claude/settings.local.json` (você).
- **Custo e contexto** por feature, lidos do JSONL da sessão em `~/.claude/projects/`.

## 11. Auditoria

- **Nunca automática.** Abrir o projeto mostra o último relatório (`.maestro/audits/<sha>.json`) e o que mudou desde então.
- **Escopo:** incremental (diff desde a última + vizinhos de 1 salto) ou completa. As duas mostram a estimativa antes e param no teto de `--max-budget-usd`.
- **Etapas:**
  1. mapa (ripwire);
  2. uma análise por pilar;
  3. verificação adversarial de cada achado;
  4. dedup contra os work items deste repo e de repos irmãos, e contra as decisões do vault.
- **Work item:** título `[P0–P3] tipo(escopo): …`, labels `bug | security | tech-debt | scalability | size:*`, corpo com `## Problema`, `## Evidência`, `## Correção sugerida`, `## Impacto`.

Prompt (base: snippet 5996330):
```
Você é arquiteto de software e engenheiro de segurança sênior auditando {repo} em modo SOMENTE LEITURA.
Escopo: {incremental: arquivos de `git diff {sha}...HEAD` e quem os chama/é chamado (1 salto) | completo: repo inteiro}.
Leia também Dockerfile, .gitlab-ci.yml, k8s/, .env.example, README e CLAUDE.md. Siga o CLAUDE.md do repo.
Pilares: {lista marcada}.

Regras:
1. Só reporte o que você VERIFICOU no código: cite arquivo:linha e o trecho.
2. Severidade por impacto: P0 perda de dado ou serviço fora em prd; P1 risco alto e provável; P2 real mas raro/contornável; P3 higiene.
3. NÃO repita nada destas listas: work items existentes e ignorados {…}; decisões do vault {…}.
   Se o achado repete um padrão já corrigido em outro repo, preencha `related`.
4. Correção concreta, sem refatoração especulativa.
5. Não altere arquivos.
Saída: JSON no schema {finding[]: sev, labels[], title, location, confidence, problem, evidence, fix, impact, size, related?}.
```

## 12. Tutorial

Spotlight + popover na primeira vez em cada área. Há passos que esperam o clique real. "Rever tutorial" fica na ajuda. O estado fica em `~/.config/maestro/state.toml`. É um overlay próprio no GPUI, porque driver.js e Shepherd só servem de referência.

## 13. Heurísticas de Nielsen

| # | Heurística | Onde |
|---|---|---|
| 1 | Status visível | estado de cada feature, fila com contagem, progresso do revisor e da auditoria |
| 2 | Mundo real | "feature" em vez de worktree; termos do GitLab |
| 3 | Controle e liberdade | Esc interrompe; desfazer em bloco, comentário, checkpoint e escrita no vault |
| 4 | Consistência | mesmo visual e atalhos do Ghostty; o mesmo diff para revisar o Claude e MRs |
| 5 | Prevenção de erros | pré-voo; avisos de não commitado e CI vermelho; confirmação antes de publicar |
| 6 | Reconhecer, não lembrar | paleta com atalhos; dicas por tela |
| 7 | Eficiência | teclado em tudo, fila, revisão incremental |
| 8 | Minimalismo | terminal no centro; editor só quando pedido |
| 9 | Recuperação de erros | erro do GitLab no próprio rascunho, com uma saída |
| 10 | Ajuda | tutorial na primeira vez e estados vazios que explicam o próximo passo |
