# Maestro — Guia de uso do logo

## 1. O logo
- **Ideia**: a perna direita do M é a batuta do maestro. Você rege, a IDE executa. O gesto ascendente também lê como um ✓ de tarefa feita.
- **Versões**: horizontal (principal) · empilhado · símbolo · wordmark.
- **Arquivos**:

| Uso | Arquivo |
|---|---|
| Símbolo (mestre) | `maestro-symbol.svg` |
| Símbolo ≤ 48 px | `maestro-symbol-small.svg` (sem o anel, cabo e haste mais grossos) |
| Horizontal / empilhado / wordmark | `maestro-horizontal.svg` · `maestro-stacked.svg` · `maestro-wordmark.svg` |
| Preto / branco | `*/…-black.svg` · `*/…-white.svg` (+ PNGs) |
| Ícone do app (dock, splash) | `symbol/maestro-symbol-app-icon.svg` · `-512/-1024.png` |
| Web | `symbol/favicon.ico` · `favicon.svg` · `icon-192/512.png` · `apple-touch-icon.png` · `site.webmanifest` · `head-snippet.html` |

## 2. Área de respiro
Mantenha livre ao redor do logo **1 × a largura do traço do M** (≈ 12 % da altura do símbolo). A área escala junto com o logo.

## 3. Tamanho mínimo
| Versão | Tela |
|---|---|
| Horizontal | 120 px de largura |
| Empilhado | 72 px de largura |
| Símbolo | 16 px (abaixo de 48 px use `maestro-symbol-small.svg`) |

## 4. Cor
Preto e branco, no espírito do Zed. Nenhuma cor de marca.

| Nome | HEX | RGB |
|---|---|---|
| Preto | `#000000` | 0 0 0 |
| Branco | `#FFFFFF` | 255 255 255 |

**Combinações aprovadas**: preto sobre fundo claro · branco sobre fundo escuro · branco sobre o tile preto (ícone).
Em temas escuros da IDE use a versão branca; o branco pesa mais que o preto, então prefira o tamanho um degrau menor.

## 5. Tipografia
O wordmark `maestro` é desenhado à mão (monoline, pontas arredondadas, mesmo vocabulário do M). Não é uma fonte: nunca o redigite.
Para textos ao redor (site, docs, splash), Inter combina bem.

## 6. Não faça
Não estique nem achate · não gire · não mude o ângulo da batuta · não separe a batuta do M · não adicione sombra, contorno, gradiente ou brilho · não reorganize nem redimensione as partes de um lockup · não use sobre fundos poluídos sem o tile.

## 7. Notas de entrega
- Testado: 16/32/48 px, uma cor, invertido, ícone em tile e mockups (README, terminal, site, app, sticker, social).
- Os mestres usam `stroke` (traço) no M e no wordmark; para gráfica ou corte, expanda os traços em contornos (Inkscape: *Caminho → Traço para caminho*).
- Não há garantia de anterioridade de marca: faça uma busca no INPI antes de registrar.
