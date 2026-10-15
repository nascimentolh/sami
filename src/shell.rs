use gpui_kit::{prelude::FluentBuilder, *};

use std::time::Duration;

use crate::{
    changes,
    cleanup::{self, Cleanup},
    compare::{self, Attempt, Compare},
    confirm::{self, Answer, Choice, Chose, Confirm},
    feature,
    herdr::{self, Status},
    keys,
    mascot::{self, Note},
    new_feature::{self, NewFeature},
    palette_cmd::{self, Cmd, CommandPalette},
    review::{self, Redo, Review},
    sidebar::{Go, Sidebar, SidebarEvent},
    terminal::TerminalView,
    theme::Theme,
    tour::{self, Anchors},
};

/// Janela do protótipo: título (36) · lateral | terminal · dicas (22) · status (24).
pub struct Shell {
    pub sidebar: Option<Entity<Sidebar>>,
    pub terminal: Entity<TerminalView>,
    pub theme: Theme,
    pub font: String,
    prefix: bool,
    /// (passos, id do tutorial, passo atual)
    tour: Option<(&'static [tour::Step], &'static str, usize)>,
    anchors: Anchors,
    palette: Option<Entity<CommandPalette>>,
    dialog: Option<Entity<NewFeature>>,
    confirm: Option<Entity<Confirm>>,
    cleanup: Option<Entity<Cleanup>>,
    compare: Option<Entity<Compare>>,
    /// (id, texto, patch que refaz o que foi desfeito)
    toasts: Vec<(usize, String, Option<Redo>)>,
    review: Option<Entity<Review>>,
    want_review: Option<Option<String>>,
    want_file: Option<String>,
    want_merged: Option<(std::path::PathBuf, u64, String)>,
    /// a fila do mascote e o que está na tela: (pedido, sequência, saindo)
    mq: Vec<Note>,
    /// as permissões que aprendem: sugestões esperando o diálogo
    learn: Vec<crate::perms::Suggestion>,
    choice: Option<Entity<Choice>>,
    mascot: Option<(Note, usize, bool)>,
    mascot_seq: usize,
    mascot_frame: u64,
    window_active: bool,
    _blink: Option<Task<()>>,
    toast_seq: usize,
    want_dialog: bool,
    left: bool,
}

/// Dicas do modo Features do protótipo; `false` = ainda não existe e aparece apagada.
const HINTS: &[(&str, &str, bool)] = &[
    ("Ctrl+Shift+Espaço", "próximo da fila", true),
    ("Ctrl+Shift+1…9", "feature", true),
    ("Ctrl+Shift+E", "revisar mudanças", true),
    ("Ctrl+Shift+\\", "dividir", true),
    ("Ctrl+Shift+Enter", "empilhar", true),
    ("Ctrl+Shift+←→", "trocar pane", true),
    ("Ctrl+A", "herdr", true),
    ("Ctrl+Shift+L", "seleção → Claude", true),
    ("Alt+N", "nova feature", true),
    ("?", "atalhos", false),
];

impl Shell {
    pub fn new(
        sidebar: Option<Entity<Sidebar>>,
        terminal: Entity<TerminalView>,
        theme: Theme,
        font: String,
        anchors: Anchors,
        cx: &mut Context<Self>,
    ) -> Self {
        let sidebar_present = sidebar.is_some();
        if let Some(s) = &sidebar {
            cx.observe(s, |this: &mut Self, s, cx| {
                // ir para a feature (por qualquer caminho) dispensa o mascote dela, como o `selectFeat`
                let focused = s
                    .read(cx)
                    .agents
                    .iter()
                    .find(|a| a.focused)
                    .map(|a| a.pane_id.clone());
                if this.window_active
                    && this
                        .mascot
                        .as_ref()
                        .is_some_and(|(n, _, out)| !out && Some(&n.pane) == focused.as_ref())
                {
                    this.dismiss_mascot(cx);
                }
                cx.notify();
            })
            .detach();
            // o botão da lateral não tem a janela; o diálogo abre no próximo render
            cx.subscribe(s, |this: &mut Self, _, e: &SidebarEvent, cx| {
                match e {
                    SidebarEvent::NewFeature => this.want_dialog = true,
                    SidebarEvent::OpenReview(p) => this.want_review = Some(Some(p.clone())),
                    SidebarEvent::OpenFile(p) => this.want_file = Some(p.clone()),
                    SidebarEvent::Merged(wt, iid, branch) => {
                        this.want_merged = Some((wt.clone(), *iid, branch.clone()))
                    }
                    SidebarEvent::Notify(n) => this.notify_note(n.clone(), cx),
                    SidebarEvent::Learn(s) => this.learn.push(s.clone()),
                }
                cx.notify();
            })
            .detach();
        }
        // o terminal notifica a cada saída do PTY; o chrome só redesenha quando o modo PREFIX muda
        cx.observe(&terminal, |this: &mut Self, t, cx| {
            let prefix = t.read(cx).prefix_pending();
            if prefix != this.prefix {
                this.prefix = prefix;
                cx.notify();
            }
        })
        .detach();
        Self {
            sidebar,
            terminal,
            theme,
            font,
            prefix: false,
            // tutorial da área na primeira vez, como no protótipo
            tour: (sidebar_present && !tour::seen("feat")).then_some((tour::FEAT, "feat", 0)),
            anchors,
            palette: None,
            dialog: None,
            confirm: None,
            cleanup: None,
            compare: None,
            toasts: Vec::new(),
            review: None,
            want_review: None,
            want_file: None,
            want_merged: None,
            mq: Vec::new(),
            learn: Vec::new(),
            choice: None,
            mascot: None,
            mascot_seq: 0,
            mascot_frame: 0,
            window_active: true,
            _blink: None,
            toast_seq: 0,
            want_dialog: false,
            left: true,
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        if m.alt && !m.control && !m.shift && k.key == "n" {
            return self.open_new_feature(window, cx);
        }
        if m.control && m.shift && !m.alt {
            match k.key.as_str() {
                "p" => return self.toggle_palette(window, cx),
                "e" => return self.toggle_review(None, window, cx),
                "l" => return self.run(Cmd::SendSelection, window, cx),
                "o" => return self.run(Cmd::OpenFileMenu, window, cx),
                "b" => return self.run(Cmd::ToggleLeft, window, cx),
                _ => {}
            }
        }
        if m.control && m.shift && !m.alt && k.key == "space" {
            return self.run(Cmd::NextAttention, window, cx);
        }
        let Some(sidebar) = &self.sidebar else { return };
        let s = sidebar.read(cx);
        let target = if let Some(n) = keys::feature_digit(k) {
            s.order().into_iter().nth(n - 1)
        } else {
            return;
        };
        if let Some(pane) = target {
            s.focus(pane);
        }
        cx.stop_propagation();
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            window.focus(&self.terminal.read(cx).focus_handle(), cx);
        } else {
            let mut items = palette_cmd::commands();
            if let Some(s) = &self.sidebar {
                let s = s.read(cx);
                for (i, (pane, label)) in s.features().into_iter().enumerate() {
                    items.push(palette_cmd::Item {
                        title: format!("Feature: {label}"),
                        key: if i < 9 {
                            format!("Ctrl+Shift+{}", i + 1)
                        } else {
                            String::new()
                        },
                        cmd: Some(Cmd::Feature(pane)),
                        tag: None,
                    });
                }
            }
            self.open_palette(
                items,
                "Comando ou nome da feature…",
                "Nada encontrado",
                window,
                cx,
            );
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn open_palette(
        &mut self,
        items: Vec<palette_cmd::Item>,
        placeholder: &'static str,
        empty: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        {
            let theme = self.theme.clone();
            let p = cx.new(|cx| CommandPalette::new(theme, items, placeholder, empty, cx));
            cx.subscribe_in(&p, window, |this, _, e: &palette_cmd::Event, window, cx| {
                let cmd = match e {
                    palette_cmd::Event::Run(c) => Some(c.clone()),
                    palette_cmd::Event::Close => None,
                };
                this.toggle_palette(window, cx);
                if let Some(c) = cmd {
                    this.run(c, window, cx);
                }
            })
            .detach();
            window.focus(&p.read(cx).focus_handle(), cx);
            self.palette = Some(p);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn open_new_feature(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // o passo 4 do tutorial termina com este clique, como no protótipo
        if matches!(self.tour, Some((_, "feat", i)) if i == tour::FEAT.len() - 1) {
            self.end_tour(cx);
        }
        let Some(sidebar) = &self.sidebar else { return };
        let s = sidebar.read(cx);
        let (Some(repo), herdr) = (s.top_repo(), s.herdr()) else {
            return;
        };
        let theme = self.theme.clone();
        let d = cx.new(|cx| NewFeature::new(herdr, repo, theme, window, cx));
        cx.subscribe_in(&d, window, |this, _, _: &new_feature::Event, window, cx| {
            this.dialog = None;
            window.focus(&this.terminal.read(cx).focus_handle(), cx);
            cx.notify();
        })
        .detach();
        self.dialog = Some(d);
        cx.stop_propagation();
        cx.notify();
    }

    pub fn toast(&mut self, msg: String, cx: &mut Context<Self>) {
        self.toast_undo(msg, None, cx);
    }

    /// Toast com "Desfazer" (6 s, como no protótipo) quando vem o patch que refaz.
    fn toast_undo(&mut self, msg: String, redo: Option<Redo>, cx: &mut Context<Self>) {
        self.toast_seq += 1;
        let id = self.toast_seq;
        let ms = if redo.is_some() { 6000 } else { 2800 };
        self.toasts.push((id, msg, redo));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(ms))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.toasts.retain(|(i, _, _)| *i != id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// O `removeFeatureDlg` do protótipo, apagando o que o inventário registrou.
    fn remove_feature(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remove_feature_at(None, None, window, cx)
    }

    /// `root`: a worktree de uma feature de fundo (o MR dela mergeou); `None`, a feature em foco.
    fn remove_feature_at(
        &mut self,
        root: Option<std::path::PathBuf>,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let s = sidebar.read(cx);
        let wt = root.or_else(|| s.top_worktree());
        let focused = s
            .agents
            .iter()
            .find(|a| {
                wt.as_ref()
                    .is_some_and(|w| std::path::Path::new(&a.cwd).starts_with(w))
            })
            .cloned();
        let herdr = s.herdr();
        let Some(inv) = wt.as_deref().and_then(feature::find_by_path) else {
            let main = wt.as_ref().is_some_and(|w| herdr::main_repo(w) == *w);
            return self.toast(
                if main {
                    "A main é o checkout principal e não pode ser removida".into()
                } else {
                    "Este terminal não é uma feature criada pelo Maestro".into()
                },
                cx,
            );
        };
        let p = feature::pending(&inv);
        let working = focused.is_some_and(|a| a.agent_status == Status::Working);
        let warn = if p.uncommitted > 0 {
            format!(
                "{} arquivo{} não commitado{} {} junto com a worktree.",
                p.uncommitted,
                if p.uncommitted > 1 { "s" } else { "" },
                if p.uncommitted > 1 { "s" } else { "" },
                if p.uncommitted > 1 {
                    "vão ser apagados"
                } else {
                    "vai ser apagado"
                }
            )
        } else if working {
            "O Claude ainda está trabalhando nela e vai ser encerrado.".into()
        } else if p.merged {
            format!(
                "Nada pendente. A branch já foi mergeada em {} e sai junto com a pasta.",
                inv.base
            )
        } else {
            "Nada pendente. A branch local continua existindo; só a pasta da worktree sai.".into()
        };
        let text = format!(
            "{warn} Pasta: {}",
            crate::sidebar::tilde(&inv.worktrees[0].path.to_string_lossy())
        );
        let theme = self.theme.clone();
        let title = title.unwrap_or_else(|| format!("Remover a feature {}?", inv.branch));
        let danger = p.uncommitted > 0 || working;
        let c =
            cx.new(|cx| Confirm::new(theme, title, text, "Remover worktree", danger, window, cx));
        cx.subscribe_in(&c, window, move |this, _, a: &Answer, window, cx| {
            this.confirm = None;
            window.focus(&this.terminal.read(cx).focus_handle(), cx);
            if matches!(a, Answer::Ok) {
                let (herdr, inv) = (herdr.clone(), inv.clone());
                cx.spawn(async move |this, cx| {
                    let branch = inv.branch.clone();
                    let r = cx
                        .background_executor()
                        .spawn(async move { feature::conclude(&herdr, &inv) })
                        .await;
                    let _ = this.update(cx, |this, cx| match r {
                        Ok(_) => this.toast(format!("Worktree {branch} removida"), cx),
                        Err(e) => this.toast(format!("Não deu para remover: {e}"), cx),
                    });
                })
                .detach();
            }
            cx.notify();
        })
        .detach();
        self.confirm = Some(c);
        cx.notify();
    }

    /// O `compareDlg` do protótipo para o grupo da feature em foco.
    fn open_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let s = sidebar.read(cx);
        let wt = s.top_worktree();
        let group = wt
            .as_deref()
            .and_then(feature::find_by_path)
            .and_then(|i| i.group);
        let Some(group) =
            group.or_else(|| feature::inventories().into_iter().find_map(|i| i.group))
        else {
            return self.toast(
                "Nenhuma feature com várias tentativas. Crie uma com Tentativas: 2.".into(),
                cx,
            );
        };
        let mut invs: Vec<_> = feature::inventories()
            .into_iter()
            .filter(|i| i.group.as_ref() == Some(&group))
            .collect();
        invs.sort_by(|a, b| a.branch.cmp(&b.branch));
        let attempts: Vec<Attempt> = invs
            .into_iter()
            .map(|inv| {
                let path = &inv.worktrees[0].path;
                let agent = s
                    .agents
                    .iter()
                    .find(|a| std::path::Path::new(&a.cwd).starts_with(path));
                let st = agent.map_or(Status::Unknown, |a| a.agent_status);
                let ch = changes::changes(path).unwrap_or_default();
                Attempt {
                    state: (
                        crate::sidebar::glyph(st).1.to_string(),
                        self.status_color(st),
                    ),
                    done: matches!(st, Status::Done | Status::Idle),
                    files: ch.files.len(),
                    added: ch.files.iter().map(|f| f.added).sum(),
                    removed: ch.files.iter().map(|f| f.removed).sum(),
                    cost: agent.and_then(|a| s.usage.get(&a.pane_id)).map(|u| u.cost),
                    inv,
                }
            })
            .collect();
        let herdr = s.herdr();
        let theme = self.theme.clone();
        let invs: Vec<_> = attempts.iter().map(|a| a.inv.clone()).collect();
        let c = cx.new(|cx| Compare::new(group, attempts, theme, window, cx));
        cx.subscribe_in(
            &c,
            window,
            move |this, _, e: &compare::Event, window, cx| {
                this.compare = None;
                window.focus(&this.terminal.read(cx).focus_handle(), cx);
                if let compare::Event::Keep(i) = e {
                    let keep = invs[*i].clone();
                    let rest: Vec<_> = invs.iter().filter(|x| x.id != keep.id).cloned().collect();
                    let names: Vec<_> = rest.iter().map(|x| x.branch.clone()).collect();
                    let theme = this.theme.clone();
                    let herdr = herdr.clone();
                    let title = format!("Ficar com {}?", keep.branch);
                    let text =
                        format!("Remove {} (worktrees e branches locais).", names.join(", "));
                    let d = cx.new(|cx| {
                        Confirm::new(theme, title, text, "Ficar com esta", false, window, cx)
                    });
                    cx.subscribe_in(&d, window, move |this, _, a: &Answer, window, cx| {
                        this.confirm = None;
                        window.focus(&this.terminal.read(cx).focus_handle(), cx);
                        if matches!(a, Answer::Ok) {
                            let (herdr, keep, rest) = (herdr.clone(), keep.clone(), rest.clone());
                            cx.spawn(async move |this, cx| {
                                let n = rest.len();
                                let branch = keep.branch.clone();
                                let r = cx
                                    .background_executor()
                                    .spawn(async move {
                                        rest.iter()
                                            .try_for_each(|x| feature::discard(&herdr, x))?;
                                        feature::keep(&keep)
                                    })
                                    .await;
                                let _ = this.update(cx, |this, cx| match r {
                                    Ok(()) => this.toast(
                                        format!(
                                            "Ficou {branch}; {n} tentativa{} removida{}",
                                            if n > 1 { "s" } else { "" },
                                            if n > 1 { "s" } else { "" }
                                        ),
                                        cx,
                                    ),
                                    Err(e) => this
                                        .toast(format!("Não deu para remover as outras: {e}"), cx),
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    })
                    .detach();
                    this.confirm = Some(d);
                }
                cx.notify();
            },
        )
        .detach();
        self.compare = Some(c);
        cx.notify();
    }

    /// Painel "Revisar" da feature em foco (`Ctrl+Shift+E`); com `path`, abre já nesse arquivo.
    fn toggle_review(&mut self, path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let root = self
            .sidebar
            .as_ref()
            .and_then(|s| s.read(cx).top_worktree());
        if path.is_none() && self.review.take().is_some() {
            window.focus(&self.terminal.read(cx).focus_handle(), cx);
            return cx.notify();
        }
        let Some(root) = root else {
            return self.toast("Abra um terminal num repositório para revisar".into(), cx);
        };
        self.open_review_at(root, path, window, cx)
    }

    /// O painel de uma worktree escolhida (a da fila, que pode ainda não estar em foco).
    fn open_review_at(
        &mut self,
        root: std::path::PathBuf,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same = self
            .review
            .as_ref()
            .is_some_and(|r| r.read(cx).root == root);
        if !same {
            let branch = herdr::branch(&root).unwrap_or_default();
            let theme = self.theme.clone();
            let anchors = self.anchors.clone();
            let r = cx.new(|cx| Review::new(root.clone(), branch.clone(), theme, anchors, cx));
            cx.observe(&r, |this, r, cx| {
                // o arquivo aberto (e se está sujo) aparece marcado na aba Arquivos da lateral
                let open = r.read(cx).current_file(cx);
                if let Some(s) = &this.sidebar {
                    s.update(cx, |s, cx| s.set_open(open, cx));
                }
                cx.notify();
            })
            .detach();
            cx.subscribe_in(
                &r,
                window,
                move |this, r, e: &review::Event, window, cx| match e {
                    review::Event::Close => {
                        this.review = None;
                        window.focus(&this.terminal.read(cx).focus_handle(), cx);
                        cx.notify();
                    }
                    review::Event::Toast(m, redo) => this.toast_undo(m.clone(), redo.clone(), cx),
                review::Event::RunTests(cmd) => {
                    let root = r.read(cx).root.clone();
                    let Some(s) = this.sidebar.clone() else { return };
                    let s = s.read(cx);
                    let target = s
                        .agents
                        .iter()
                        .find(|a| std::path::Path::new(&a.cwd).starts_with(&root))
                        .map(|a| a.pane_id.clone());
                    let (herdr, cmd) = (s.herdr(), cmd.clone());
                    // um split embaixo do Claude da feature, como no protótipo
                    std::thread::spawn(move || {
                        let p = herdr.call(
                            "pane.split",
                            serde_json::json!({ "direction": "down", "cwd": root, "target_pane_id": target, "focus": false }),
                        )?;
                        let pane = p["pane"]["pane_id"].as_str().unwrap_or_default().to_string();
                        herdr.call("pane.send_text", serde_json::json!({ "pane_id": pane, "text": format!("{cmd}\r") }))
                    });
                    this.toast("Testes afetados rodando num split".into(), cx);
                }
                    review::Event::Send(text) => {
                        let root = r.read(cx).root.clone();
                        this.ask_claude(
                            &root,
                            text.clone(),
                            format!("Revisão enviada ao Claude de {branch}"),
                            cx,
                        )
                    }
                    review::Event::Ask(text, ok) => {
                        let root = r.read(cx).root.clone();
                        this.ask_claude(&root, text.clone(), ok.clone(), cx)
                    }
                    review::Event::Conclude => {
                        let root = r.read(cx).root.clone();
                        this.remove_feature_at(Some(root), None, window, cx)
                    }
                    review::Event::MrOpened => {
                        if this.tour.is_none() && !tour::seen("mrtab") {
                            this.tour = Some((tour::MRTAB, "mrtab", 0));
                            cx.notify();
                        }
                    }
                },
            )
            .detach();
            self.review = Some(r);
            if self.tour.is_none() && !tour::seen("review") {
                self.tour = Some((tour::REVIEW, "review", 0));
            }
        }
        if let (Some(p), Some(r)) = (path, &self.review) {
            r.update(cx, |r, cx| r.show_file(p, cx));
        }
        if let Some(r) = &self.review {
            let f = r.read(cx).focus_handle(cx);
            window.focus(&f, cx);
        }
        cx.notify();
    }

    /// O `learnPerm` do protótipo: sempre permitir, só para você ou para o time.
    fn ask_learn(
        &mut self,
        s: crate::perms::Suggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let repo = s
            .repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = s
            .rule
            .strip_prefix("Bash(")
            .and_then(|r| r.strip_suffix(":*)"))
            .unwrap_or(&s.rule)
            .to_string();
        let title = format!("Você aprovou `{key}` {} vezes em {repo}", s.count);
        let text = format!(
            "Em {} feature{} nesta semana. Sempre permitir `{}` neste repo?",
            s.features,
            if s.features > 1 { "s" } else { "" },
            s.rule
        );
        let options = vec![
            ("Agora não".to_string(), false),
            (
                "Só pra mim (.claude/settings.local.json)".to_string(),
                false,
            ),
            (
                "Para o time (.claude/settings.json, vai no git)".to_string(),
                true,
            ),
        ];
        let theme = self.theme.clone();
        let c = cx.new(|cx| Choice::new(theme, title, text, options, window, cx));
        cx.subscribe_in(&c, window, move |this, _, Chose(i), window, cx| {
            this.choice = None;
            window.focus(&this.terminal.read(cx).focus_handle(), cx);
            crate::perms::mark(&s);
            let r = match i {
                Some(1) => crate::perms::allow_personal(&s)
                    .map(|_| format!("Permitido para você: {}", s.rule)),
                Some(2) => crate::perms::allow_team(&s).map(|_| {
                    format!(
                        "{} adicionado ao .claude/settings.json. Entra no próximo commit.",
                        s.rule
                    )
                }),
                _ => return cx.notify(),
            };
            match r {
                Ok(m) => this.toast(m, cx),
                Err(e) => this.toast(format!("Não deu para gravar a regra: {e}"), cx),
            }
        })
        .detach();
        self.choice = Some(c);
        cx.notify();
    }

    /// O `notify(f)` do protótipo: quem está olhando a feature não precisa do mascote.
    fn notify_note(&mut self, n: Note, cx: &mut Context<Self>) {
        let focused = self.sidebar.as_ref().and_then(|s| {
            s.read(cx)
                .agents
                .iter()
                .find(|a| a.focused)
                .map(|a| a.pane_id.clone())
        });
        if self.window_active && focused.as_ref() == Some(&n.pane) {
            return;
        }
        let shown = self
            .mascot
            .as_ref()
            .is_some_and(|(m, _, _)| m.pane == n.pane);
        if !shown && !self.mq.iter().any(|m| m.pane == n.pane) {
            self.mq.push(n);
        }
        if self.mascot.is_none() {
            self.next_mascot(cx);
        }
        cx.notify();
    }

    fn next_mascot(&mut self, cx: &mut Context<Self>) {
        if self.mq.is_empty() {
            self.mascot = None;
            self._blink = None;
            return cx.notify();
        }
        let n = self.mq.remove(0);
        mascot::play(n.kind);
        self.mascot_seq += 1;
        self.mascot = Some((n, self.mascot_seq, false));
        // o piscar do protótipo: um quadro a cada 450 ms
        self._blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(450))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.mascot_frame += 1;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn dismiss_mascot(&mut self, cx: &mut Context<Self>) {
        let Some((n, seq, false)) = self.mascot.clone() else {
            return;
        };
        self.mq.retain(|m| m.pane != n.pane);
        self.mascot = Some((n, seq, true));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(700))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.mascot.as_ref().is_some_and(|m| m.1 == seq) {
                    this.mascot = None;
                    this.next_mascot(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// `agent.prompt` no Claude da worktree; o toast diz se chegou.
    fn ask_claude(
        &mut self,
        root: &std::path::Path,
        text: String,
        ok: String,
        cx: &mut Context<Self>,
    ) {
        let (Some(pane), Some(s)) = (self.claude_pane(root, cx), self.sidebar.clone()) else {
            return self.toast("Nenhum Claude rodando nesta worktree".into(), cx);
        };
        let herdr = s.read(cx).herdr();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    herdr.call(
                        "agent.prompt",
                        serde_json::json!({ "target": pane, "text": text }),
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(_) => this.toast(ok, cx),
                Err(e) => this.toast(format!("O Claude não recebeu: {e}"), cx),
            });
        })
        .detach();
    }

    /// O pane do Claude que roda na worktree `root`.
    fn claude_pane(&self, root: &std::path::Path, cx: &App) -> Option<String> {
        let s = self.sidebar.as_ref()?.read(cx);
        s.agents
            .iter()
            .find(|a| std::path::Path::new(&a.cwd).starts_with(root))
            .map(|a| a.pane_id.clone())
    }

    /// O `sendSel` do protótipo: `@arquivo:linhas` digitado no Claude da feature, sem Enter.
    fn send_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.clone() else {
            return self.toast("Selecione linhas no editor ou no diff".into(), cx);
        };
        let (root, sel) = {
            let r = r.read(cx);
            (r.root.clone(), r.selection_ref(cx))
        };
        let Some(sel) = sel else {
            return self.toast("Selecione linhas no editor ou no diff".into(), cx);
        };
        let (Some(pane), Some(s)) = (self.claude_pane(&root, cx), self.sidebar.clone()) else {
            return self.toast(
                "Este terminal não tem Claude; escolha uma feature".into(),
                cx,
            );
        };
        let s = s.read(cx);
        let herdr = s.herdr();
        s.focus(pane.clone());
        std::thread::spawn(move || {
            herdr.call(
                "pane.send_text",
                serde_json::json!({ "pane_id": pane, "text": format!("{sel} ") }),
            )
        });
        window.focus(&self.terminal.read(cx).focus_handle(), cx);
        cx.stop_propagation();
        cx.notify();
    }

    /// "Arquivo: Abrir…": os arquivos da worktree em foco, na mesma paleta.
    fn open_file_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self
            .sidebar
            .as_ref()
            .and_then(|s| s.read(cx).top_worktree())
        else {
            return self.toast(
                "Abra um terminal num repositório para abrir arquivos".into(),
                cx,
            );
        };
        let items = crate::changes::files(&root)
            .into_iter()
            .map(|p| palette_cmd::Item {
                title: p.clone(),
                key: String::new(),
                cmd: Some(Cmd::OpenFile(p)),
                tag: None,
            })
            .collect();
        self.open_palette(items, "Nome do arquivo…", "Nada encontrado", window, cx);
    }

    fn run(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = self.sidebar.clone();
        let focus = |pane: Option<String>, cx: &mut Context<Self>| {
            if let (Some(s), Some(p)) = (&sidebar, pane) {
                s.read(cx).focus(p);
            }
        };
        match cmd {
            // o `nextAttention` do protótipo
            Cmd::NextAttention => {
                let q = sidebar
                    .as_ref()
                    .map(|s| s.read(cx).attention())
                    .unwrap_or_default();
                let Some(first) = q.first() else {
                    return self.toast("Fila vazia: nada esperando você".into(), cx);
                };
                let rest = if q.len() > 1 {
                    format!(" · faltam {}", q.len() - 1)
                } else {
                    String::new()
                };
                self.toast(format!("Fila: {}{rest}", first.text), cx);
                return self.run(Cmd::Go(first.go.clone()), window, cx);
            }
            Cmd::AttentionMenu => {
                let t = self.theme.clone();
                let items = sidebar
                    .as_ref()
                    .map(|s| s.read(cx).attention())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|i| palette_cmd::Item {
                        title: i.text,
                        key: String::new(),
                        tag: Some((
                            i.tag.into(),
                            match i.p {
                                0 => t.orange,
                                1 => rgb(0xff5555).into(),
                                2 => t.ok,
                                3 => t.accent,
                                _ => t.muted,
                            },
                        )),
                        cmd: Some(Cmd::Go(i.go)),
                    })
                    .collect();
                return self.open_palette(
                    items,
                    "Fila de atenção: filtrar…",
                    "Fila vazia. Nada esperando você.",
                    window,
                    cx,
                );
            }
            Cmd::Go(go) => {
                let pane = match &go {
                    Go::Pane(p) | Go::Mr(p, _) | Go::Review(p, _) | Go::Compare(p) => p.clone(),
                };
                focus(Some(pane), cx);
                match go {
                    Go::Pane(_) => {}
                    Go::Mr(_, wt) => {
                        self.open_review_at(wt, None, window, cx);
                        if let Some(r) = &self.review {
                            r.update(cx, |r, cx| r.open_mr(cx));
                        }
                        return cx.notify();
                    }
                    Go::Review(_, wt) => return self.open_review_at(wt, None, window, cx),
                    Go::Compare(_) => return self.open_compare(window, cx),
                }
            }
            Cmd::Blocked => focus(
                sidebar.as_ref().and_then(|s| s.read(cx).first_blocked()),
                cx,
            ),
            Cmd::Feature(p) => focus(Some(p), cx),
            Cmd::Tutorial => {
                let on_mr = self.review.as_ref().is_some_and(|r| r.read(cx).on_mr());
                self.tour = Some(if on_mr {
                    (tour::MRTAB, "mrtab", 0)
                } else if self.review.is_some() {
                    (tour::REVIEW, "review", 0)
                } else {
                    (tour::FEAT, "feat", 0)
                })
            }
            Cmd::Herdr(a) => self.terminal.update(cx, |t, _| t.herdr_action(a)),
            Cmd::Interrupt => self.terminal.update(cx, |t, _| t.send(b"\x1b")),
            Cmd::ToggleLeft => self.left = !self.left,
            Cmd::Features => {}
            Cmd::NewFeature => return self.open_new_feature(window, cx),
            Cmd::RemoveFeature => return self.remove_feature(window, cx),
            Cmd::Compare => return self.open_compare(window, cx),
            Cmd::Review => return self.toggle_review(None, window, cx),
            Cmd::SendSelection => return self.send_selection(window, cx),
            Cmd::OpenFileMenu => return self.open_file_menu(window, cx),
            Cmd::OpenFile(p) => {
                if self.review.is_none() {
                    self.toggle_review(None, window, cx);
                }
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| r.open_file(p, window, cx));
                }
                return cx.notify();
            }
            Cmd::OpenMr => {
                if self.review.is_none() {
                    self.toggle_review(None, window, cx);
                }
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| r.open_mr(cx));
                }
                return cx.notify();
            }
            Cmd::Save => {
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| r.save(cx));
                }
                return;
            }
            Cmd::Affected | Cmd::SendReview => {
                if self.review.is_none() {
                    self.toggle_review(None, window, cx);
                }
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| match cmd {
                        Cmd::Affected => r.run_tests(cx),
                        _ => r.send(cx),
                    });
                }
                return;
            }
            Cmd::AddRepoMenu => {
                let wt = self
                    .sidebar
                    .as_ref()
                    .and_then(|s| s.read(cx).top_worktree());
                let Some(inv) = wt.as_deref().and_then(feature::find_by_path) else {
                    return self.toast(
                        "Abra uma feature criada pelo Maestro para adicionar um repo".into(),
                        cx,
                    );
                };
                let items = feature::sibling_repos(&inv.repo)
                    .into_iter()
                    .filter(|r| !inv.worktrees.iter().any(|w| w.repo == *r))
                    .map(|r| palette_cmd::Item {
                        title: r
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        key: String::new(),
                        cmd: Some(Cmd::AddRepo(r)),
                        tag: None,
                    })
                    .collect();
                return self.open_palette(
                    items,
                    "Repositório para adicionar à feature…",
                    "Nenhum repositório irmão para adicionar",
                    window,
                    cx,
                );
            }
            Cmd::AddRepo(repo) => {
                let wt = self
                    .sidebar
                    .as_ref()
                    .and_then(|s| s.read(cx).top_worktree());
                let (Some(mut inv), Some(herdr)) = (
                    wt.as_deref().and_then(feature::find_by_path),
                    self.sidebar.as_ref().map(|s| s.read(cx).herdr()),
                ) else {
                    return;
                };
                let name = repo
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                cx.spawn(async move |this, cx| {
                    let r = cx
                        .background_executor()
                        .spawn(async move { feature::add_repo(&herdr, &mut inv, &repo) })
                        .await;
                    let _ = this.update(cx, |this, cx| match r {
                        Ok(_) => this.toast(
                            format!("{name} entrou na feature; /add-dir enviado ao Claude"),
                            cx,
                        ),
                        Err(e) => this.toast(format!("Não deu para adicionar {name}: {e}"), cx),
                    });
                })
                .detach();
            }
            Cmd::Cleanup => {
                let repos = self
                    .sidebar
                    .as_ref()
                    .map(|s| s.read(cx).repo_paths())
                    .unwrap_or_default();
                let theme = self.theme.clone();
                let c = cx.new(|cx| Cleanup::new(repos, theme, window, cx));
                cx.subscribe_in(&c, window, |this, _, e: &cleanup::Event, window, cx| {
                    let cleanup::Event::Done(msg) = e;
                    this.cleanup = None;
                    if let Some(m) = msg.clone() {
                        this.toast(m, cx);
                    }
                    window.focus(&this.terminal.read(cx).focus_handle(), cx);
                    cx.notify();
                })
                .detach();
                self.cleanup = Some(c);
                cx.notify();
                return;
            }
        }
        window.focus(&self.terminal.read(cx).focus_handle(), cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn advance_tour(&mut self, cx: &mut Context<Self>) {
        match self.tour {
            Some((steps, id, i)) if i + 1 < steps.len() => self.tour = Some((steps, id, i + 1)),
            _ => return self.end_tour(cx),
        }
        cx.notify();
    }

    fn end_tour(&mut self, cx: &mut Context<Self>) {
        if let Some((_, id, _)) = self.tour.take() {
            tour::mark(id);
        }
        cx.notify();
    }

    /// Posições dos alvos relativas à raiz (a janela sem decoração tem margem de sombra).
    fn relative_anchors(&self, origin: Point<Pixels>) -> tour::Anchors {
        let map = self
            .anchors
            .borrow()
            .iter()
            .map(|(k, b)| (*k, Bounds::new(b.origin - origin, b.size)))
            .collect();
        std::rc::Rc::new(std::cell::RefCell::new(map))
    }

    fn kbd(&self, k: &str) -> Div {
        let t = &self.theme;
        div()
            .font_family(t.mono.clone())
            .text_size(px(11.))
            .px(px(4.))
            .border_1()
            .border_color(t.line)
            .rounded(px(3.))
            .text_color(t.muted)
            .bg(t.panel)
            .whitespace_nowrap()
            .child(k.to_string())
    }

    fn dot(&self, id: &'static str, action: fn(&mut Window)) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(id)
            .size(px(10.))
            .rounded_full()
            .bg(t.line)
            .hover(|d| d.bg(t.muted))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, _| action(window))
    }

    fn title(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let (blocked, attention) = self.sidebar.as_ref().map_or((0, 0), |s| {
            let s = s.read(cx);
            let n = s
                .agents
                .iter()
                .filter(|x| x.agent_status == Status::Blocked)
                .count();
            (n, s.attention().len())
        });
        let mode = |label: &str, on: bool, count: Option<String>| {
            div()
                .flex()
                .gap(px(6.))
                .items_center()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(5.))
                .text_color(if on { t.fg } else { t.muted })
                .when(on, |d| d.bg(t.raise))
                // ponytail: MRs e Vault são as Etapas 5 e 6; até lá ficam visíveis e apagados
                .when(!on, |d| d.opacity(0.45))
                .child(label.to_string())
                .when_some(count, |d, c| {
                    d.child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(11.))
                            .text_color(t.orange)
                            .child(c),
                    )
                })
        };
        div()
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .bg(t.panel)
            .border_b_1()
            .border_color(t.line)
            // decoração própria (a do protótipo): arrastar move, dois cliques maximizam
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(self.dot("close", |w| w.remove_window()))
                    .child(self.dot("min", |w| w.minimize_window()))
                    .child(self.dot("max", |w| w.zoom_window())),
            )
            .child(div().font_weight(FontWeight::SEMIBOLD).child("maestro"))
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .ml(px(8.))
                    .child(mode(
                        "Features",
                        true,
                        (blocked > 0).then(|| format!("{blocked} !")),
                    ))
                    .child(mode("MRs", false, None))
                    .child(mode("Vault", false, None)),
            )
            .child(
                div()
                    .id("fila")
                    .relative()
                    .child(tour::anchor(&self.anchors, "fila"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(t.line)
                    .cursor_pointer()
                    .hover(|d| d.bg(t.raise))
                    .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::AttentionMenu, w, cx)))
                    .child("Fila")
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.orange)
                            .child(attention.to_string()),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .text_color(t.muted)
                    .child(
                        div()
                            .id("files")
                            .flex()
                            .gap(px(6.))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, w, cx| this.open_file_menu(w, cx)))
                            .child(self.kbd("Ctrl+Shift+O"))
                            .child("arquivo"),
                    )
                    .child(
                        div()
                            .id("cmds")
                            .flex()
                            .gap(px(6.))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, w, cx| this.toggle_palette(w, cx)))
                            .child(self.kbd("Ctrl+Shift+P"))
                            .child("comandos"),
                    )
                    .child(div().opacity(0.45).child(self.kbd("?"))),
            )
    }

    /// Abas do workspace em foco, como no `.hbar` do protótipo (a barra do próprio herdr fica escondida).
    fn hbar(&self, cx: &App) -> Div {
        let t = &self.theme;
        let mut bar = div()
            .flex()
            .flex_none()
            .items_center()
            .bg(t.panel)
            .border_b_1()
            .border_color(rgb(0x191a21))
            .font_family(t.mono.clone())
            .text_size(px(12.))
            .whitespace_nowrap()
            .overflow_hidden();
        let Some(sidebar) = &self.sidebar else {
            return bar;
        };
        let s = sidebar.read(cx);
        let ws = s
            .workspaces
            .iter()
            .find(|w| w.focused)
            .map(|w| w.workspace_id.clone());
        for (i, tab) in s
            .tabs
            .iter()
            .filter(|x| Some(&x.workspace_id) == ws.as_ref())
            .enumerate()
        {
            let agent = s.agents.iter().find(|a| a.tab_id == tab.tab_id);
            let id = tab.tab_id.clone();
            let side = sidebar.clone();
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("tab:{}", tab.tab_id)))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .py(px(3.))
                    .cursor_pointer()
                    .text_color(if tab.focused { t.fg } else { t.comment })
                    .when(tab.focused, |d| d.bg(t.win))
                    .on_click(move |_, _, cx| side.read(cx).focus_tab(id.clone()))
                    .child(format!(
                        "{} {}",
                        i + 1,
                        if agent.is_some() { "claude" } else { "zsh" }
                    ))
                    .when_some(
                        agent.filter(|a| !matches!(a.agent_status, Status::Idle | Status::Unknown)),
                        |d, a| {
                            let (g, _) = crate::sidebar::glyph(a.agent_status);
                            d.child(
                                div()
                                    .ml(px(6.))
                                    .text_color(self.status_color(a.agent_status))
                                    .child(g),
                            )
                        },
                    ),
            );
        }
        if let Some(ws) = ws {
            let side = sidebar.clone();
            bar = bar.child(
                div()
                    .id("newtab")
                    .px(px(12.))
                    .py(px(3.))
                    .cursor_pointer()
                    .text_color(t.comment)
                    .on_click(move |_, _, cx| side.read(cx).new_tab(ws.clone()))
                    .child("+"),
            );
        }
        bar = bar.child(div().flex_1());
        let hint = div().px(px(10.)).py(px(3.));
        if self.prefix {
            bar.child(
                hint.bg(rgb(0xf1fa8c))
                    .text_color(rgb(0x282a36))
                    .font_weight(FontWeight::BOLD)
                    .child("PREFIX: v divide · - empilha · c aba · x fecha · z zoom"),
            )
        } else {
            bar.child(
                hint.text_color(t.comment)
                    .child("Ctrl+Shift+\\ divide · Ctrl+Shift+Enter empilha"),
            )
        }
    }

    fn status_color(&self, s: Status) -> Hsla {
        let t = &self.theme;
        match s {
            Status::Blocked => t.orange,
            Status::Working => t.cyan,
            Status::Done => t.ok,
            Status::Idle | Status::Unknown => t.comment,
        }
    }

    fn hints(&self) -> Div {
        let t = &self.theme;
        let mut bar = div()
            .h(px(22.))
            .flex_none()
            .flex()
            .gap(px(14.))
            .items_center()
            .px(px(12.))
            .text_size(px(11.5))
            .text_color(t.muted)
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .overflow_hidden()
            .whitespace_nowrap();
        for (k, label, ready) in HINTS {
            bar = bar.child(
                div()
                    .flex()
                    .gap(px(4.))
                    .items_center()
                    .when(!ready, |d| d.opacity(0.45))
                    .child(self.kbd(k))
                    .child(label.to_string()),
            );
        }
        bar
    }

    fn status(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let cell = || {
            div()
                .h_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .border_r_1()
                .border_color(t.line)
        };
        let mut bar = div()
            .h(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .text_size(px(12.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .overflow_hidden()
            .whitespace_nowrap();
        match &self.sidebar {
            None => bar = bar.child(cell().child("herdr: desligado")),
            Some(s) => {
                let s = s.read(cx);
                let blocked = s
                    .agents
                    .iter()
                    .filter(|a| a.agent_status == Status::Blocked)
                    .count();
                let working = s
                    .agents
                    .iter()
                    .filter(|a| a.agent_status == Status::Working)
                    .count();
                bar = bar.child(cell().child(if s.offline {
                    "herdr: fora do ar"
                } else {
                    "herdr: conectado"
                }));
                if blocked > 0 {
                    let sidebar = self.sidebar.clone().unwrap();
                    bar = bar.child(
                        cell()
                            .id("att")
                            .text_color(t.orange)
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                let s = sidebar.read(cx);
                                if let Some(p) = s.first_blocked() {
                                    s.focus(p);
                                }
                            })
                            .child(format!(
                                "! {blocked} precisa{} de você",
                                if blocked > 1 { "m" } else { "" }
                            )),
                    );
                }
                bar = bar.child(cell().child(format!("{working} trabalhando")));
                bar = bar.child(
                    cell().child(format!("hoje: US$ {}", crate::sidebar::brl(s.today_cost()))),
                );
                let focused = s.agents.iter().find(|a| a.focused);
                if let Some(b) = focused.and_then(|a| {
                    herdr::repo_root(std::path::Path::new(&a.cwd)).and_then(herdr::branch)
                }) {
                    bar = bar.child(cell().child(format!("⎇ {b}")));
                }
            }
        }
        let cursor = self.review.as_ref().and_then(|r| r.read(cx).cursor(cx));
        bar.child(div().flex_1())
            .children(cursor.map(|(l, c)| cell().child(format!("Ln {l}, Col {c}"))))
            .child(cell().border_r_0().child(format!("ghostty: {}", self.font)))
    }
}

/// Bordas de redimensionar da janela sem decoração do sistema.
fn resize_edges() -> impl Iterator<Item = Div> {
    use ResizeEdge::*;
    const E: f32 = 5.;
    [
        Top,
        Bottom,
        Left,
        Right,
        TopLeft,
        TopRight,
        BottomLeft,
        BottomRight,
    ]
    .into_iter()
    .map(|edge| {
        let d = div().absolute();
        let d = match edge {
            Top => d
                .top_0()
                .left(px(E))
                .right(px(E))
                .h(px(E))
                .cursor_ns_resize(),
            Bottom => d
                .bottom_0()
                .left(px(E))
                .right(px(E))
                .h(px(E))
                .cursor_ns_resize(),
            Left => d
                .left_0()
                .top(px(E))
                .bottom(px(E))
                .w(px(E))
                .cursor_ew_resize(),
            Right => d
                .right_0()
                .top(px(E))
                .bottom(px(E))
                .w(px(E))
                .cursor_ew_resize(),
            TopLeft => d.top_0().left_0().size(px(E)).cursor_nwse_resize(),
            BottomRight => d.bottom_0().right_0().size(px(E)).cursor_nwse_resize(),
            TopRight => d.top_0().right_0().size(px(E)).cursor_nesw_resize(),
            BottomLeft => d.bottom_0().left_0().size(px(E)).cursor_nesw_resize(),
        };
        d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            window.start_window_resize(edge);
        })
    })
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        self.window_active = window.is_window_active();
        if std::mem::take(&mut self.want_dialog) {
            self.open_new_feature(window, cx);
        }
        if self.review.is_none()
            && let Some(s) = &self.sidebar
        {
            s.update(cx, |s, cx| s.set_open(None, cx));
        }
        if self.confirm.is_none()
            && self.choice.is_none()
            && self.dialog.is_none()
            && !self.learn.is_empty()
        {
            let s = self.learn.remove(0);
            self.ask_learn(s, window, cx);
        }
        if self.confirm.is_none()
            && let Some((wt, iid, branch)) = self.want_merged.take()
        {
            let title = format!("!{iid} foi mergeado. Remover a worktree {branch}?");
            self.remove_feature_at(Some(wt), Some(title), window, cx);
        }
        if let Some(p) = self.want_file.take() {
            self.run(Cmd::OpenFile(p), window, cx);
        }
        if let Some(p) = self.want_review.take() {
            self.toggle_review(p, window, cx);
        }
        let overlay = self.tour.and_then(|(steps, _, i)| {
            let root = self.anchors.borrow().get("root").copied()?;
            let next = cx.listener(|this: &mut Self, _: &(), _, cx| this.advance_tour(cx));
            let skip = cx.listener(|this: &mut Self, _: &(), _, cx| this.end_tour(cx));
            let o = tour::overlay(
                steps,
                i,
                &self.relative_anchors(root.origin),
                root.size,
                &t,
                move |w, cx| next(&(), w, cx),
                move |w, cx| skip(&(), w, cx),
            );
            if o.is_none() {
                // o alvo só tem posição depois do primeiro frame
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
            o
        });
        div()
            .relative()
            .child(tour::anchor(&self.anchors, "root"))
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key != "escape" {
                    return;
                }
                // a ordem do Esc do protótipo: tutorial, diálogos, e só então o mascote
                let overlay = this.palette.is_some()
                    || this.dialog.is_some()
                    || this.confirm.is_some()
                    || this.cleanup.is_some()
                    || this.compare.is_some()
                    || this.choice.is_some();
                if this.tour.is_some() {
                    this.end_tour(cx);
                } else if !overlay && this.mascot.as_ref().is_some_and(|m| !m.2) {
                    this.dismiss_mascot(cx);
                } else {
                    return;
                }
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(13.))
            .child(self.title(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .children(self.sidebar.clone().filter(|_| self.left))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(t.win)
                            .relative()
                            .child(tour::anchor(&self.anchors, "term"))
                            .child(self.hbar(cx))
                            .child(div().flex_1().min_h_0().child(self.terminal.clone())),
                    )
                    .children(self.review.clone().map(|r| {
                        div()
                            .w(relative(0.46))
                            .max_w(px(640.))
                            .flex_none()
                            .h_full()
                            .border_l_1()
                            .border_color(t.line)
                            .child(r)
                    })),
            )
            .child(self.hints())
            .child(self.status(cx))
            .children(overlay)
            .children(self.compare.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(24.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .child(d)
            }))
            .children(self.cleanup.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .child(d)
            }))
            .children(self.choice.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    .occlude()
                    .child(d)
            }))
            .children(self.confirm.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .child(d)
            }))
            .when_some(self.mascot.clone(), |d, (n, seq, leaving)| {
                let pane = n.pane.clone();
                d.child(
                    div()
                        .id("mascot")
                        .absolute()
                        .right(px(22.))
                        .bottom(px(58.))
                        .occlude()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = &this.sidebar {
                                s.read(cx).focus(pane.clone());
                            }
                            this.dismiss_mascot(cx);
                        }))
                        .child(mascot::view(
                            &t,
                            &n,
                            self.mq.len(),
                            self.mascot_frame,
                            seq,
                            leaving,
                        )),
                )
            })
            .when(!self.toasts.is_empty(), |d| {
                let mut col = div()
                    .absolute()
                    .bottom(px(56.))
                    .left_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.));
                for (id, m, redo) in &self.toasts {
                    let mut toast = confirm::toast(&t, m).flex().gap(px(12.)).items_center();
                    if let Some(redo) = redo.clone() {
                        let id = *id;
                        toast = toast.child(
                            div()
                                .id(("undo", id))
                                .underline()
                                .font_weight(FontWeight::SEMIBOLD)
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toasts.retain(|(i, _, _)| *i != id);
                                    let r = match &redo {
                                        Redo::Patch(root, patch) => {
                                            crate::diff::apply(root, patch, false)
                                        }
                                        Redo::Restore(root, sha) => feature::restore(root, sha),
                                    };
                                    if let Err(e) = r {
                                        this.toast(format!("Não deu para refazer: {e}"), cx);
                                    }
                                    cx.notify();
                                }))
                                .child("Desfazer"),
                        );
                    }
                    col = col.child(toast);
                }
                d.child(col)
            })
            .children(self.dialog.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .child(d)
            }))
            .children(self.palette.clone().map(|p| {
                // o `.scrim` do protótipo: clique fora fecha
                div()
                    .id("scrim")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, w, cx| this.toggle_palette(w, cx)),
                    )
                    .child(
                        div()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(p),
                    )
            }))
            // por último, para ficar por cima do conteúdo
            .children(resize_edges())
    }
}
