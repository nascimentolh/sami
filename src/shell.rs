use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    cleanup::{self, Cleanup},
    confirm::{self, Answer, Confirm},
    feature,
    herdr::{self, Status},
    keys,
    new_feature::{self, NewFeature},
    palette_cmd::{self, Cmd, CommandPalette},
    sidebar::{Sidebar, SidebarEvent},
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
    tour: Option<usize>,
    anchors: Anchors,
    palette: Option<Entity<CommandPalette>>,
    dialog: Option<Entity<NewFeature>>,
    confirm: Option<Entity<Confirm>>,
    cleanup: Option<Entity<Cleanup>>,
    toasts: Vec<(usize, String)>,
    toast_seq: usize,
    want_dialog: bool,
    left: bool,
}

/// Dicas do modo Features do protótipo; `false` = ainda não existe e aparece apagada.
const HINTS: &[(&str, &str, bool)] = &[
    ("Ctrl+Shift+Espaço", "próximo da fila", true),
    ("Ctrl+Shift+1…9", "feature", true),
    ("Ctrl+Shift+E", "revisar mudanças", false),
    ("Ctrl+Shift+\\", "dividir", true),
    ("Ctrl+Shift+Enter", "empilhar", true),
    ("Ctrl+Shift+←→", "trocar pane", true),
    ("Ctrl+A", "herdr", true),
    ("Ctrl+Shift+L", "seleção → Claude", false),
    ("Alt+N", "nova feature", false),
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
            cx.observe(s, |_, _, cx| cx.notify()).detach();
            // o botão da lateral não tem a janela; o diálogo abre no próximo render
            cx.subscribe(s, |this: &mut Self, _, _: &SidebarEvent, cx| {
                this.want_dialog = true;
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
            tour: (sidebar_present && !tour::seen("feat")).then_some(0),
            anchors,
            palette: None,
            dialog: None,
            confirm: None,
            cleanup: None,
            toasts: Vec::new(),
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
                "b" => return self.run(Cmd::ToggleLeft, window, cx),
                _ => {}
            }
        }
        let Some(sidebar) = &self.sidebar else { return };
        let s = sidebar.read(cx);
        let target = if let Some(n) = keys::feature_digit(k) {
            s.order().into_iter().nth(n - 1)
        } else if m.control && m.shift && !m.alt && k.key == "space" {
            s.next_attention()
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
                    });
                }
            }
            let theme = self.theme.clone();
            let p = cx.new(|cx| CommandPalette::new(theme, items, cx));
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
        if self.tour == Some(tour::FEAT.len() - 1) {
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
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.toasts.push((id, msg));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(2800))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.toasts.retain(|(i, _)| *i != id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// O `removeFeatureDlg` do protótipo, apagando o que o inventário registrou.
    fn remove_feature(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let s = sidebar.read(cx);
        let focused = s.agents.iter().find(|a| a.focused).cloned();
        let wt = s.top_worktree();
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
                "{} arquivo{} não commitado{} vai ser apagado junto com a worktree.",
                p.uncommitted,
                if p.uncommitted > 1 { "s" } else { "" },
                if p.uncommitted > 1 { "s" } else { "" }
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
        let title = format!("Remover a feature {}?", inv.branch);
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

    fn run(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = self.sidebar.clone();
        let focus = |pane: Option<String>, cx: &mut Context<Self>| {
            if let (Some(s), Some(p)) = (&sidebar, pane) {
                s.read(cx).focus(p);
            }
        };
        match cmd {
            Cmd::NextAttention => focus(
                sidebar.as_ref().and_then(|s| s.read(cx).next_attention()),
                cx,
            ),
            Cmd::Blocked => focus(
                sidebar.as_ref().and_then(|s| s.read(cx).first_blocked()),
                cx,
            ),
            Cmd::Feature(p) => focus(Some(p), cx),
            Cmd::Tutorial => self.tour = Some(0),
            Cmd::Herdr(a) => self.terminal.update(cx, |t, _| t.herdr_action(a)),
            Cmd::Interrupt => self.terminal.update(cx, |t, _| t.send(b"\x1b")),
            Cmd::ToggleLeft => self.left = !self.left,
            Cmd::Features => {}
            Cmd::NewFeature => return self.open_new_feature(window, cx),
            Cmd::RemoveFeature => return self.remove_feature(window, cx),
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
            Some(i) if i + 1 < tour::FEAT.len() => self.tour = Some(i + 1),
            _ => return self.end_tour(cx),
        }
        cx.notify();
    }

    fn end_tour(&mut self, cx: &mut Context<Self>) {
        self.tour = None;
        tour::mark("feat");
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
            let a = &s.read(cx).agents;
            let n = |st: &[Status]| a.iter().filter(|x| st.contains(&x.agent_status)).count();
            (n(&[Status::Blocked]), n(&[Status::Blocked, Status::Done]))
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
                    .on_click({
                        let sidebar = self.sidebar.clone();
                        move |_, _, cx| {
                            if let Some(s) = &sidebar {
                                let s = s.read(cx);
                                if let Some(p) = s.next_attention() {
                                    s.focus(p);
                                }
                            }
                        }
                    })
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
                            .flex()
                            .gap(px(6.))
                            .opacity(0.45)
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
                                if let Some(p) = s.next_attention() {
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
        bar.child(div().flex_1())
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
        if std::mem::take(&mut self.want_dialog) {
            self.open_new_feature(window, cx);
        }
        let overlay = self.tour.and_then(|i| {
            let root = self.anchors.borrow().get("root").copied()?;
            let next = cx.listener(|this: &mut Self, _: &(), _, cx| this.advance_tour(cx));
            let skip = cx.listener(|this: &mut Self, _: &(), _, cx| this.end_tour(cx));
            let o = tour::overlay(
                tour::FEAT,
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
                if this.tour.is_some() && e.keystroke.key == "escape" {
                    this.end_tour(cx);
                    cx.stop_propagation();
                }
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
                    ),
            )
            .child(self.hints())
            .child(self.status(cx))
            .children(overlay)
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
                    .child(d)
            }))
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
                for (_, m) in &self.toasts {
                    col = col.child(confirm::toast(&t, m));
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
