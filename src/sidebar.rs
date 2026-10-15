use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    changes::{self, Changes},
    feature::{self, Inventory},
    herdr::{self, Agent, Herdr, Status, Tab, Workspace},
    mascot::{Kind, Note},
    mr::{self, Fetch, Mr},
    perms,
    session::{Sessions, Usage},
    theme::Theme,
    tour::{self, Anchors},
};

pub enum SidebarEvent {
    NewFeature,
    /// clique num arquivo de "Mudanças": abre a revisão nele
    OpenReview(String),
    /// clique em "Arquivos": abre o arquivo no editor
    OpenFile(String),
    /// o MR da feature desta worktree acabou de mergear: (worktree, iid, branch)
    Merged(PathBuf, u64, String),
    /// um agente passou a pedir permissão ou terminou (o `notify(f)` do protótipo)
    Notify(Note),
    /// a mesma permissão foi pedida vezes demais: sugerir a regra
    Learn(perms::Suggestion),
    /// o Maestro escreveu no vault (Histórico, daily)
    Vault(Result<crate::vault::Write, String>),
}

impl EventEmitter<SidebarEvent> for Sidebar {}

/// Lateral "Features" do protótipo: agentes por repositório e, embaixo, os workspaces ("Terminais").
pub struct Sidebar {
    herdr: Arc<Herdr>,
    pub agents: Vec<Agent>,
    pub workspaces: Vec<Workspace>,
    pub tabs: Vec<Tab>,
    pub offline: bool,
    since: HashMap<String, (Status, Instant)>,
    pub usage: HashMap<String, Usage>,
    pub inventories: Vec<Inventory>,
    changes: Option<Changes>,
    /// aba Arquivos à frente e a lista dela (lida no clique da aba)
    files: Option<Vec<String>>,
    /// MR de cada feature (pela worktree), o `mrSum` do card
    mrs: HashMap<PathBuf, Mr>,
    learned: HashSet<(PathBuf, String)>,
    /// comentários já mandados ao Claude, por worktree (`<id>.sent`)
    sent: HashMap<PathBuf, HashSet<u64>>,
    /// o pedido de permissão de cada pane bloqueado ("pediu permissão: npm test")
    asks: HashMap<String, String>,
    /// (arquivo do editor, sujo, aba do editor à frente)
    open: Option<(String, bool, bool)>,
    sessions: Arc<Mutex<Sessions>>,
    anchors: Anchors,
    theme: Theme,
    _poll: Task<()>,
}

/// Aonde um item da fila leva.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    Pane(String),
    /// (pane, worktree): a feature e a aba MR dela
    Mr(String, PathBuf),
    /// (pane, worktree): a feature e o painel Revisar
    Review(String, PathBuf),
    Compare(String),
}

/// Um item do `attention()` do protótipo; `p` é a prioridade (0 = permissão … 4 = daily).
#[derive(Clone, Debug, PartialEq)]
pub struct AttItem {
    pub p: u8,
    pub tag: &'static str,
    pub text: String,
    pub go: Go,
}

pub struct Repo {
    pub name: String,
    pub path: String,
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
    m2: Option<(String, String, bool)>,
    mr: Option<Mr>,
    on: bool,
    unseen: Option<Hsla>,
    dim: bool,
}

impl Sidebar {
    pub fn new(herdr: Arc<Herdr>, theme: Theme, anchors: Anchors, cx: &mut Context<Self>) -> Self {
        // ponytail: polling de 500 ms (status não tem evento global no protocolo 19, só por pane);
        // trocar por events.subscribe por pane se a latência ou a CPU incomodarem
        let h = herdr.clone();
        let poll = cx.spawn(async move |this, cx| {
            for tick in 0u64.. {
                let h = h.clone();
                let state = cx
                    .background_executor()
                    .spawn(async move { Ok((h.agents()?, h.workspaces()?, h.tabs()?)) })
                    .await;
                // "há N min" envelhece sozinho: redesenha a cada 30 s mesmo sem mudança
                let alive = this.update(cx, |this, cx| {
                    let (changed, notes) = this.apply(state);
                    for n in notes {
                        cx.emit(SidebarEvent::Notify(n));
                    }
                    if changed || tick % 60 == 0 {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                // custo/contexto e Mudanças são mais caros (JSONL e git): a cada 3 s
                if tick % 6 == 0 {
                    let Ok((jobs, root, sessions)) = this.read_with(cx, |this, _| {
                        let jobs: Vec<_> = this
                            .agents
                            .iter()
                            .map(|a| (a.pane_id.clone(), a.cwd.clone(), title(a)))
                            .collect();
                        (jobs, this.top_worktree(), this.sessions.clone())
                    }) else {
                        break;
                    };
                    let (usage, changes, inventories, learn) = cx
                        .background_executor()
                        .spawn(async move {
                            let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                            let mut s = sessions.lock().unwrap();
                            let usage: HashMap<_, _> = jobs
                                .into_iter()
                                .filter_map(|(pane, cwd, title)| {
                                    Some((pane.clone(), s.usage(&home, &pane, &cwd, &title)?))
                                })
                                .collect();
                            (
                                usage,
                                root.and_then(|r| changes::changes(&r)),
                                feature::inventories(),
                                perms::pending(),
                            )
                        })
                        .await;
                    let Ok(invs) = this.read_with(cx, |this, _| this.inventories.clone()) else {
                        break;
                    };
                    let sent: HashMap<PathBuf, HashSet<u64>> = cx
                        .background_executor()
                        .spawn(async move {
                            invs.iter()
                                .map(|i| {
                                    let ids = feature::state_lines(&i.id, "sent")
                                        .iter()
                                        .filter_map(|l| l.parse().ok())
                                        .collect();
                                    (i.worktrees[0].path.clone(), ids)
                                })
                                .collect()
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        for s in learn {
                            if this.learned.insert((s.repo.clone(), s.rule.clone())) {
                                cx.emit(SidebarEvent::Learn(s));
                            }
                        }
                        if this.sent != sent {
                            this.sent = sent;
                            cx.notify();
                        }
                        if this.usage != usage
                            || this.changes != changes
                            || this.inventories != inventories
                        {
                            this.usage = usage;
                            this.changes = changes;
                            this.inventories = inventories;
                            cx.notify();
                        }
                    });
                }
                // ponytail: MRs a cada 60 s (4 chamadas do glab por feature); webhook não chega aqui
                if tick % 120 == 4 {
                    let Ok(jobs) = this.read_with(cx, |this, _| {
                        this.inventories
                            .iter()
                            .map(|i| (i.worktrees[0].path.clone(), i.branch.clone()))
                            .collect::<Vec<_>>()
                    }) else {
                        break;
                    };
                    let mrs: HashMap<PathBuf, Mr> = cx
                        .background_executor()
                        .spawn(async move {
                            jobs.into_iter()
                                .filter_map(|(wt, b)| match mr::fetch(&wt, &b) {
                                    Fetch::Mr(m) => Some((wt, *m)),
                                    _ => None,
                                })
                                .collect()
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        for (wt, m) in &mrs {
                            let Some(inv) =
                                this.inventories.iter().find(|i| i.worktrees[0].path == *wt)
                            else {
                                continue;
                            };
                            // MR aberto ou mergeado → Histórico (e [x] na daily), uma vez cada
                            let repo = inv
                                .repo
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let hist = feature::state_lines(&inv.id, "hist");
                            let step = if m.merged {
                                format!("merged:{}", m.iid)
                            } else {
                                format!("open:{}", m.iid)
                            };
                            if !m.closed && !hist.contains(&step) {
                                feature::add_state_line(&inv.id, "hist", &step);
                                for r in crate::vault::on_mr(
                                    &repo,
                                    m.iid,
                                    &inv.branch,
                                    &m.title,
                                    m.merged,
                                ) {
                                    cx.emit(SidebarEvent::Vault(r));
                                }
                            }
                            // uma vez por MR, mesmo depois de reiniciar
                            let iid = m.iid.to_string();
                            if m.merged && !feature::state_lines(&inv.id, "merged").contains(&iid) {
                                feature::add_state_line(&inv.id, "merged", &iid);
                                cx.emit(SidebarEvent::Merged(
                                    wt.clone(),
                                    m.iid,
                                    inv.branch.clone(),
                                ));
                            }
                        }
                        if this.mrs != mrs {
                            this.mrs = mrs;
                            cx.notify();
                        }
                    });
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
            tabs: Vec::new(),
            offline: false,
            since: HashMap::new(),
            usage: HashMap::new(),
            inventories: Vec::new(),
            changes: None,
            files: None,
            mrs: HashMap::new(),
            learned: HashSet::new(),
            sent: HashMap::new(),
            asks: HashMap::new(),
            open: None,
            sessions: Arc::default(),
            anchors,
            theme,
            _poll: poll,
        }
    }

    /// Aplica o estado do herdr; devolve se mudou e os agentes que acabaram de pedir atenção.
    fn apply(
        &mut self,
        state: std::io::Result<(Vec<Agent>, Vec<Workspace>, Vec<Tab>)>,
    ) -> (bool, Vec<Note>) {
        let offline = state.is_err();
        let (agents, workspaces, tabs) = state.unwrap_or_default();
        let now = Instant::now();
        let mut notes = Vec::new();
        for a in &agents {
            let entry = self
                .since
                .entry(a.pane_id.clone())
                .or_insert((a.agent_status, now));
            if entry.0 != a.agent_status {
                *entry = (a.agent_status, now);
                let kind = match a.agent_status {
                    Status::Blocked => Some(Kind::Ask),
                    Status::Done => Some(Kind::Done),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let wt = herdr::repo_root(Path::new(&a.cwd)).map(Path::to_path_buf);
                    let inv = self
                        .inventories
                        .iter()
                        .find(|i| i.worktrees.iter().any(|w| Some(&w.path) == wt.as_ref()));
                    let branch = inv
                        .map(|i| i.branch.clone())
                        .or_else(|| branch_of(a))
                        .unwrap_or_else(|| title(a));
                    let text = if kind == Kind::Ask {
                        let ask = inv
                            .and_then(|i| perms::last_ask(&i.id))
                            .unwrap_or_else(|| title(a));
                        self.asks.insert(a.pane_id.clone(), ask.clone());
                        ask
                    } else {
                        "terminou, pronto pra revisar".into()
                    };
                    notes.push(Note {
                        pane: a.pane_id.clone(),
                        kind,
                        branch,
                        text,
                    });
                }
            }
        }
        let changed = self.agents != agents
            || self.workspaces != workspaces
            || self.tabs != tabs
            || self.offline != offline;
        self.agents = agents;
        self.workspaces = workspaces;
        self.tabs = tabs;
        self.offline = offline;
        (changed, notes)
    }

    /// Repositórios em ordem alfabética; agentes fora de git vão com os Terminais.
    pub fn repos(&self) -> (Vec<Repo>, Vec<&Agent>) {
        let mut repos: Vec<Repo> = Vec::new();
        let mut loose = Vec::new();
        for a in &self.agents {
            let Some(root) = herdr::repo_root(Path::new(&a.cwd)).map(herdr::main_repo) else {
                loose.push(a);
                continue;
            };
            let root = root.as_path();
            let path = root.to_string_lossy().into_owned();
            match repos.iter_mut().find(|r| r.path == path) {
                Some(r) => r.agents.push(a.clone()),
                None => repos.push(Repo {
                    name: root
                        .file_name()
                        .map_or(path.clone(), |n| n.to_string_lossy().into()),
                    path,
                    agents: vec![a.clone()],
                }),
            }
        }
        repos.sort_by(|a, b| a.name.cmp(&b.name));
        (repos, loose)
    }

    /// Worktree do agente em foco (ou do primeiro em git): a de "Mudanças".
    pub fn top_worktree(&self) -> Option<PathBuf> {
        let focused = self.agents.iter().find(|a| a.focused);
        focused
            .into_iter()
            .chain(&self.agents)
            .find_map(|a| herdr::repo_root(Path::new(&a.cwd)))
            .map(Path::to_path_buf)
    }

    pub fn set_open(&mut self, open: Option<(String, bool, bool)>, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            cx.notify();
        }
    }

    pub fn feature_refs(&self) -> Vec<crate::vault_view::FeatureRef> {
        crate::vault_view::features_from(&self.inventories, &self.agents, &self.mrs)
    }

    /// Repositório principal dessa worktree: o do cabeçalho e o da "Nova feature".
    pub fn top_repo(&self) -> Option<PathBuf> {
        self.top_worktree().map(|w| herdr::main_repo(&w))
    }

    pub fn today_cost(&self) -> f64 {
        self.usage.values().map(|u| u.today).sum::<f64>() + 0.0
    }

    pub fn first_blocked(&self) -> Option<String> {
        let order = self.order();
        order.into_iter().find(|id| {
            self.agents
                .iter()
                .any(|a| &a.pane_id == id && a.agent_status == Status::Blocked)
        })
    }

    /// (pane, nome na lateral) na ordem da tela, para a paleta.
    pub fn features(&self) -> Vec<(String, String)> {
        let (repos, loose) = self.repos();
        let label = |pane: &str| -> String {
            repos
                .iter()
                .flat_map(|r| &r.agents)
                .find(|a| a.pane_id == pane)
                .and_then(branch_of)
                .or_else(|| loose.iter().find(|a| a.pane_id == pane).map(|a| title(a)))
                .unwrap_or_default()
        };
        self.order()
            .into_iter()
            .map(|p| {
                let l = label(&p);
                (p, l)
            })
            .collect()
    }

    /// Ordem em que aparecem na lateral (Ctrl+Shift+1…9).
    pub fn order(&self) -> Vec<String> {
        let (repos, loose) = self.repos();
        let top = self.top_repo().map(|p| p.to_string_lossy().into_owned());
        repos
            .iter()
            .filter(|r| Some(&r.path) == top.as_ref())
            .chain(repos.iter().filter(|r| Some(&r.path) != top.as_ref()))
            .flat_map(|r| r.agents.iter().map(|a| a.pane_id.clone()))
            .chain(loose.iter().map(|a| a.pane_id.clone()))
            .collect()
    }

    // uma chamada curta ao socket; thread própria para não segurar a UI nem o contexto
    /// Repositórios principais com algum agente aberto (onde a Limpeza procura).
    pub fn repo_paths(&self) -> Vec<PathBuf> {
        self.repos()
            .0
            .into_iter()
            .map(|r| PathBuf::from(r.path))
            .collect()
    }

    pub fn herdr(&self) -> Arc<Herdr> {
        self.herdr.clone()
    }

    pub fn focus(&self, pane_id: String) {
        let h = self.herdr.clone();
        std::thread::spawn(move || h.focus(&pane_id));
    }

    pub fn focus_tab(&self, id: String) {
        let h = self.herdr.clone();
        std::thread::spawn(move || h.focus_tab(&id));
    }

    pub fn new_tab(&self, workspace_id: String) {
        let h = self.herdr.clone();
        std::thread::spawn(move || h.new_tab(&workspace_id));
    }

    fn focus_workspace(&self, id: String) {
        let h = self.herdr.clone();
        std::thread::spawn(move || h.focus_workspace(&id));
    }

    /// Próximo da fila: quem precisa de você primeiro, depois quem terminou.
    /// A fila: permissão > CI/Sonar > comentários, prontas e comparar > contexto alto.
    // ponytail: MR alheio, auditoria e daily entram com as Etapas 8, 9 e 7
    pub fn attention(&self) -> Vec<AttItem> {
        let mut q = Vec::new();
        let mut groups_done: HashSet<String> = HashSet::new();
        for pane in self.order() {
            let Some(a) = self.agents.iter().find(|a| a.pane_id == pane) else {
                continue;
            };
            let Some(wt) = herdr::repo_root(Path::new(&a.cwd)).map(Path::to_path_buf) else {
                continue;
            };
            let inv = self
                .inventories
                .iter()
                .find(|i| i.worktrees.iter().any(|w| w.path == wt));
            // o git só roda quando o item existe (a fila é recalculada a cada desenho do título)
            let branch = || {
                inv.map_or_else(
                    || branch_of(a).unwrap_or_else(|| title(a)),
                    |i| i.branch.clone(),
                )
            };
            let item = |p, tag, text: String, go| AttItem { p, tag, text, go };
            if a.agent_status == Status::Blocked {
                let msg = self.asks.get(&pane).cloned().unwrap_or_else(|| title(a));
                q.push(item(
                    0,
                    "permissão",
                    format!("{}: {msg}", branch()),
                    Go::Pane(pane.clone()),
                ));
            }
            if let Some(m) = self.mrs.get(&wt).filter(|m| !m.merged) {
                let go = Go::Mr(pane.clone(), wt.clone());
                if m.pipe == mr::Status::Fail {
                    q.push(item(
                        1,
                        "CI",
                        format!("CI falhou em !{} ({})", m.iid, branch()),
                        go.clone(),
                    ));
                }
                if m.sonar == mr::Status::Fail {
                    q.push(item(
                        1,
                        "Sonar",
                        format!("Quality gate reprovado em !{}", m.iid),
                        go.clone(),
                    ));
                }
                let sent = self.sent.get(&wt);
                let n = m
                    .comments
                    .iter()
                    .filter(|c| sent.is_none_or(|s| !s.contains(&c.id)))
                    .count();
                if n > 0 {
                    let s = if n > 1 { "s" } else { "" };
                    q.push(item(
                        2,
                        "MR",
                        format!("{n} comentário{s} novo{s} em !{}", m.iid),
                        go,
                    ));
                }
            }
            if a.agent_status == Status::Done {
                q.push(item(
                    2,
                    "revisar",
                    format!("{} pronta pra revisar", branch()),
                    Go::Review(pane.clone(), wt.clone()),
                ));
            }
            if let Some(g) = inv.and_then(|i| i.group.clone()) {
                let attempts: Vec<&Inventory> = self
                    .inventories
                    .iter()
                    .filter(|i| i.group.as_ref() == Some(&g))
                    .collect();
                let done = attempts.len() > 1
                    && attempts.iter().all(|i| {
                        self.agents.iter().any(|x| {
                            x.agent_status == Status::Done
                                && i.worktrees
                                    .iter()
                                    .any(|w| Path::new(&x.cwd).starts_with(&w.path))
                        })
                    });
                if done && groups_done.insert(g.clone()) {
                    q.push(item(
                        2,
                        "comparar",
                        format!("Tentativas de {g} prontas para comparar"),
                        Go::Compare(pane.clone()),
                    ));
                }
            }
            if let Some(u) = self.usage.get(&pane).filter(|u| u.ctx >= 80) {
                q.push(item(
                    3,
                    "contexto",
                    format!(
                        "{} com {}% do contexto: perto de compactar",
                        branch(),
                        u.ctx
                    ),
                    Go::Pane(pane.clone()),
                ));
            }
        }
        q.sort_by_key(|i| i.p);
        q
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
        let wt = herdr::repo_root(Path::new(&a.cwd)).map(Path::to_path_buf);
        let port = self
            .inventories
            .iter()
            .find(|i| i.worktrees.iter().any(|w| Some(&w.path) == wt.as_ref()))
            .map_or(String::new(), |i| format!(":{} · ", i.port));
        let m2 = self.usage.get(&a.pane_id).map(|u| {
            (
                format!("{port}US$ {}", brl(u.cost)),
                format!("ctx {}%{}", u.ctx, if u.ctx >= 80 { " ⚠" } else { "" }),
                u.ctx >= 80,
            )
        });
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
            m2,
            mr: wt
                .as_ref()
                .and_then(|w| self.mrs.get(w))
                .cloned()
                .map(|mut m| {
                    let sent = wt.as_ref().and_then(|w| self.sent.get(w));
                    m.comments
                        .retain(|c| sent.is_none_or(|s| !s.contains(&c.id)));
                    m
                }),
            on: a.focused,
            unseen: attention.then_some(color),
            dim: false,
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
            .when(r.dim, |d| d.opacity(0.45))
            .when(!r.dim, |d| d.cursor_pointer().hover(|d| d.bg(t.raise)))
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
            .when_some(r.m2, |d, (cost, ctx, warn)| {
                d.child(
                    div()
                        .pl(px(24.))
                        .flex()
                        .font_family(t.mono.clone())
                        .text_size(px(11.))
                        .text_color(t.comment)
                        .child(format!("{cost} · "))
                        .child(div().when(warn, |d| d.text_color(t.orange)).child(ctx))
                        .when_some(r.mr, |d, m| d.children(mr_sum(t, &m))),
                )
            })
    }

    fn changes_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let n = self.changes.as_ref().map_or(0, |c| c.files.len());
        let on_files = self.files.is_some();
        let tab = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .cursor_pointer()
                .px(px(8.))
                .py(px(4.))
                .rounded_t(px(5.))
                .text_size(px(12.))
                .text_color(if on { t.fg } else { t.muted })
                .when(on, |d| d.border_b_2().border_color(t.accent))
                .child(label)
        };
        let mut list = div().flex().flex_col().pt(px(4.)).pb(px(10.));
        let row = |id: String, dir: &str, name: &str| {
            let dir = if dir.is_empty() {
                String::new()
            } else {
                format!("{dir}/")
            };
            div()
                .id(SharedString::from(id))
                .cursor_pointer()
                .hover(|d| d.bg(t.raise))
                .flex()
                .gap(px(6.))
                .items_center()
                .px(px(12.))
                .py(px(3.))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .child(div().text_color(t.muted).child(dir))
                        .child(name.to_string()),
                )
        };
        let split = |p: &str| -> (String, String) {
            p.rsplit_once('/')
                .map_or((String::new(), p.to_string()), |(d, n)| {
                    (d.into(), n.into())
                })
        };
        match (&self.files, &self.changes) {
            (Some(files), _) => {
                // ponytail: os 300 primeiros; árvore ou busca (Ctrl+Shift+O) para repo grande
                for p in files.iter().take(300) {
                    let (dir, name) = split(p);
                    let open = self.open.as_ref().filter(|o| o.0 == *p);
                    let path = p.clone();
                    list = list.child(
                        row(format!("fl:{p}"), &dir, &name)
                            .when(open.is_some_and(|o| o.2), |d| d.bg(t.accent_soft))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SidebarEvent::OpenFile(path.clone()))
                            }))
                            .when(open.is_some_and(|o| o.1), |d| {
                                d.child(div().text_size(px(11.)).text_color(t.muted).child("●"))
                            }),
                    );
                }
            }
            (None, None) => {}
            (None, Some(c)) if c.files.is_empty() => {
                list = list.child(none(t, "Nada mudou nesta branch.".into()));
            }
            (None, Some(c)) => {
                for f in &c.files {
                    let (dir, name) = split(&f.path);
                    let path = f.path.clone();
                    list = list.child(
                        row(format!("ch:{}", f.path), &dir, &name)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SidebarEvent::OpenReview(path.clone()))
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(t.mono.clone())
                                    .text_size(px(11.))
                                    .text_color(t.muted)
                                    .child(format!(
                                        "{}+{} −{}",
                                        if f.new { "novo " } else { "" },
                                        f.added,
                                        f.removed
                                    )),
                            ),
                    );
                }
                let pending = if c.uncommitted > 0 {
                    format!(
                        "{} arquivo{} não commitado{} · ",
                        c.uncommitted,
                        if c.uncommitted > 1 { "s" } else { "" },
                        if c.uncommitted > 1 { "s" } else { "" }
                    )
                } else {
                    String::new()
                };
                list = list.child(
                    div()
                        .flex()
                        .px(px(12.))
                        .py(px(6.))
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child(format!("{pending}base "))
                        .child(div().font_family(t.mono.clone()).child(c.base.clone())),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .pt(px(6.))
                    .px(px(8.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(
                        tab("lt-ch", format!("Mudanças {n}"), !on_files).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.files = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        tab("lt-files", "Arquivos".into(), on_files).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.files = Some(
                                    this.top_worktree()
                                        .map(|r| changes::files(&r))
                                        .unwrap_or_default(),
                                );
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(list)
    }
}

/// O `mrSum` do protótipo: ` · !148 CI✗ Sonar✓ 2 coment.`
fn mr_sum(t: &Theme, m: &Mr) -> Vec<Div> {
    let red: Hsla = rgb(0xff5555).into();
    let mark = |label: &str, s: mr::Status| {
        let (g, c) = match s {
            mr::Status::Ok => ("✓", t.ok),
            mr::Status::Fail => ("✗", red),
            mr::Status::Running => ("⟳", rgb(0xf1fa8c).into()),
            mr::Status::None => ("–", t.comment),
        };
        div().ml(px(4.)).text_color(c).child(format!("{label}{g}"))
    };
    let mut v = vec![div().child(format!(" · !{}", m.iid))];
    if m.merged {
        v.push(div().ml(px(4.)).text_color(t.ok).child("mergeado"));
        return v;
    }
    v.push(mark("CI", m.pipe));
    v.push(mark("Sonar", m.sonar));
    if !m.comments.is_empty() {
        v.push(
            div()
                .ml(px(4.))
                .text_color(t.orange)
                .child(format!("{} coment.", m.comments.len())),
        );
    }
    v
}

fn none(t: &Theme, text: String) -> Div {
    div()
        .px(px(12.))
        .py(px(6.))
        .text_size(px(12.))
        .text_color(t.muted)
        .child(text)
}

/// `0.42` → `0,42`, como o protótipo escreve dinheiro
pub fn brl(v: f64) -> String {
    format!("{v:.2}").replace('.', ",")
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

fn branch_of(a: &Agent) -> Option<String> {
    herdr::repo_root(Path::new(&a.cwd)).and_then(herdr::branch)
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
        let top = self.top_repo().map(|p| p.to_string_lossy().into_owned());
        let repo_head = |name: String, path: &str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .pt(px(10.))
                .pb(px(6.))
                .px(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(name)
                .child(
                    div()
                        .truncate()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(t.muted)
                        .child(tilde(path)),
                )
        };

        let mut col = div().flex().flex_col();
        // ordem do protótipo: repo · Auditoria · FEATURES · cards · Nova feature · TERMINAIS · Mudanças
        if let Some(top) = repos.iter().find(|r| Some(&r.path) == top.as_ref()) {
            col = col.child(repo_head(top.name.clone(), &top.path));
            // ponytail: a Auditoria é a Etapa 9; até lá o card aparece apagado
            col = col.child(feats().child(self.row(
                Row {
                    id: "auditoria".into(),
                    glyph: "◇",
                    glyph_color: t.comment,
                    b: "Auditoria".into(),
                    k: String::new(),
                    m: "nunca rodou".into(),
                    m_color: t.muted,
                    m2: None,
                    mr: None,
                    on: false,
                    unseen: None,
                    dim: true,
                },
                |_| {},
            )));
        }
        col = col.child(
            section(&t, "Features")
                .child(div().flex_1())
                .child(div().font_family(t.mono.clone()).child(total.to_string())),
        );
        if self.offline {
            col = col.child(none(&t, "herdr fora do ar; tentando de novo".into()));
        } else if repos.is_empty() {
            col = col.child(none(&t, "Nenhum Claude rodando num repositório.".into()));
        }
        let mut list = feats();
        // o repo do cabeçalho primeiro; os outros depois, cada um com o seu cabeçalho
        let ordered = repos
            .iter()
            .filter(|r| Some(&r.path) == top.as_ref())
            .chain(repos.iter().filter(|r| Some(&r.path) != top.as_ref()));
        for repo in ordered {
            if Some(&repo.path) != top.as_ref() {
                list = list.child(repo_head(repo.name.clone(), &repo.path));
            }
            for a in &repo.agents {
                let pane = a.pane_id.clone();
                let this = this.clone();
                let row = self.agent_row(a, branch_of(a).as_deref());
                list = list.child(self.row(row, move |cx| this.read(cx).focus(pane.clone())));
            }
        }
        col = col.child(
            list.relative()
                .child(tour::anchor(&self.anchors, "features")),
        );
        col = col.child(
            div()
                .id("newf")
                .cursor_pointer()
                .hover(|d| d.text_color(t.fg).border_color(t.accent))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::NewFeature)))
                .relative()
                .child(tour::anchor(&self.anchors, "newf"))
                .mx(px(6.))
                .mt(px(2.))
                .mb(px(8.))
                .px(px(8.))
                .py(px(6.))
                .border_1()
                .border_dashed()
                .border_color(t.line)
                .rounded(px(6.))
                .flex()
                .justify_between()
                .items_center()
                .text_color(t.muted)
                .child("+ Nova feature")
                .child(
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(11.))
                        .px(px(4.))
                        .border_1()
                        .border_color(t.line)
                        .rounded(px(3.))
                        .bg(t.panel)
                        .child("Alt+N"),
                ),
        );
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
                m2: None,
                mr: None,
                on: false,
                unseen: None,
                dim: false,
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
        if top.is_some() {
            col = col.child(self.changes_panel(cx));
        }

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
            tab_id: "t".into(),
            workspace_id: "w".into(),
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
