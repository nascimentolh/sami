use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    herdr::{self, Agent, Herdr, Status, Workspace},
    theme::Theme,
};

/// Lateral "Features" do protótipo: agentes por repositório e, embaixo, os workspaces ("Terminais").
pub struct Sidebar {
    herdr: Arc<Herdr>,
    pub agents: Vec<Agent>,
    pub workspaces: Vec<Workspace>,
    pub offline: bool,
    since: HashMap<String, (Status, Instant)>,
    theme: Theme,
    _poll: Task<()>,
}

pub struct Repo {
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub agents: Vec<Agent>,
}

struct Row {
    id: String,
    glyph: &'static str,
    glyph_color: Hsla,
    b: String,
    k: String,
    m: String,
    m_color: Hsla,
    on: bool,
    unseen: Option<Hsla>,
}

impl Sidebar {
    pub fn new(herdr: Arc<Herdr>, theme: Theme, cx: &mut Context<Self>) -> Self {
        // ponytail: polling de 500 ms (status não tem evento global no protocolo 19, só por pane);
        // trocar por events.subscribe por pane se a latência ou a CPU incomodarem
        let h = herdr.clone();
        let poll = cx.spawn(async move |this, cx| {
            for tick in 0u64.. {
                let h = h.clone();
                let state = cx
                    .background_executor()
                    .spawn(async move { h.agents().and_then(|a| Ok((a, h.workspaces()?))) })
                    .await;
                // "há N min" envelhece sozinho: redesenha a cada 30 s mesmo sem mudança
                let alive = this.update(cx, |this, cx| {
                    if this.apply(state) || tick % 60 == 0 {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
            }
        });
        Self {
            herdr,
            agents: Vec::new(),
            workspaces: Vec::new(),
            offline: false,
            since: HashMap::new(),
            theme,
            _poll: poll,
        }
    }

    fn apply(&mut self, state: std::io::Result<(Vec<Agent>, Vec<Workspace>)>) -> bool {
        let offline = state.is_err();
        let (agents, workspaces) = state.unwrap_or_default();
        let now = Instant::now();
        for a in &agents {
            let entry = self
                .since
                .entry(a.pane_id.clone())
                .or_insert((a.agent_status, now));
            if entry.0 != a.agent_status {
                *entry = (a.agent_status, now);
            }
        }
        let changed =
            self.agents != agents || self.workspaces != workspaces || self.offline != offline;
        self.agents = agents;
        self.workspaces = workspaces;
        self.offline = offline;
        changed
    }

    /// Repositórios em ordem alfabética; agentes fora de git vão com os Terminais.
    pub fn repos(&self) -> (Vec<Repo>, Vec<&Agent>) {
        let mut repos: Vec<Repo> = Vec::new();
        let mut loose = Vec::new();
        for a in &self.agents {
            let Some(root) = herdr::repo_root(Path::new(&a.cwd)) else {
                loose.push(a);
                continue;
            };
            let path = root.to_string_lossy().into_owned();
            match repos.iter_mut().find(|r| r.path == path) {
                Some(r) => r.agents.push(a.clone()),
                None => repos.push(Repo {
                    name: root
                        .file_name()
                        .map_or(path.clone(), |n| n.to_string_lossy().into()),
                    branch: herdr::branch(root),
                    path,
                    agents: vec![a.clone()],
                }),
            }
        }
        repos.sort_by(|a, b| a.name.cmp(&b.name));
        (repos, loose)
    }

    /// Ordem em que aparecem na lateral (Ctrl+Shift+1…9).
    pub fn order(&self) -> Vec<String> {
        let (repos, loose) = self.repos();
        repos
            .iter()
            .flat_map(|r| r.agents.iter().map(|a| a.pane_id.clone()))
            .chain(loose.iter().map(|a| a.pane_id.clone()))
            .collect()
    }

    // uma chamada curta ao socket; thread própria para não segurar a UI nem o contexto
    pub fn focus(&self, pane_id: String) {
        let h = self.herdr.clone();
        std::thread::spawn(move || h.focus(&pane_id));
    }

    fn focus_workspace(&self, id: String) {
        let h = self.herdr.clone();
        std::thread::spawn(move || h.focus_workspace(&id));
    }

    /// Próximo da fila: quem precisa de você primeiro, depois quem terminou.
    pub fn next_attention(&self) -> Option<String> {
        let order = self.order();
        [Status::Blocked, Status::Done].iter().find_map(|s| {
            order
                .iter()
                .find(|id| {
                    self.agents
                        .iter()
                        .any(|a| &a.pane_id == *id && a.agent_status == *s)
                })
                .cloned()
        })
    }

    fn agent_row(&self, a: &Agent, branch: Option<&str>) -> Row {
        let t = &self.theme;
        let (g, label) = glyph(a.agent_status);
        let color = match a.agent_status {
            Status::Blocked => t.orange,
            Status::Working => t.cyan,
            Status::Done => t.ok,
            Status::Idle | Status::Unknown => t.comment,
        };
        let attention = matches!(a.agent_status, Status::Blocked | Status::Done);
        Row {
            id: a.pane_id.clone(),
            glyph: g,
            glyph_color: color,
            b: branch.map_or_else(|| title(a), Into::into),
            k: self
                .since
                .get(&a.pane_id)
                .map(|(_, t)| ago(t.elapsed()))
                .unwrap_or_default(),
            m: match (attention, branch.is_some()) {
                (true, true) => format!("{label} · {}", title(a)),
                (true, false) => label.into(),
                (false, true) => title(a),
                (false, false) => tilde(&a.cwd),
            },
            m_color: if attention { color } else { t.muted },
            on: a.focused,
            unseen: attention.then_some(color),
        }
    }

    fn row(&self, r: Row, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(SharedString::from(r.id))
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px_2()
            .py(px(6.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|d| d.bg(t.raise))
            .when(r.on, |d| d.bg(t.accent_soft))
            .on_click(move |_, _, cx| on_click(cx))
            .when_some(r.unseen, |d, c| {
                d.child(
                    div()
                        .absolute()
                        .left(px(2.))
                        .top(px(8.))
                        .bottom(px(8.))
                        .w(px(2.))
                        .rounded(px(2.))
                        .bg(c),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(16.))
                            .flex_none()
                            .text_center()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .text_color(r.glyph_color)
                            .child(r.glyph),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(t.mono.clone())
                            .text_size(px(12.5))
                            .child(r.b),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(t.muted)
                            .child(r.k),
                    ),
            )
            .child(
                div()
                    .pl(px(24.))
                    .truncate()
                    .text_size(px(12.))
                    .text_color(r.m_color)
                    .child(r.m),
            )
    }
}

pub fn ago(d: Duration) -> String {
    match d.as_secs() / 60 {
        0 => "agora".into(),
        m @ 1..60 => format!("há {m} min"),
        m => format!("há {} h", m / 60),
    }
}

pub fn glyph(s: Status) -> (&'static str, &'static str) {
    match s {
        Status::Blocked => ("!", "precisa de você"),
        Status::Working => ("◐", "trabalhando"),
        Status::Done => ("✓", "pronto pra revisar"),
        Status::Idle | Status::Unknown => ("○", "parado"),
    }
}

fn title(a: &Agent) -> String {
    let t = a.terminal_title_stripped.as_deref().unwrap_or(&a.agent);
    // o claude põe o spinner no começo do título
    let t = t.trim_start_matches(|c: char| !c.is_alphanumeric());
    if t.is_empty() {
        a.agent.clone()
    } else {
        t.into()
    }
}

pub fn tilde(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.into(),
    }
}

fn section(t: &Theme, label: &str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .min_h(px(30.))
        .px(px(10.))
        .text_size(px(11.))
        .text_color(t.muted)
        .border_b_1()
        .border_color(t.line)
        .child(label.to_uppercase())
}

fn feats() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .pt(px(2.))
        .px(px(6.))
        .pb(px(6.))
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let this = cx.entity();
        let (repos, loose) = self.repos();
        let total: usize = repos.iter().map(|r| r.agents.len()).sum();

        let mut col = div().flex().flex_col().child(
            section(&t, "Features")
                .child(div().flex_1())
                .child(div().font_family(t.mono.clone()).child(total.to_string())),
        );
        if self.offline {
            col = col.child(
                div()
                    .p_3()
                    .text_color(t.muted)
                    .child("herdr fora do ar; tentando de novo"),
            );
        } else if repos.is_empty() {
            col = col.child(
                div()
                    .p_3()
                    .text_color(t.muted)
                    .child("Nenhum Claude rodando num repositório."),
            );
        }
        for repo in &repos {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt(px(10.))
                    .pb(px(6.))
                    .px(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(repo.name.clone())
                    .child(
                        div()
                            .truncate()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(t.muted)
                            .child(tilde(&repo.path)),
                    ),
            );
            let mut list = feats();
            for a in &repo.agents {
                let pane = a.pane_id.clone();
                let this = this.clone();
                let row = self.agent_row(a, repo.branch.as_deref());
                list = list.child(self.row(row, move |cx| this.read(cx).focus(pane.clone())));
            }
            col = col.child(list);
        }

        col = col.child(
            div()
                .pt(px(8.))
                .pb(px(2.))
                .px(px(14.))
                .text_size(px(11.))
                .text_color(t.muted)
                .child("TERMINAIS"),
        );
        let mut terms = feats();
        for w in &self.workspaces {
            let id = w.workspace_id.clone();
            let this = this.clone();
            let row = Row {
                id: format!("ws:{}", w.workspace_id),
                glyph: "$",
                glyph_color: t.comment,
                b: w.label.clone(),
                k: format!(
                    "{} aba{}",
                    w.tab_count,
                    if w.tab_count == 1 { "" } else { "s" }
                ),
                m: "só terminal".into(),
                m_color: t.muted,
                on: false,
                unseen: None,
            };
            terms = terms.child(self.row(row, move |cx| this.read(cx).focus_workspace(id.clone())));
        }
        for a in loose {
            let pane = a.pane_id.clone();
            let this = this.clone();
            let row = self.agent_row(a, None);
            terms = terms.child(self.row(row, move |cx| this.read(cx).focus(pane.clone())));
        }
        col = col.child(terms);

        div()
            .id("sidebar")
            .w(px(250.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.line)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(13.))
            .child(col)
    }
}

#[cfg(test)]
mod tests {
    use super::{ago, title};
    use crate::herdr::{Agent, Status};
    use std::time::Duration;

    #[test]
    fn titulo_e_tempo() {
        let a = Agent {
            pane_id: "p".into(),
            agent: "claude".into(),
            agent_status: Status::Idle,
            terminal_title_stripped: Some("◑ revisão".into()),
            cwd: "/".into(),
            focused: false,
        };
        assert_eq!(title(&a), "revisão");
        assert_eq!(ago(Duration::from_secs(30)), "agora");
        assert_eq!(ago(Duration::from_secs(180)), "há 3 min");
        assert_eq!(ago(Duration::from_secs(7300)), "há 2 h");
    }
}
