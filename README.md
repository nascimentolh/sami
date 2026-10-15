# Maestro

IDE pessoal em Rust (GPUI) construída em volta do Claude Code no terminal. Cada feature roda numa worktree própria, você revisa em multibuffer o que o Claude fez, acompanha os MRs do GitLab e consulta a memória do vault.

- O que falta fazer, por etapa: [CHECKLIST.md](CHECKLIST.md)
- Como cada parte funciona: [docs/DESIGN.md](docs/DESIGN.md)
- Protótipo clicável de UX: [prototype/maestro.html](prototype/maestro.html) (abra no navegador)

## Montar o ambiente

1. Ambiente geral (Ghostty, herdr, starship, Claude Code com skills, plugins, MCPs e hooks): [rafaelmaestro/dotfiles](https://github.com/rafaelmaestro/dotfiles), `./install.sh`.
2. Build do Maestro: `./scripts/deps.sh` (pacotes do GPUI, rustup, checagem do Vulkan).

Status: ainda não há código Rust; o próximo passo é a Etapa 0.
