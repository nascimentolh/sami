use std::{collections::HashMap, time::Duration};

use futures::StreamExt;
use gpui_kit::{prelude::FluentBuilder, *};
use serde_json::Value;

use crate::{
    diff::{self, FileDiff, Kind},
    editor,
    form::{FieldEvent, TextField, button, checkbox},
    mr::Status,
    mrs::{self, Draft, Finding, Opened, Row, Run},
    theme::Theme,
    tour::{self, Anchors},
};

pub enum Event {
    Toast(String),
    /// confirmação antes de publicar: (título, texto)
    AskPublish(String, String),
    /// descarte com "Desfazer" no toast
    Discarded(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Review,
    Mine,
    Group,
}

enum Phase {
    Setup,
    Run(Vec<(&'static str, String)>),
    Done(Vec<(Finding, bool)>, f64),
}

/// A gaveta "Revisão Claude" do protótipo.
struct Drawer {
    row: Row,
    phase: Phase,
    incremental: bool,
    pid: Option<u32>,
    /// 6. pipeline vermelho ou rodando: avisa antes de revisar
    ci_warn: Option<&'static str>,
}

/// O modo MRs do protótipo: inbox (`renderInbox`), o MR com threads e rascunhos (`renderMR`) e o revisor.
pub struct MrsView {
    me: Option<String>,
    lists: [Option<Result<Vec<Row>, String>>; 3],
    ci: HashMap<String, Status>,
    tab: Tab,
    sel: usize,
    filter: Entity<TextField>,
    seen: String,
    open: Option<(Row, Option<Result<Opened, String>>)>,
    drafts: HashMap<String, Vec<Draft>>,
    next_id: usize,
    selected: Option<(String, u32)>,
    editing: Option<(usize, Entity<TextField>)>,
    drawer: Option<Drawer>,
    /// o `S.undo` do protótipo: (MR, [(posição, rascunho)]) por descarte
    undo: Vec<(String, Vec<(usize, Draft)>)>,
    posting: bool,
    scroll: ScrollHandle,
    focus: FocusHandle,
    theme: Theme,
    anchors: Anchors,
    _poll: Task<()>,
}

impl EventEmitter<Event> for MrsView {}

fn red() -> Hsla {
    rgb(0xff5555).into()
}
fn yellow() -> Hsla {
    rgb(0xf1fa8c).into()
}

impl MrsView {
    pub fn new(theme: Theme, anchors: Anchors, cx: &mut Context<Self>) -> Self {
        let t = theme.clone();
        let filter = cx.new(|cx| TextField::new(t, "", "filtrar  ( / )", false, false, cx));
        cx.subscribe(&filter, |this, _, e: &FieldEvent, cx| {
            if matches!(e, FieldEvent::Changed) {
                this.sel = 0;
            }
            cx.notify();
        })
        .detach();
        // ponytail: a inbox relê a cada 60 s, como o desenho pede (R força)
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
            }
        });
        Self {
            me: None,
            lists: [None, None, None],
            ci: HashMap::new(),
            tab: Tab::Review,
            sel: 0,
            filter,
            seen: std::fs::read_to_string(mrs::seen_file()).unwrap_or_default(),
            open: None,
            drafts: HashMap::new(),
            next_id: 0,
            selected: None,
            editing: None,
            drawer: None,
            undo: Vec::new(),
            posting: false,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            theme,
            anchors,
            _poll: poll,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Quantos MRs esperam a sua revisão (o número de "MRs" no título).
    pub fn waiting(&self) -> usize {
        match &self.lists[0] {
            Some(Ok(v)) => v.len(),
            _ => 0,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let me = self.me.clone();
        cx.spawn(async move |this, cx| {
            let (me, lists) = cx
                .background_executor()
                .spawn(async move {
                    let Some(me) = me.or_else(mrs::me) else {
                        let e = Err("glab sem login: rode glab auth login".to_string());
                        return (None, [e.clone(), e.clone(), e]);
                    };
                    let l = mrs::inbox(&me, &mrs::group());
                    (Some(me), l)
                })
                .await;
            let rows: Vec<Row> = lists.iter().flatten().flatten().cloned().collect();
            let _ = this.update(cx, |this, cx| {
                this.me = me;
                this.seen = std::fs::read_to_string(mrs::seen_file()).unwrap_or_default();
                this.lists = lists.map(Some);
                cx.notify();
            });
            // o selo de CI vem do detalhe de cada MR (a lista não traz o pipeline)
            for r in rows {
                let (path, iid, key) = (r.path.clone(), r.iid, r.key());
                let ci = cx
                    .background_executor()
                    .spawn(async move {
                        mrs::read(
                            &format!("projects/{}/merge_requests/{iid}", mrs::enc(&path)),
                            false,
                        )
                        .map(|d| mrs::ci_of(&d))
                    })
                    .await;
                if let Ok(ci) = ci
                    && this
                        .update(cx, |this, cx| {
                            this.ci.insert(key, ci);
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let i = match self.tab {
            Tab::Review => 0,
            Tab::Mine => 1,
            Tab::Group => 2,
        };
        let q = self.filter.read(cx).value.to_lowercase();
        match &self.lists[i] {
            Some(Ok(v)) => v
                .iter()
                .filter(|m| {
                    q.is_empty()
                        || format!("{} {} {} !{}", m.proj(), m.title, m.who, m.iid)
                            .to_lowercase()
                            .contains(&q)
                })
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `novo`, `revisado` ou `+N commits` (o sha mudou desde a sua última revisão).
    fn pill(&self, r: &Row) -> (&'static str, String) {
        if mrs::seen(&self.seen, &r.path, r.iid, &r.sha) {
            ("", "revisado".into())
        } else if mrs::last_seen(&self.seen, &r.path, r.iid).is_some() {
            ("inc", "commits novos".into())
        } else {
            ("new", "novo".into())
        }
    }

    pub fn open_row(&mut self, r: Row, cx: &mut Context<Self>) {
        let (path, iid) = (r.path.clone(), r.iid);
        self.open = Some((r, None));
        self.selected = None;
        cx.spawn(async move |this, cx| {
            let o = cx
                .background_executor()
                .spawn(async move { mrs::open(&path, iid) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some((row, slot)) = this.open.as_mut()
                    && row.iid == iid
                {
                    *slot = Some(o);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.editing = None;
        cx.notify();
    }

    fn head(&self) -> String {
        match &self.open {
            Some((_, Some(Ok(o)))) => o.detail["diff_refs"]["head_sha"]
                .as_str()
                .unwrap_or_default()
                .into(),
            _ => String::new(),
        }
    }

    fn mr_drafts(&mut self) -> Option<&mut Vec<Draft>> {
        let k = self.open.as_ref()?.0.key();
        Some(self.drafts.entry(k).or_default())
    }

    fn new_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((file, line)) = self.selected.clone() else {
            return cx.emit(Event::Toast("Clique numa linha do diff primeiro".into()));
        };
        self.next_id += 1;
        let id = self.next_id;
        let head = self.head();
        if let Some(d) = self.mr_drafts() {
            d.push(Draft {
                id,
                file,
                line: Some(line),
                body: String::new(),
                by: "você".into(),
                sev: None,
                label: None,
                head,
                gitlab_id: None,
                error: None,
                general: false,
            });
        }
        self.edit(id, String::new(), window, cx);
    }

    fn edit(&mut self, id: usize, body: String, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.theme.clone();
        let f =
            cx.new(|cx| TextField::new(t, &body, "Comentário para o autor do MR", true, false, cx));
        cx.subscribe(&f, move |this, f, e: &FieldEvent, cx| {
            let text = f.read(cx).value.trim().to_string();
            match e {
                FieldEvent::Submit if text.is_empty() => {
                    return cx.emit(Event::Toast("Escreva algo antes de salvar".into()));
                }
                FieldEvent::Submit => {
                    if let Some(d) = this
                        .mr_drafts()
                        .and_then(|v| v.iter_mut().find(|d| d.id == id))
                    {
                        d.body = text;
                    }
                }
                FieldEvent::Cancel => {
                    if let Some(v) = this.mr_drafts() {
                        v.retain(|d| d.id != id || !d.body.is_empty());
                    }
                }
                FieldEvent::Changed => return,
            }
            this.editing = None;
            cx.notify();
        })
        .detach();
        window.focus(&f.read(cx).focus_handle(), cx);
        self.editing = Some((id, f));
        cx.notify();
    }

    fn pending(&self) -> Vec<&Draft> {
        let Some((row, _)) = &self.open else {
            return Vec::new();
        };
        self.drafts
            .get(&row.key())
            .map(|v| v.iter().filter(|d| !d.body.is_empty()).collect())
            .unwrap_or_default()
    }

    /// O `askPublish` do protótipo: confirma com a contagem e quem recebe a notificação.
    fn ask_publish(&mut self, cx: &mut Context<Self>) {
        let Some((row, Some(Ok(o)))) = &self.open else {
            return;
        };
        let n = self.pending().len();
        if n == 0 {
            return cx.emit(Event::Toast("Nenhum rascunho para publicar".into()));
        }
        let state = o.detail["state"].as_str().unwrap_or_default();
        if !mrs::can_publish(state) {
            return cx.emit(Event::Toast(format!(
                "O MR está {state}: os rascunhos ficam só aqui (ninguém lê comentário em MR fechado)"
            )));
        }
        let (path, iid) = (row.path.clone(), row.iid);
        let ours: Vec<u64> = self.pending().iter().filter_map(|d| d.gitlab_id).collect();
        let foreign = mrs::foreign_drafts(&path, iid, &ours);
        let extra = if foreign > 0 {
            format!(
                " Atenção: você tem {foreign} rascunho{} seu{} neste MR feito{} fora do Maestro; o GitLab publica junto.",
                if foreign > 1 { "s" } else { "" },
                if foreign > 1 { "s" } else { "" },
                if foreign > 1 { "s" } else { "" }
            )
        } else {
            String::new()
        };
        cx.emit(Event::AskPublish(
            format!(
                "Publicar {n} comentário{} em {} !{iid}?",
                if n > 1 { "s" } else { "" },
                row.proj()
            ),
            format!(
                "Sai como uma review só: {} recebe 1 notificação.{extra}",
                row.who
            ),
        ));
    }

    pub fn publish(&mut self, cx: &mut Context<Self>) {
        let Some((row, Some(Ok(o)))) = self.open.clone() else {
            return;
        };
        let key = row.key();
        let mut drafts: Vec<Draft> = self.drafts.get(&key).cloned().unwrap_or_default();
        drafts.retain(|d| !d.body.is_empty());
        self.posting = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (r, drafts) = cx
                .background_executor()
                .spawn(async move {
                    let r = mrs::publish(&row.path, row.iid, &o, &mut drafts);
                    (r, drafts)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.posting = false;
                match r {
                    Ok(()) => {
                        this.drafts.remove(&key);
                        if let Some((row, _)) = &this.open {
                            mrs::mark_seen(&row.path, row.iid, &row.sha);
                        }
                        cx.emit(Event::Toast("Review publicada".into()));
                    }
                    Err(e) => {
                        this.drafts.insert(key, drafts);
                        cx.emit(Event::Toast(format!(
                            "Review não publicada: {e}. Veja em vermelho."
                        )));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn discard(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some((row, _)) = &self.open else { return };
        let key = row.key();
        let Some(v) = self.mr_drafts() else { return };
        let Some(i) = v.iter().position(|d| d.id == id) else {
            return;
        };
        let d = v.remove(i);
        self.undo.push((key, vec![(i, d)]));
        cx.emit(Event::Discarded("Comentário descartado".into()));
        cx.notify();
    }

    /// O `undo()` do protótipo: devolve o último descarte no lugar.
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some((key, mut g)) = self.undo.pop() else {
            cx.emit(Event::Toast("Nada para desfazer".into()));
            return;
        };
        g.sort_by_key(|x| x.0);
        let v = self.drafts.entry(key).or_default();
        for (i, mut d) in g {
            // a draft note remota já foi apagada: volta como rascunho local
            d.gitlab_id = None;
            v.insert(i.min(v.len()), d);
        }
        cx.emit(Event::Toast("Desfeito".into()));
        cx.notify();
    }

    fn discard_all(&mut self, cx: &mut Context<Self>) {
        let Some((row, _)) = &self.open else { return };
        let (path, iid, key) = (row.path.clone(), row.iid, row.key());
        let ids: Vec<u64> = self.pending().iter().filter_map(|d| d.gitlab_id).collect();
        let gone: Vec<(usize, Draft)> = self
            .mr_drafts()
            .map(|v| v.drain(..).enumerate().collect())
            .unwrap_or_default();
        if gone.is_empty() {
            return;
        }
        let n = gone.len();
        self.undo.push((key, gone));
        cx.emit(Event::Discarded(format!(
            "{n} descartado{}",
            if n > 1 { "s" } else { "" }
        )));
        // o que já tinha ido como draft note sai do GitLab também
        std::thread::spawn(move || mrs::discard_remote(&path, iid, &ids));
        cx.notify();
    }

    pub fn start_review(&mut self, cx: &mut Context<Self>) {
        let row = match &self.open {
            Some((r, _)) => Some(r.clone()),
            None => self.rows(cx).get(self.sel).cloned(),
        };
        let Some(row) = row else { return };
        let inc = mrs::last_seen(&self.seen, &row.path, row.iid).is_some_and(|s| s != row.sha);
        let status = match self.ci.get(&row.key()) {
            Some(Status::Fail) => "failed",
            Some(Status::Running) => "running",
            _ => "success",
        };
        let ci_warn = (!mrs::pipeline_ok(status)).then_some(if status == "failed" {
            "Pipeline falhou"
        } else {
            "Pipeline ainda rodando"
        });
        self.drawer = Some(Drawer {
            row,
            phase: Phase::Setup,
            incremental: inc,
            pid: None,
            ci_warn,
        });
        cx.notify();
    }

    fn run_review(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drawer.as_mut() else {
            return;
        };
        let row = d.row.clone();
        let since = d
            .incremental
            .then(|| mrs::last_seen(&self.seen, &row.path, row.iid))
            .flatten();
        d.phase = Phase::Run(Vec::new());
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Run>();
        std::thread::spawn(move || {
            let base = format!(
                "projects/{}/merge_requests/{}",
                mrs::enc(&row.path),
                row.iid
            );
            let detail = match mrs::read(&base, false) {
                Ok(v) => v,
                Err(e) => return drop(tx.unbounded_send(Run::Failed(e))),
            };
            let notes =
                mrs::read(&format!("{base}/notes?per_page=100"), true).unwrap_or(Value::Null);
            mrs::review(
                &row.path,
                row.iid,
                &row.title,
                &row.target,
                detail["description"].as_str().unwrap_or_default(),
                &notes,
                since,
                |m| drop(tx.unbounded_send(m)),
            );
        });
        cx.spawn(async move |this, cx| {
            while let Some(m) = rx.next().await {
                let stop = this
                    .update(cx, |this, cx| {
                        let raw = match &this.open {
                            Some((_, Some(Ok(o)))) => {
                                o.raw.iter().map(|r| (r.0.clone(), r.2.clone())).collect()
                            }
                            _ => Vec::new(),
                        };
                        let Some(d) = this.drawer.as_mut() else {
                            return true;
                        };
                        match (m, &mut d.phase) {
                            (Run::Pid(p), _) => d.pid = Some(p),
                            (Run::Ok(t), Phase::Run(s)) => {
                                for x in s.iter_mut().filter(|x| x.0 == "run") {
                                    x.0 = "ok";
                                }
                                s.push(("ok", t));
                            }
                            (Run::Tool(t), Phase::Run(s)) => s.push(("sub", t)),
                            (Run::Done(mut f, cost), _) => {
                                mrs::mark_outside(&mut f, &raw);
                                d.phase = Phase::Done(
                                    f.into_iter()
                                        .map(|x| {
                                            let on = !x.outside;
                                            (x, on)
                                        })
                                        .collect(),
                                    cost,
                                );
                                mrs::mark_seen(&d.row.path, d.row.iid, &d.row.sha);
                            }
                            (Run::Failed(e), _) => {
                                cx.emit(Event::Toast(format!("Revisão falhou: {e}")));
                                this.drawer = None;
                            }
                            _ => {}
                        }
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if stop {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn close_drawer(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.drawer.as_ref().and_then(|d| d.pid) {
            let _ = std::process::Command::new("kill")
                .arg(p.to_string())
                .status();
        }
        self.drawer = None;
        cx.notify();
    }

    /// O `makeDrafts` do protótipo: os achados marcados viram rascunhos; nada é publicado.
    fn make_drafts(&mut self, cx: &mut Context<Self>) {
        let Some(Drawer {
            row,
            phase: Phase::Done(f, _),
            ..
        }) = self.drawer.take()
        else {
            return;
        };
        let same = self
            .open
            .as_ref()
            .is_some_and(|(r, _)| r.key() == row.key());
        if !same {
            self.open_row(row.clone(), cx);
        }
        let head = self.head();
        let list = self.drafts.entry(row.key()).or_default();
        for (x, on) in f.into_iter().filter(|x| x.1) {
            let _ = on;
            self.next_id += 1;
            list.push(Draft {
                id: self.next_id,
                file: x.file,
                line: x.line,
                body: x.body,
                by: "Claude".into(),
                sev: Some(x.severity),
                label: Some(x.label),
                head: head.clone(),
                gitlab_id: None,
                error: None,
                general: x.outside,
            });
        }
        cx.emit(Event::Toast(
            "Rascunhos criados. Nada foi publicado.".into(),
        ));
        cx.notify();
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.filter.read(cx).focus_handle().is_focused(window) {
            if e.keystroke.key == "escape" && self.editing.is_none() {
                self.filter.update(cx, |f, cx| f.set(String::new(), cx));
                window.focus(&self.focus, cx);
            }
            return;
        }
        let k = &e.keystroke;
        let shift_r = k.key == "r" && k.modifiers.shift && !k.modifiers.control;
        match (k.key.as_str(), &self.open) {
            _ if shift_r => self.start_review(cx),
            ("enter", Some(_)) if k.modifiers.control => self.ask_publish(cx),
            ("z", Some(_)) if k.modifiers.control && !k.modifiers.shift => self.undo(cx),
            ("c", Some(_)) if !k.modifiers.modified() => self.new_draft(window, cx),
            ("escape", Some(_)) => self.back(cx),
            ("j" | "down", None) => {
                self.sel = (self.sel + 1).min(self.rows(cx).len().saturating_sub(1))
            }
            ("k" | "up", None) => self.sel = self.sel.saturating_sub(1),
            ("enter", None) => {
                if let Some(r) = self.rows(cx).get(self.sel).cloned() {
                    self.open_row(r, cx);
                }
            }
            ("/", None) => {
                let f = self.filter.read(cx).focus_handle();
                window.focus(&f, cx);
            }
            ("r", None) if !k.modifiers.modified() => self.refresh(cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn ci_label(s: Option<Status>, t: &Theme) -> (String, Hsla) {
        match s {
            Some(Status::Ok) => ("✓ CI".into(), t.ok),
            Some(Status::Running) => ("⟳ CI".into(), yellow()),
            Some(Status::Fail) => ("✗ CI".into(), red()),
            Some(Status::None) => ("– sem CI".into(), t.muted),
            None => ("…".into(), t.muted),
        }
    }

    fn inbox(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let rows = self.rows(cx);
        let tab = |id: &'static str, label: &str, which: Tab, i: usize| {
            let n = match &self.lists[i] {
                Some(Ok(v)) => v.len().to_string(),
                _ => "…".into(),
            };
            div()
                .id(id)
                .flex()
                .gap(px(4.))
                .items_center()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(if self.tab == which { t.fg } else { t.muted })
                .when(self.tab == which, |d| d.bg(t.accent_soft))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = which;
                    this.sel = 0;
                    cx.notify();
                }))
                .child(label.to_string())
                .child(
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .child(n),
                )
        };
        let group = mrs::group();
        let bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(tab("ib-rev", "Para revisar", Tab::Review, 0))
                    .child(tab("ib-mine", "Meus", Tab::Mine, 1))
                    .child(tab("ib-grp", &group, Tab::Group, 2)),
            )
            .child(div().flex_1())
            .child(div().w(px(200.)).child(self.filter.clone()));
        let i = match self.tab {
            Tab::Review => 0,
            Tab::Mine => 1,
            Tab::Group => 2,
        };
        let q = self.filter.read(cx).value.clone();
        let mut list = div()
            .id("ib-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .relative()
            .child(tour::anchor(&self.anchors, "ilist"));
        match &self.lists[i] {
            None => list = list.child(div().p(px(24.)).text_color(t.muted).child("Lendo os MRs…")),
            Some(Err(e)) => {
                list = list.child(self.empty("Não deu para ler os MRs".into(), e.clone()))
            }
            Some(Ok(_)) if rows.is_empty() && !q.is_empty() => {
                list = list.child(self.empty(
                    format!("Nenhum MR bate com “{q}”"),
                    "Esc limpa o filtro.".into(),
                ))
            }
            Some(Ok(_)) if rows.is_empty() => {
                list = list.child(self.empty(
                    "Nenhum MR aguardando você".into(),
                    "A inbox relê a cada 60 s; R força.".into(),
                ))
            }
            Some(Ok(_)) => {
                for (i, r) in rows.into_iter().enumerate() {
                    let (pk, pt) = self.pill(&r);
                    let pc = match pk {
                        "new" => t.accent,
                        "inc" => t.orange,
                        _ => t.muted,
                    };
                    let (ci, cc) = Self::ci_label(self.ci.get(&r.key()).copied(), &t);
                    let unread = pk == "new";
                    let r2 = r.clone();
                    list = list.child(
                        div()
                            .id(("ib", i))
                            .flex()
                            .gap(px(12.))
                            .items_center()
                            .px(px(12.))
                            .py(px(8.))
                            .border_b_1()
                            .border_color(t.line)
                            .cursor_pointer()
                            .when(i == self.sel, |d| d.bg(t.accent_soft))
                            .hover(|d| d.bg(t.raise))
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                this.sel = i;
                                if e.click_count() == 2 {
                                    this.open_row(r2.clone(), cx);
                                }
                                cx.notify();
                            }))
                            .child(div().w(px(14.)).flex_none().when(unread, |d| {
                                d.child(div().size(px(7.)).rounded_full().bg(t.accent))
                            }))
                            .child(
                                div()
                                    .w(px(200.))
                                    .flex_none()
                                    .flex()
                                    .gap(px(4.))
                                    .font_family(t.mono.clone())
                                    .text_size(px(13.))
                                    .truncate()
                                    .child(r.proj().to_string())
                                    .child(div().text_color(t.muted).child(format!("!{}", r.iid))),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(r.title.clone()))
                            .child(
                                div()
                                    .w(px(120.))
                                    .flex_none()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(t.muted)
                                    .child(r.who.clone()),
                            )
                            .child(div().w(px(70.)).flex_none().text_color(cc).child(ci))
                            .child(
                                div().w(px(110.)).flex_none().child(
                                    div()
                                        .font_family(t.mono.clone())
                                        .text_size(px(12.))
                                        .px(px(7.))
                                        .rounded(px(10.))
                                        .border_1()
                                        .border_color(if pk.is_empty() { t.line } else { pc })
                                        .text_color(pc)
                                        .whitespace_nowrap()
                                        .child(pt),
                                ),
                            ),
                    );
                }
            }
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(bar)
            .child(list)
    }

    fn empty(&self, title: String, text: String) -> Div {
        let t = &self.theme;
        div()
            .m_auto()
            .max_w(px(380.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_center()
            .text_color(t.muted)
            .child(div().text_size(px(16.)).text_color(t.fg).child(title))
            .child(text)
    }

    fn code(&self, text: &str) -> StyledText {
        let hl: Vec<_> = diff::highlight(text)
            .into_iter()
            .map(|(r, tok)| (r, editor::style(&self.theme, tok)))
            .collect();
        StyledText::new(text.to_string()).with_highlights(hl)
    }

    fn thread(&self, d: &Value, i: usize, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let notes = d["notes"].as_array().cloned().unwrap_or_default();
        let resolved = notes.first().is_some_and(|n| n["resolved"] == true);
        let resolvable = notes.first().is_some_and(|n| n["resolvable"] == true);
        let id = d["id"].as_str().unwrap_or_default().to_string();
        let who = |n: &Value| format!("@{}", n["author"]["username"].as_str().unwrap_or_default());
        let first = notes.first().cloned().unwrap_or_default();
        let mut bd = div()
            .px(px(10.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(first["body"].as_str().unwrap_or_default().to_string());
        for n in notes.iter().skip(1) {
            bd = bd.child(
                div()
                    .border_l_2()
                    .border_color(t.line)
                    .pl(px(8.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(who(n)))
                    .child(n["body"].as_str().unwrap_or_default().to_string()),
            );
        }
        let path = self.open.as_ref().map(|(r, _)| (r.path.clone(), r.iid));
        div()
            .ml(px(94.))
            .mr(px(12.))
            .my(px(6.))
            .max_w(px(720.))
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .bg(t.raise)
            .font_family(t.sans)
            .text_size(px(14.))
            .when(resolved, |d| d.opacity(0.7))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .px(px(10.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .child(
                        div()
                            .text_color(t.fg)
                            .font_weight(FontWeight::MEDIUM)
                            .child(who(&first)),
                    )
                    .child(
                        first["created_at"]
                            .as_str()
                            .unwrap_or_default()
                            .chars()
                            .take(10)
                            .collect::<String>(),
                    )
                    .child(div().flex_1())
                    .when(resolvable, |d| {
                        d.child(
                            button(
                                &t,
                                ("res", i),
                                if resolved { "Reabrir" } else { "✓ Resolver" },
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let Some((p, iid)) = path.clone() else { return };
                                    let ep = format!(
                                        "projects/{}/merge_requests/{iid}/discussions/{id}",
                                        mrs::enc(&p)
                                    );
                                    let body = serde_json::json!({ "resolved": !resolved });
                                    match mrs::write("PUT", &ep, &body) {
                                        Ok(_) => {
                                            if let Some((row, _)) = this.open.clone() {
                                                this.open_row(row, cx);
                                            }
                                        }
                                        Err(e) => cx.emit(Event::Toast(format!("GitLab: {e}"))),
                                    }
                                },
                            )),
                        )
                    }),
            )
            .child(bd)
    }

    fn draft_box(&self, d: &Draft, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let id = d.id;
        let editing = self
            .editing
            .as_ref()
            .filter(|(i, _)| *i == id)
            .map(|(_, f)| f.clone());
        let border = if d.error.is_some() { red() } else { yellow() };
        let sev_color = match d.sev.as_deref() {
            Some("blocker" | "major") => red(),
            Some("minor") => t.orange,
            _ => t.muted,
        };
        div()
            .ml(px(94.))
            .mr(px(12.))
            .my(px(6.))
            .max_w(px(720.))
            .rounded(px(6.))
            .font_family(t.sans)
            .text_size(px(14.))
            .border_1()
            .when(d.error.is_none(), |x| {
                x.border_dashed().bg(Hsla {
                    a: 0.07,
                    ..yellow()
                })
            })
            .border_color(border)
            .when(self.posting, |x| x.opacity(0.6))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .px(px(10.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(yellow())
                            .child("RASCUNHO"),
                    )
                    .child(format!("· {}", d.by))
                    .when_some(d.sev.clone(), |x, s| {
                        x.child(
                            div()
                                .font_family(t.mono.clone())
                                .text_size(px(12.))
                                .px(px(6.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(sev_color)
                                .text_color(sev_color)
                                .child(s),
                        )
                    })
                    .when(d.general || d.line.is_none(), |x| x.child("nota geral"))
                    .child(div().flex_1())
                    .when(self.posting, |x| x.child("publicando…"))
                    .when(editing.is_none() && !self.posting, |x| {
                        let body = d.body.clone();
                        x.child(button(&t, ("ed", id), "Editar", false).on_click(
                            cx.listener(move |this, _, w, cx| this.edit(id, body.clone(), w, cx)),
                        ))
                        .child(
                            button(&t, ("del", id), "Descartar", false)
                                .text_color(red())
                                .on_click(cx.listener(move |this, _, _, cx| this.discard(id, cx))),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when_some(editing, |x, f| {
                        x.child(f).child(
                            div()
                                .text_size(px(13.))
                                .text_color(t.muted)
                                .child("Ctrl+⏎ salva · Esc cancela"),
                        )
                    })
                    .when(self.editing.as_ref().is_none_or(|(i, _)| *i != id), |x| {
                        let label = d
                            .label
                            .clone()
                            .filter(|l| !l.is_empty())
                            .map(|l| format!("{l}: "))
                            .unwrap_or_default();
                        x.child(format!("{label}{}", d.body))
                    })
                    .when_some(d.error.clone(), |x, e| {
                        x.child(div().text_size(px(13.)).text_color(red()).child(e))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .child(
                                        button(&t, ("retry", id), "Tentar de novo", false)
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.publish(cx)),
                                            ),
                                    )
                                    .child(
                                        button(&t, ("gen", id), "Publicar como nota geral", false)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(d) = this
                                                    .mr_drafts()
                                                    .and_then(|v| v.iter_mut().find(|d| d.id == id))
                                                {
                                                    d.general = true;
                                                    d.error = None;
                                                }
                                                this.publish(cx);
                                            })),
                                    ),
                            )
                    }),
            )
    }

    fn file_block(&self, fi: usize, f: &FileDiff, o: &Opened, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let (a, r) = f.counts();
        let drafts: Vec<Draft> = self
            .open
            .as_ref()
            .and_then(|(row, _)| self.drafts.get(&row.key()))
            .cloned()
            .unwrap_or_default();
        let threads: Vec<(usize, Value)> = o
            .discussions
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, d)| d["notes"][0]["system"] != true)
            .map(|(i, d)| (i, d.clone()))
            .collect();
        let mut block = div().flex().flex_col().child(
            div()
                .flex()
                .gap(px(10.))
                .items_center()
                .px(px(12.))
                .py(px(6.))
                .bg(t.panel)
                .border_t_1()
                .border_b_1()
                .border_color(t.line)
                .font_family(t.sans)
                .child(div().font_family(t.mono.clone()).child(f.path.clone()))
                .when(f.new, |x| {
                    x.child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .px(px(7.))
                            .rounded(px(10.))
                            .border_1()
                            .border_color(t.accent)
                            .text_color(t.accent)
                            .child("novo"),
                    )
                })
                .child(div().text_color(t.ok).child(format!("+{a}")))
                .child(div().text_color(red()).child(format!("−{r}"))),
        );
        for (hi, h) in f.hunks.iter().enumerate() {
            block = block.child(
                div()
                    .px(px(12.))
                    .text_color(t.comment)
                    .bg(rgba(0x6272a41f))
                    .child(h.header.clone()),
            );
            for (li, l) in h.lines.iter().enumerate() {
                let sel = self
                    .selected
                    .as_ref()
                    .is_some_and(|(p, n)| *p == f.path && Some(*n) == l.new);
                let (path, n) = (f.path.clone(), l.new);
                let bg = match l.kind {
                    Kind::Add => Some(Hsla::from(rgba(0x50fa7b1a))),
                    Kind::Del => Some(Hsla::from(rgba(0xff55551f))),
                    Kind::Context => None,
                };
                let sign = match l.kind {
                    Kind::Add => "+",
                    Kind::Del => "-",
                    Kind::Context => "",
                };
                let num = |v: Option<u32>| {
                    div()
                        .w(px(40.))
                        .flex_none()
                        .pr(px(8.))
                        .text_right()
                        .text_color(t.comment)
                        .child(v.map(|x| x.to_string()).unwrap_or_default())
                };
                let id = (fi * 1000 + hi) * 10_000 + li;
                block = block.child(
                    div()
                        .id(("ml", id))
                        .group("ml")
                        .relative()
                        .flex()
                        .h(px(20.))
                        .cursor_pointer()
                        .when_some(bg, |x, c| x.bg(c))
                        .hover(|x| x.bg(rgba(0x44475a73)))
                        .when(sel, |x| x.border_l_3().border_color(t.accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(n) = n {
                                this.selected = Some((path.clone(), n));
                            }
                            cx.notify();
                        }))
                        .child(num(l.old))
                        .child(num(l.new))
                        .child(div().w(px(18.)).flex_none().text_color(t.muted).child(sign))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .pr(px(8.))
                                .child(self.code(&l.text)),
                        )
                        .when(l.new.is_some(), |x| {
                            let path = f.path.clone();
                            let n = l.new.unwrap();
                            x.child(
                                div()
                                    .id(("mcm", id))
                                    .absolute()
                                    .left(px(62.))
                                    .top(px(2.))
                                    .size(px(16.))
                                    .rounded(px(3.))
                                    .bg(t.accent)
                                    .text_color(rgb(0x282a36))
                                    .font_family(t.sans)
                                    .text_size(px(13.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .invisible()
                                    .group_hover("ml", |s| s.visible())
                                    .when(sel, |x| x.visible())
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        this.selected = Some((path.clone(), n));
                                        this.new_draft(w, cx);
                                    }))
                                    .child("+"),
                            )
                        }),
                );
                if let Some(n) = l.new {
                    for (ti, d) in threads.iter().filter(|(_, d)| {
                        d["notes"][0]["position"]["new_path"] == f.path.as_str()
                            && d["notes"][0]["position"]["new_line"].as_u64() == Some(n as u64)
                    }) {
                        block = block.child(self.thread(d, *ti, cx));
                    }
                    for d in drafts
                        .iter()
                        .filter(|d| d.file == f.path && d.line == Some(n))
                    {
                        block = block.child(self.draft_box(d, cx));
                    }
                }
            }
        }
        for d in drafts.iter().filter(|d| {
            d.file == f.path
                && d.line.is_some_and(|l| {
                    !f.hunks
                        .iter()
                        .flat_map(|h| &h.lines)
                        .any(|x| x.new == Some(l))
                })
        }) {
            block = block.child(self.draft_box(d, cx));
        }
        block
    }

    /// A coluna "Arquivos do MR" (o `renderMRFiles` do protótipo).
    pub fn files_panel(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let (row, Some(Ok(o))) = self.open.as_ref()? else {
            return None;
        };
        let t = self.theme.clone();
        let drafts = self.drafts.get(&row.key()).cloned().unwrap_or_default();
        let mut col = div()
            .id("mr-files")
            .w(px(270.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.line)
            .child(
                div()
                    .min_h(px(30.))
                    .flex()
                    .items_center()
                    .px(px(10.))
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .border_b_1()
                    .border_color(t.line)
                    .child("ARQUIVOS DO MR"),
            );
        for (i, f) in o.files.iter().enumerate() {
            let (a, r) = f.counts();
            let n = drafts.iter().filter(|d| d.file == f.path).count();
            let (dir, name) = f
                .path
                .rsplit_once('/')
                .map_or((String::new(), f.path.clone()), |(d, n)| {
                    (format!("{d}/"), n.into())
                });
            col = col.child(
                div()
                    .id(("mf", i))
                    .flex()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(3.))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.raise))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.scroll.scroll_to_item(i);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_family(t.mono.clone())
                            .text_size(px(13.))
                            .child(div().text_color(t.muted).child(dir))
                            .child(name),
                    )
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .text_color(if n > 0 { yellow() } else { t.muted })
                            .child(if n > 0 {
                                format!("{n} rasc.")
                            } else {
                                format!("+{a} −{r}")
                            }),
                    ),
            );
        }
        Some(col)
    }

    fn mr(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let Some((row, opened)) = self.open.clone() else {
            return div();
        };
        let ci = Self::ci_label(self.ci.get(&row.key()).copied(), &t);
        let header = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(14.))
            .pt(px(10.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(12.))
                    .items_baseline()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.title.clone()),
                    )
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_color(t.muted)
                            .child(format!("{} !{}", row.proj(), row.iid)),
                    )
                    .child(div().text_color(t.muted).child(row.who.clone()))
                    .child(div().text_color(ci.1).child(ci.0)),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .pb(px(6.))
                    .child(
                        button(&t, "mr-back", "← Inbox", false)
                            .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        button(&t, "mr-rev", "Revisar com Claude ⇧R", true)
                            .on_click(cx.listener(|this, _, _, cx| this.start_review(cx))),
                    ),
            );
        let body = match &opened {
            None => div().p(px(24.)).text_color(t.muted).child("Lendo o MR…"),
            Some(Err(e)) => self.empty("Não deu para abrir o MR".into(), e.clone()),
            Some(Ok(o)) => {
                let mut list = div()
                    .flex()
                    .flex_col()
                    .pb(px(40.))
                    .font_family(t.mono.clone())
                    .text_size(px(13.5))
                    .line_height(px(22.));
                for (i, f) in o.files.iter().enumerate() {
                    list = list.child(self.file_block(i, f, o, cx));
                }
                let general: Vec<Draft> = self
                    .drafts
                    .get(&row.key())
                    .map(|v| v.iter().filter(|d| d.line.is_none()).cloned().collect())
                    .unwrap_or_default();
                for d in &general {
                    list = list.child(self.draft_box(d, cx));
                }
                div().flex_1().min_h_0().flex().flex_col().child(
                    div()
                        .id("mr-diff")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(list),
                )
            }
        };
        let pend = self.pending().len();
        let s = if pend > 1 { "s" } else { "" };
        let pubbar = div()
            .flex()
            .flex_wrap()
            .gap(px(10.))
            .items_center()
            .px(px(12.))
            .py(px(8.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .when(pend > 0, |d| {
                d.child(
                    div()
                        .text_color(yellow())
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("{pend} rascunho{s} não publicado{s}")),
                )
                .child(div().text_color(t.muted).child("só você vê"))
            })
            .when(pend == 0, |d| {
                d.child(
                    div()
                        .text_color(t.muted)
                        .child("C numa linha comenta; ⇧R pede revisão ao Claude."),
                )
            })
            .child(div().flex_1())
            .child(
                button(&t, "mr-dall", "Descartar todos", false)
                    .text_color(red())
                    .when(pend == 0, |d| d.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.discard_all(cx))),
            )
            .child(
                button(&t, "mr-pub", "Publicar review Ctrl+⏎", true)
                    .when(pend == 0 || self.posting, |d| d.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.ask_publish(cx))),
            );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .child(pubbar)
    }

    fn drawer(&self, cx: &mut Context<Self>) -> Option<Div> {
        let d = self.drawer.as_ref()?;
        let t = self.theme.clone();
        let fieldset = |legend: &str| {
            div()
                .border_1()
                .border_color(t.line)
                .rounded(px(6.))
                .px(px(12.))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .child(legend.to_string()),
                )
        };
        let radio = |on: bool| {
            div()
                .size(px(12.))
                .rounded_full()
                .border_1()
                .border_color(if on { t.accent } else { t.line })
                .when(on, |d| d.bg(t.accent))
        };
        let (body, foot) = match &d.phase {
            Phase::Setup => {
                let can_inc = mrs::last_seen(&self.seen, &d.row.path, d.row.iid)
                    .is_some_and(|s| s != d.row.sha);
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .when_some(d.ci_warn, |b, w| {
                        b.child(div().text_color(red()).child(format!(
                            "{w} em {} !{}. Revisar código que o CI ainda pode reprovar costuma gerar achados repetidos.",
                            d.row.proj(),
                            d.row.iid
                        )))
                    })
                    .child(
                        fieldset("Escopo")
                            .child(
                                div()
                                    .id("sc-all")
                                    .flex()
                                    .gap(px(8.))
                                    .items_center()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(d) = this.drawer.as_mut() {
                                            d.incremental = false;
                                        }
                                        cx.notify();
                                    }))
                                    .child(radio(!d.incremental))
                                    .child("MR inteiro"),
                            )
                            .child(
                                div()
                                    .id("sc-inc")
                                    .flex()
                                    .gap(px(8.))
                                    .items_center()
                                    .when(!can_inc, |x| x.opacity(0.45))
                                    .when(can_inc, |x| {
                                        x.cursor_pointer().on_click(cx.listener(
                                            |this, _, _, cx| {
                                                if let Some(d) = this.drawer.as_mut() {
                                                    d.incremental = true;
                                                }
                                                cx.notify();
                                            },
                                        ))
                                    })
                                    .child(radio(d.incremental))
                                    .child("Só commits novos desde a última revisão"),
                            ),
                    )
                    .child(
                        fieldset("Contexto enviado").child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .font_family(t.mono.clone())
                                .text_size(px(13.5))
                                .child(
                                    "· descrição do MR e todos os comentários existentes (dedup)",
                                )
                                .child("· rubrica ~/.claude/mr-review-rubric.md")
                                .child("· CLAUDE.md e skills do repo"),
                        ),
                    )
                    .child(
                        div()
                            .text_color(t.muted)
                            .child("Os achados viram rascunhos. Nada é publicado sem você."),
                    );
                let foot = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        button(&t, "rv-x", "Cancelar", false)
                            .on_click(cx.listener(|this, _, _, cx| this.close_drawer(cx))),
                    )
                    .child(
                        button(&t, "rv-go", "Iniciar revisão", true)
                            .on_click(cx.listener(|this, _, _, cx| this.run_review(cx))),
                    );
                (body, foot)
            }
            Phase::Run(steps) => {
                let mut b = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .font_family(t.mono.clone())
                    .text_size(px(13.5));
                for (k, s) in steps {
                    let (g, c, pl) = match *k {
                        "ok" => ("✓", t.ok, 0.),
                        "run" => ("⟳", yellow(), 0.),
                        _ => ("↳", t.muted, 18.),
                    };
                    b = b.child(div().pl(px(pl)).text_color(c).child(format!("{g} {s}")));
                }
                b = b.child(
                    div()
                        .text_color(yellow())
                        .child("⟳ o Claude está lendo o MR…"),
                );
                let foot = div().child(
                    button(&t, "rv-stop", "Parar", false)
                        .text_color(red())
                        .on_click(cx.listener(|this, _, _, cx| this.close_drawer(cx))),
                );
                (b, foot)
            }
            Phase::Done(f, cost) => {
                let on = f.iter().filter(|x| x.1).count();
                let mut b = div().flex().flex_col().gap(px(8.));
                if f.is_empty() {
                    b = b.child(div().text_color(t.muted).child("Nada novo para comentar: o que o Claude achou já estava nos comentários do MR."));
                }
                for (i, (x, sel)) in f.iter().enumerate() {
                    let sc = match x.severity.as_str() {
                        "blocker" | "major" => red(),
                        "minor" => t.orange,
                        _ => t.muted,
                    };
                    b = b.child(
                        div()
                            .id(("fd", i))
                            .flex()
                            .gap(px(10.))
                            .px(px(10.))
                            .py(px(8.))
                            .border_1()
                            .border_color(t.line)
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(!sel, |d| d.opacity(0.55))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Drawer {
                                    phase: Phase::Done(f, _),
                                    ..
                                }) = this.drawer.as_mut()
                                {
                                    f[i].1 = !f[i].1;
                                }
                                cx.notify();
                            }))
                            .child(checkbox(&t, *sel))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap(px(8.))
                                            .items_center()
                                            .font_family(t.mono.clone())
                                            .text_size(px(13.))
                                            .child(
                                                div()
                                                    .px(px(6.))
                                                    .rounded(px(3.))
                                                    .border_1()
                                                    .border_color(sc)
                                                    .text_color(sc)
                                                    .text_size(px(12.))
                                                    .child(x.severity.clone()),
                                            )
                                            .child(format!(
                                                "{}:{}",
                                                x.file,
                                                x.line.map_or("–".into(), |l| l.to_string())
                                            ))
                                            .when(x.outside, |d| {
                                                d.child(
                                                    div()
                                                        .font_family(t.sans)
                                                        .text_color(t.orange)
                                                        .child(
                                                            "⚠ fora do diff, vai como nota geral",
                                                        ),
                                                )
                                            }),
                                    )
                                    .child(x.body.clone()),
                            ),
                    );
                }
                let foot = div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(t.muted)
                            .child(format!("US$ {}", crate::sidebar::brl(*cost))),
                    )
                    .child(div().flex_1())
                    .child(
                        button(&t, "rv-x", "Descartar tudo", false)
                            .on_click(cx.listener(|this, _, _, cx| this.close_drawer(cx))),
                    )
                    .child(
                        button(
                            &t,
                            "rv-mk",
                            &format!("Criar {on} rascunho{}", if on != 1 { "s" } else { "" }),
                            true,
                        )
                        .when(on == 0, |d| d.opacity(0.45))
                        .when(on > 0, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| this.make_drafts(cx)))
                        }),
                    );
                (b, foot)
            }
        };
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(px(440.))
                .bg(t.raise)
                .border_l_1()
                .border_color(t.line)
                .shadow_lg()
                .flex()
                .flex_col()
                .occlude()
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .items_center()
                        .px(px(14.))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(t.line)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Revisão Claude"),
                        )
                        .child(
                            div()
                                .font_family(t.mono.clone())
                                .text_color(t.muted)
                                .child(format!("{} !{}", d.row.proj(), d.row.iid)),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("rv-c")
                                .cursor_pointer()
                                .text_color(t.muted)
                                .on_click(cx.listener(|this, _, _, cx| this.close_drawer(cx)))
                                .child("×"),
                        ),
                )
                .child(
                    div()
                        .id("rv-db")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(px(14.))
                        .py(px(12.))
                        .child(body),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .px(px(14.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(t.line)
                        .child(foot),
                ),
        )
    }

    pub fn in_mr(&self) -> bool {
        self.open.is_some()
    }
}

impl Render for MrsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let center = if self.open.is_some() {
            self.mr(cx)
        } else {
            self.inbox(cx)
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .bg(t.win)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .children(self.files_panel(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(center),
            )
            .children(self.drawer(cx))
    }
}
