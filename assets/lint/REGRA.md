# Como escrever uma regra no Maestro

Uma regra vive em `.maestro/regras/<id>.regra` do repositório e é lida pela engine nativa do
Maestro — não é ESLint, não é prettier, não tem plugin de terceiro no meio.

**Uma diretiva por linha, `chave args…`.** Sem indentação, sem aspas, sem escape. Linha vazia e
linha começando com `#` são comentário. Regra torta não carrega calada: o Maestro mostra o arquivo,
a linha e o motivo.

## O arquivo inteiro

```
# dinheiro em ponto flutuante não fecha o total
regra dinheiro-inteiro
severidade bloqueio
quando arquivo src/domain/** src/app/**
quando no declaracao-de-campo
proibido tipo number Decimal decimal double
a_menos_que nome_termina_em Cents Micros PercentMicros
porque Valor é inteiro em unidade mínima: em ponto flutuante o total não fecha.
errado preco: number
certo precoCents: number
```

## As chaves

| Chave | O que faz |
|---|---|
| `regra <id>` | o nome que aparece no diagnóstico e na válvula. Obrigatório. |
| `severidade aviso\|bloqueio` | `aviso` (padrão) é avisado na escrita; `bloqueio` reprova no commit. |
| `quando arquivo <glob…>` | onde a regra vale. `*` é um segmento, `**` é qualquer profundidade. Sem isto, vale em todo arquivo da linguagem. |
| `quando no <seletor>` | em que nó a regra olha. Obrigatório — é o que deixa a engine despachar numa passada só. `maestro lint --seletores` lista os que existem. |
| `proibido <fato> <valor…>` | viola se o fato casar com algum valor. |
| `exigido <fato> <valor…>` | viola se o fato **não** casar com nenhum valor. |
| `a_menos_que <fato> <valor…>` | isenta o nó, mesmo que `proibido`/`exigido` tenham casado. |
| `porque <texto>` | o motivo, em uma linha. Obrigatório: é o que vai para a LLM e para quem lê o erro de madrugada. Sem ele, o agente conserta a linha e repete o erro no arquivo seguinte. |
| `errado <texto>` / `certo <texto>` | o exemplo que vai junto do diagnóstico. |

Pode repetir `proibido`, `exigido` e `a_menos_que`: casa quando **algum** deles casa. Precisa de
duas condições ao mesmo tempo? São duas regras — e aí cada diagnóstico é específico.

## Fato, nunca árvore

A regra nunca fala de AST. Ela cita **fatos** que o plugin de linguagem extrai do nó (`nome`,
`tipo`, e os que forem entrando). É o que mantém a regra legível e faz a gramática crescer sem
reescrever regra nenhuma.

O operador vem no sufixo do fato:

| Sufixo | Casa quando |
|---|---|
| (nenhum) | o fato é igual a um dos valores |
| `_termina_em` | termina com um dos valores |
| `_comeca_com` | começa com um dos valores |
| `_contem` | contém um dos valores |

Ex.: `a_menos_que nome_termina_em Cents` e `proibido tipo_contem any`.

## Suprimir precisa de motivo

Na linha do código:

```ts
taxa: number // regra-ok: dinheiro-inteiro taxa legada do XML fiscal, sai na migração 4.2
```

`regra-ok: <id> <motivo>` — **sem motivo não vale** e a violação continua aparecendo. O
`maestro lint --ledger` lista todas as supressões do repo, então ela é uma dívida visível e não um
jeito de calar a engine.

## Rodando

| Comando | Para quê |
|---|---|
| `maestro lint <arquivo…>` | checa; sai com 2 quando achou algo (é o que o hook `PostToolUse` entrega ao modelo) |
| `maestro lint --regras` | lista o que está carregado, nativo + do repo |
| `maestro lint --seletores` | os nós que dá para citar em `quando no` |
| `maestro lint --como` | este documento |

## Para a LLM, em três linhas

1. Leia `maestro lint --seletores` antes de escrever `quando no`: seletor inventado não casa com nada e a regra fica muda.
2. `porque` é obrigatório e é uma frase sobre a **consequência**, não sobre a sintaxe. "em ponto flutuante o total não fecha", não "não use number".
3. Depois de escrever a regra, rode `maestro lint` num arquivo que **deveria** violar e num que **não** deveria. Regra que não foi vista pegar nada ainda não existe.
