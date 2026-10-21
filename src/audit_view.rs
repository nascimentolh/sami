use std::{collections::HashMap, path::PathBuf};

use futures::StreamExt;
use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    audit::{self, Ev, Finding, Ignored, Opts, Report, St, Wi},
    form::{FieldEvent, TextField, button, checkbox},
    theme::Theme,
    tour::{self, Anchors},
};

pub enum Event {
    Toast(String),
    /// "Não é problema", com desfazer no toast
    Ignored(Ignored),
    /// confirmação antes de abrir os work items: (título, texto)
    AskCreate(String, String),
    /// "Corrigir em feature": (título, texto, branch, prompt)
    AskFix(String, String, String, String),
    /// "Won't do": (achado, título, texto); o motivo vem do campo do diálogo
    AskWont(usize, String, String),
    /// fechado como won't do, com desfazer (reabre)
    Wont(usize, u64),
}

/// (correção, por quê) ou o erro
type Iter = Result<(String, String), String>;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Cached,
    Setup,
    Run,
    Stopped,
    Done,
}

/// O `renderAudit` do protótipo: relatório salvo, nova auditoria, progresso e achados.
pub struct AuditView {
    repo: Option<PathBuf>,
    report: Option<Report>,
    since: Option<(usize, usize)>,
    total: usize,
    siblings: Vec<String>,
    ignored: Vec<Ignored>,
    phase: Phase,
    inc: bool,
    pillars: [bool; 5],
    budget: Entity<TextField>,
    base: Entity<TextField>,
    why: bool,
    prod: bool,
    gcloud: bool,
    dropped_open: bool,
    steps: Vec<(&'static str, String)>,
    tool: Option<String>,
    spent: f64,
    cap: f64,
    pid: Option<u32>,
    pub seen: bool,
    /// já rodou nesta sessão: Cancelar volta para o resultado, não para o relatório salvo
    had: bool,
    open: Option<usize>,
    editing: Option<(usize, Entity<TextField>)>,
    creating: bool,
    /// o work item no GitLab, por iid (`None` enquanto lê)
    wis: HashMap<u64, Option<Result<Wi, String>>>,
    note: Entity<TextField>,
    /// "Iterar com o Claude": iid → (pedido, `None` enquanto pensa, ou (correção, por quê))
    iters: HashMap<u64, (String, Option<Iter>)>,
    /// comentar, fechar, reabrir ou atualizar em andamento
    writing: bool,
    scroll: ScrollHandle,
    focus: FocusHandle,
    theme: Theme,
    anchors: Anchors,
}

impl EventEmitter<Event> for AuditView {}

fn red() -> Hsla {
    rgb(0xff5555).into()
}
fn yellow() -> Hsla {
    rgb(0xf1fa8c).into()
}

const WHY: &[&str] = &[
    "Saída em JSON validada por schema (o campo de cada work item vem pronto), em vez de Markdown para parsear.",
    "Evidência obrigatória: arquivo:linha com o trecho citado; o que não dá para verificar não entra.",
    "Severidade P0 a P3 definida por impacto, igual aos work items do email-service.",
    "Uma segunda passada verifica cada achado contra o código (o papel que o Codex fazia) e descarta falsos positivos.",
    "Lista dos work items existentes e dos ignorados no prompt: não repete nada.",
    "Somente leitura: as ferramentas liberadas não escrevem nada nem acessam a rede, exceto os MCPs que você marcar.",
];

impl AuditView {
    pub fn new(theme: Theme, anchors: Anchors, cx: &mut Context<Self>) -> Self {
        let t = theme.clone();
        let budget = cx.new(|cx| TextField::new(t.clone(), "2", "", false, true, cx));
        let base = cx.new(|cx| TextField::new(t.clone(), "", "sha", false, true, cx));
        let note = cx.new(|cx| {
            TextField::new(
                t,
                "",
                "Comentar no work item, ou pedir ao Claude outra correção (ele lê o código e os comentários, só leitura)",
                true,
                false,
                cx,
            )
        });
        for f in [&budget, &base, &note] {
            cx.subscribe(f, |_, _, _: &FieldEvent, cx| cx.notify())
                .detach();
        }
        Self {
            repo: None,
            report: None,
            since: None,
            total: 0,
            siblings: Vec::new(),
            ignored: Vec::new(),
            phase: Phase::Cached,
            inc: true,
            pillars: [true; 5],
            budget,
            base,
            why: false,
            prod: false,
            gcloud: std::env::var_os("PATH")
                .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("gcloud").is_file())),
            dropped_open: false,
            steps: Vec::new(),
            tool: None,
            spent: 0.,
            cap: 2.,
            pid: None,
            seen: true,
            had: false,
            open: None,
            editing: None,
            creating: false,
            wis: HashMap::new(),
            note,
            iters: HashMap::new(),
            writing: false,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            theme,
            anchors,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Lê só o relatório salvo e o `git` local: abrir o projeto nunca roda auditoria.
    pub fn set_repo(&mut self, repo: Option<PathBuf>, cx: &mut Context<Self>) {
        if repo == self.repo || self.running() {
            return;
        }
        self.repo = repo;
        self.reload();
        self.phase = Phase::Cached;
        cx.notify();
    }

    /// Relê o relatório e o `git` ao abrir o modo (commits novos desde a última).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.running() {
            self.reload();
            cx.notify();
        }
    }

    fn reload(&mut self) {
        let Some(repo) = &self.repo else { return };
        self.report = audit::latest(repo);
        self.since = self
            .report
            .as_ref()
            .and_then(|r| audit::since(repo, &r.sha));
        self.total = audit::tracked(repo);
        self.ignored = audit::ignored(repo);
        let name = self.name();
        self.siblings = audit::siblings(&crate::vault::load(&crate::vault::dir()), &name);
    }

    fn name(&self) -> String {
        self.repo
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn running(&self) -> bool {
        self.phase == Phase::Run
    }

    pub fn spent(&self) -> f64 {
        self.spent
    }

    /// O card "Auditoria" da lateral: (rodando, k, m).
    pub fn card(&self) -> (bool, String, String) {
        if self.running() {
            return (
                true,
                "rodando".into(),
                format!("US$ {}", audit::brl(self.spent)),
            );
        }
        match (&self.report, self.since) {
            (None, _) => (false, String::new(), "nunca rodou".into()),
            (Some(r), Some((c, _))) if c > 0 => {
                (false, when(r), format!("{c} commit{} desde a última", s(c)))
            }
            (Some(r), _) => (false, when(r), "em dia".into()),
        }
    }

    /// Pronta com achados e ainda não vista: entra na fila de atenção.
    pub fn ready(&self) -> bool {
        !self.seen && self.phase == Phase::Done
    }

    pub fn new_audit(&mut self, inc: bool, cx: &mut Context<Self>) {
        if self.running() {
            return;
        }
        self.inc = inc;
        let b = if inc { "2" } else { "12" };
        self.budget.update(cx, |f, cx| f.set(b.into(), cx));
        self.phase = Phase::Setup;
        cx.notify();
    }

    fn base_sha(&self, cx: &App) -> String {
        match &self.report {
            Some(r) => r.sha.clone(),
            None => self.base.read(cx).value.trim().to_string(),
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let base = self.base_sha(cx);
        if self.inc && audit::since(&repo, &base).is_none() {
            cx.emit(Event::Toast(if base.is_empty() {
                "Diga desde qual sha a incremental começa.".into()
            } else {
                format!("O sha {base} não existe neste repo.")
            }));
            return;
        }
        let cap = self
            .budget
            .read(cx)
            .value
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|v| *v >= 0.5)
            .unwrap_or(if self.inc { 2. } else { 12. });
        let o = Opts {
            inc: self.inc,
            base,
            pillars: audit::PILLARS
                .iter()
                .zip(self.pillars)
                .filter(|(_, on)| *on)
                .map(|(p, _)| p.0)
                .collect(),
            budget: cap,
            prod: self.prod,
        };
        if o.pillars.is_empty() {
            cx.emit(Event::Toast("Marque ao menos um pilar.".into()));
            return;
        }
        self.cap = cap;
        self.spent = 0.;
        self.steps.clear();
        self.tool = None;
        self.phase = Phase::Run;
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Ev>();
        std::thread::spawn(move || audit::run(&repo, o, |e| drop(tx.unbounded_send(e))));
        cx.spawn(async move |this, cx| {
            while let Some(e) = rx.next().await {
                let stop = this
                    .update(cx, |this, cx| {
                        if this.phase != Phase::Run {
                            return true;
                        }
                        match e {
                            Ev::Pid(p) => this.pid = Some(p),
                            Ev::Step(k, t) => {
                                for x in this.steps.iter_mut().filter(|x| x.0 == "run") {
                                    x.0 = "ok";
                                }
                                this.tool = None;
                                this.steps.push((k, t));
                            }
                            Ev::Tool(t) => this.tool = Some(t),
                            Ev::Spent(c) => this.spent = c,
                            Ev::Done(r) => {
                                let n = r.findings.iter().filter(|f| f.open()).count();
                                this.report = Some(r);
                                this.since = Some((0, 0));
                                this.phase = Phase::Done;
                                this.open = None;
                                this.seen = false;
                                this.had = true;
                                this.pid = None;
                                cx.emit(Event::Toast(format!(
                                    "Auditoria pronta: {n} achado{} para revisar",
                                    s(n)
                                )));
                            }
                            Ev::Failed(m) => this.stop(&m),
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

    fn stop(&mut self, why: &str) {
        if let Some(p) = self.pid.take() {
            let _ = std::process::Command::new("kill")
                .arg(p.to_string())
                .status();
        }
        self.phase = Phase::Stopped;
        self.tool = None;
        for x in self.steps.iter_mut().filter(|x| x.0 == "run") {
            x.0 = "";
        }
        self.steps.push(("", why.into()));
    }

    fn visible(&self) -> Vec<(usize, &Finding)> {
        let Some(r) = &self.report else {
            return Vec::new();
        };
        r.findings
            .iter()
            .enumerate()
            .filter(|(_, f)| !self.is_ignored(f))
            .collect()
    }

    /// O won't do também entra no ignore (some das próximas auditorias), mas fica neste relatório.
    fn is_ignored(&self, f: &Finding) -> bool {
        !matches!(f.st, St::Wont(_))
            && self
                .ignored
                .iter()
                .any(|i| i.title == f.title && i.location == f.location)
    }

    fn selected(&self) -> Vec<usize> {
        self.visible()
            .into_iter()
            .filter(|(_, f)| f.on && f.open())
            .map(|(i, _)| i)
            .collect()
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let (Some(repo), Some(r)) = (&self.repo, &self.report)
            && let Err(e) = audit::save(repo, r)
        {
            cx.emit(Event::Toast(format!(
                "Não deu para salvar o relatório: {e}"
            )));
        }
    }

    fn toggle(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(f) = self.report.as_mut().and_then(|r| r.findings.get_mut(i))
            && f.open()
        {
            f.on = !f.on;
            self.save(cx);
        }
        cx.notify();
    }

    fn ignore(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(f) = self.report.as_ref().and_then(|r| r.findings.get(i)) else {
            return;
        };
        let ig = Ignored {
            title: f.title.clone(),
            location: f.location.clone(),
        };
        self.ignored.push(ig.clone());
        if let Some(repo) = &self.repo {
            audit::set_ignored(repo, &self.ignored);
        }
        self.open = None;
        cx.emit(Event::Ignored(ig));
        cx.notify();
    }

    pub fn unignore(&mut self, ig: &Ignored, cx: &mut Context<Self>) {
        self.ignored.retain(|x| x != ig);
        if let Some(repo) = &self.repo {
            audit::set_ignored(repo, &self.ignored);
        }
        cx.notify();
    }

    fn edit(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.report.as_ref().and_then(|r| r.findings.get(i)) else {
            return;
        };
        let t = self.theme.clone();
        let body = f.body();
        let field = cx.new(|cx| TextField::new(t, &body, "", true, true, cx));
        cx.subscribe(&field, move |this, _, e: &FieldEvent, cx| match e {
            FieldEvent::Submit => this.save_text(i, cx),
            FieldEvent::Cancel => {
                this.editing = None;
                cx.notify();
            }
            FieldEvent::Changed => cx.notify(),
        })
        .detach();
        window.focus(&field.read(cx).focus_handle(), cx);
        self.editing = Some((i, field));
        cx.notify();
    }

    fn save_text(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some((_, field)) = self.editing.take() else {
            return;
        };
        let v = field.read(cx).value.clone();
        if let Some(f) = self.report.as_mut().and_then(|r| r.findings.get_mut(i)) {
            f.custom = Some(v);
        }
        self.save(cx);
        cx.notify();
    }

    fn ask_create(&mut self, cx: &mut Context<Self>) {
        let sel = self.selected();
        let Some(r) = &self.report else { return };
        if sel.is_empty() || self.creating {
            return;
        }
        let mut labels: Vec<&str> = Vec::new();
        for i in &sel {
            for l in &r.findings[*i].labels {
                if !labels.contains(&l.as_str()) {
                    labels.push(l);
                }
            }
        }
        let n = sel.len();
        cx.emit(Event::AskCreate(
            format!("Abrir {n} work item{} em {}?", s(n), self.name()),
            format!(
                "Títulos no padrão [P0–P3] tipo(escopo), labels {}, corpo com Problema / Evidência / Correção sugerida / Impacto. Ninguém é atribuído e nada é corrigido.",
                labels.join(", ")
            ),
        ));
    }

    /// Depois do "Abrir work items" confirmado. Só escreve pelo `mrs::write`.
    pub fn create(&mut self, cx: &mut Context<Self>) {
        let sel = self.selected();
        let (Some(repo), Some(r)) = (self.repo.clone(), &self.report) else {
            return;
        };
        let Some(project) = audit::project_of(&repo) else {
            cx.emit(Event::Toast("O origin deste repo não é do GitLab.".into()));
            return;
        };
        let items: Vec<(usize, Finding)> =
            sel.iter().map(|i| (*i, r.findings[*i].clone())).collect();
        self.creating = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    items
                        .into_iter()
                        .map(|(i, f)| (i, audit::create(&project, &f)))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.creating = false;
                let mut ok = Vec::new();
                let mut err = None;
                if let Some(r) = this.report.as_mut() {
                    for (i, x) in res {
                        match x {
                            Ok(iid) => {
                                r.findings[i].st = St::Created(iid);
                                r.findings[i].on = false;
                                ok.push(iid);
                            }
                            Err(e) => err = Some(e),
                        }
                    }
                }
                this.save(cx);
                let msg = match (ok.first(), ok.last(), err) {
                    (_, _, Some(e)) if ok.is_empty() => format!("Não deu para abrir: {e}"),
                    (Some(a), Some(b), e) => format!(
                        "{} work item{} aberto{}: #{a}{}{}",
                        ok.len(),
                        s(ok.len()),
                        s(ok.len()),
                        if a == b {
                            String::new()
                        } else {
                            format!(" a #{b}")
                        },
                        e.map(|e| format!(" · falhou: {e}")).unwrap_or_default()
                    ),
                    _ => String::new(),
                };
                cx.emit(Event::Toast(msg));
                cx.notify();
            });
        })
        .detach();
    }

    fn ask_fix(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(f) = self.report.as_ref().and_then(|r| r.findings.get(i)) else {
            return;
        };
        let St::Created(iid) = f.st else { return };
        let br = audit::fix_branch(iid, &f.title);
        cx.emit(Event::AskFix(
            format!("Corrigir #{iid} numa feature nova?"),
            format!(
                "Cria a worktree {br} e abre o Claude com o work item #{iid} como tarefa. Ele não commita nem abre MR sem você pedir."
            ),
            br,
            format!(
                "Corrija o work item #{iid} ({}). Leia o work item com glab antes. Não commite.",
                f.item_title()
            ),
        ));
    }

    fn expand(&mut self, i: usize, cx: &mut Context<Self>) {
        self.open = if self.open == Some(i) { None } else { Some(i) };
        self.editing = None;
        self.note.update(cx, |n, cx| n.set(String::new(), cx));
        if self.open.is_some()
            && let Some(iid) = self.iid_of(i)
        {
            self.load_wi(iid, cx);
        }
        cx.notify();
    }

    fn iid_of(&self, i: usize) -> Option<u64> {
        self.report.as_ref()?.findings.get(i)?.iid()
    }

    fn project(&self) -> Option<String> {
        audit::project_of(self.repo.as_ref()?)
    }

    fn load_wi(&mut self, iid: u64, cx: &mut Context<Self>) {
        let Some(project) = self.project() else {
            self.wis
                .insert(iid, Some(Err("o origin deste repo não é do GitLab".into())));
            return;
        };
        self.wis.entry(iid).or_insert(None);
        cx.spawn(async move |this, cx| {
            let w = cx
                .background_executor()
                .spawn(async move { audit::work_item(&project, iid) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.wis.insert(iid, Some(w));
                cx.notify();
            });
        })
        .detach();
    }

    /// Uma escrita no GitLab fora da thread de UI; no fim relê o work item e avisa.
    fn gitlab(
        &mut self,
        iid: u64,
        ok: String,
        op: impl FnOnce(&str) -> Result<(), String> + Send + 'static,
        done: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project() else {
            return cx.emit(Event::Toast("O origin deste repo não é do GitLab.".into()));
        };
        if self.writing {
            return;
        }
        self.writing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { op(&project) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.writing = false;
                match r {
                    Ok(()) => {
                        done(this, cx);
                        if !ok.is_empty() {
                            cx.emit(Event::Toast(ok));
                        }
                    }
                    Err(e) => cx.emit(Event::Toast(format!("O GitLab recusou: {e}"))),
                }
                this.load_wi(iid, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn open_gitlab(&self, iid: u64) {
        let url = match self.wis.get(&iid) {
            Some(Some(Ok(w))) if !w.web.is_empty() => w.web.clone(),
            _ => match self.project() {
                Some(p) => format!("https://gitlab.com/{p}/-/issues/{iid}"),
                None => return,
            },
        };
        crate::spawn_reaped(std::process::Command::new("xdg-open").arg(url));
    }

    fn comment(&mut self, iid: u64, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.note.read(cx).value.trim().to_string();
        if text.is_empty() {
            window.focus(&self.note.read(cx).focus_handle(), cx);
            return;
        }
        self.gitlab(
            iid,
            "Comentário publicado no work item".into(),
            move |p| audit::comment(p, iid, &text),
            |this, cx| this.note.update(cx, |n, cx| n.set(String::new(), cx)),
            cx,
        );
    }

    fn iterate(&mut self, iid: u64, window: &mut Window, cx: &mut Context<Self>) {
        let ask = self.note.read(cx).value.trim().to_string();
        let (Some(repo), Some(Some(Ok(w)))) = (self.repo.clone(), self.wis.get(&iid).cloned())
        else {
            return;
        };
        if ask.is_empty() {
            window.focus(&self.note.read(cx).focus_handle(), cx);
            return;
        }
        self.note.update(cx, |n, cx| n.set(String::new(), cx));
        self.iters.insert(iid, (ask.clone(), None));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { audit::iterate(&repo, iid, &w, &ask) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(it) = this.iters.get_mut(&iid) {
                    it.1 = Some(r.map(|(f, y, _)| (f, y)));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_iter(&mut self, iid: u64, cx: &mut Context<Self>) {
        let (Some((ask, Some(Ok((fix, why))))), Some(Some(Ok(w)))) =
            (self.iters.get(&iid).cloned(), self.wis.get(&iid))
        else {
            return;
        };
        let body = audit::replace_fix(&w.body, &fix);
        let note = format!("Correção sugerida atualizada a pedido: {ask}\n\n{why}");
        self.gitlab(
            iid,
            format!("#{iid} atualizado no GitLab"),
            move |p| audit::update_fix(p, iid, &body, &note),
            move |this, _| {
                this.iters.remove(&iid);
            },
            cx,
        );
    }

    fn ask_wont(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(iid) = self.iid_of(i) else { return };
        cx.emit(Event::AskWont(
            i,
            format!("Fechar #{iid} como won't do?"),
            "Fecha o work item no GitLab com a label wont-do e o motivo abaixo como comentário. Ele também entra no .maestro/audit-ignore.json, para não voltar nas próximas auditorias.".into(),
        ));
    }

    fn set_wont(&mut self, i: usize, on: bool, cx: &mut Context<Self>) {
        let Some(f) = self.report.as_mut().and_then(|r| r.findings.get_mut(i)) else {
            return;
        };
        let Some(iid) = f.iid() else { return };
        f.st = if on { St::Wont(iid) } else { St::Created(iid) };
        let ig = Ignored {
            title: f.title.clone(),
            location: f.location.clone(),
        };
        self.ignored.retain(|x| *x != ig);
        if on {
            self.ignored.push(ig);
        }
        if let Some(repo) = &self.repo {
            audit::set_ignored(repo, &self.ignored);
        }
        self.save(cx);
    }

    /// Depois do diálogo: o motivo vira comentário, o work item fecha e entra no ignore.
    pub fn wont_do(&mut self, i: usize, why: String, cx: &mut Context<Self>) {
        let Some(iid) = self.iid_of(i) else { return };
        self.gitlab(
            iid,
            String::new(),
            move |p| audit::wont_do(p, iid, &why),
            move |this, cx| {
                this.set_wont(i, true, cx);
                cx.emit(Event::Wont(i, iid));
            },
            cx,
        );
    }

    pub fn reopen(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(iid) = self.iid_of(i) else { return };
        self.gitlab(
            iid,
            format!("#{iid} reaberto"),
            move |p| audit::reopen(p, iid),
            move |this, cx| this.set_wont(i, false, cx),
            cx,
        );
    }

    pub fn repo(&self) -> Option<PathBuf> {
        self.repo.clone()
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some()
            || self.budget.read(cx).focus_handle().is_focused(window)
            || self.base.read(cx).focus_handle().is_focused(window)
            || self.note.read(cx).focus_handle().is_focused(window)
        {
            return;
        }
        let k = &e.keystroke;
        if k.modifiers.modified() {
            return;
        }
        let vis: Vec<usize> = self.visible().into_iter().map(|(i, _)| i).collect();
        let pos = self.open.and_then(|o| vis.iter().position(|i| *i == o));
        match k.key.as_str() {
            "space" if matches!(self.phase, Phase::Done | Phase::Cached) => {
                if let Some(o) = self.open {
                    self.toggle(o, cx);
                }
            }
            "j" | "down" if !vis.is_empty() => {
                self.open = Some(vis[pos.map_or(0, |p| (p + 1).min(vis.len() - 1))])
            }
            "k" | "up" if !vis.is_empty() => {
                self.open = Some(vis[pos.map_or(0, |p| p.saturating_sub(1))])
            }
            "escape" if self.phase == Phase::Setup => self.cancel(),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn cancel(&mut self) {
        self.phase = if self.had { Phase::Done } else { Phase::Cached };
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let (l1, pill) = match &self.report {
            Some(r) => (
                format!(
                    "última {} em {} · ",
                    if r.kind == "full" {
                        "completa"
                    } else {
                        "incremental"
                    },
                    when(r)
                ),
                match self.since {
                    Some((c, f)) => format!(
                        "{c} commit{} e {f} arquivo{} mudaram desde então",
                        s(c),
                        s(f)
                    ),
                    None => "o sha da última não está mais neste repo".into(),
                },
            ),
            None => (
                "nunca rodou".into(),
                "nenhum relatório salvo em .maestro/audits".into(),
            ),
        };
        let cached = self.phase == Phase::Cached;
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(14.))
            .pt(px(10.))
            .pb(px(8.))
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
                            .child(format!("Auditoria · {}", self.name())),
                    )
                    .child(div().flex().text_color(t.muted).child(l1).when_some(
                        self.report.as_ref(),
                        |d, r| {
                            d.child(div().font_family(t.mono.clone()).child(r.sha.clone()))
                                .child(format!(" · US$ {}", audit::brl(r.cost)))
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .child(
                        div()
                            .px(px(7.))
                            .py(px(1.))
                            .rounded(px(10.))
                            .border_1()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .when(cached && self.report.is_some(), |d| {
                                d.border_color(t.orange).text_color(t.orange)
                            })
                            .when(!(cached && self.report.is_some()), |d| {
                                d.border_color(t.line).text_color(t.muted)
                            })
                            .child(pill),
                    )
                    .child(div().flex_1())
                    .when(!self.running(), |d| {
                        d.child(
                            div().child(tour::anchor(&self.anchors, "auNew")).child(
                                button(&t, "au-new", "Nova auditoria…", true).on_click(
                                    cx.listener(|this, _, _, cx| this.new_audit(true, cx)),
                                ),
                            ),
                        )
                    }),
            )
    }

    fn fieldset(&self, legend: &str) -> Div {
        let t = &self.theme;
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
    }

    fn setup(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let radio = |on: bool| {
            div()
                .size(px(12.))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(if on { t.accent } else { t.line })
                .when(on, |d| d.bg(t.accent))
        };
        let files = self.since.map_or(0, |x| x.1);
        let input = |f: &Entity<TextField>, w: f32| div().w(px(w)).child(f.clone());
        let inc_base = match &self.report {
            Some(r) => div().font_family(t.mono.clone()).child(r.sha.clone()),
            // sem relatório salvo, a incremental pergunta o sha
            None => input(&self.base, 110.),
        };
        let est_inc = if self.report.is_some() {
            audit::estimate(true, files, self.total)
        } else {
            let base = self.base.read(cx).value.trim().to_string();
            let f = self
                .repo
                .as_ref()
                .and_then(|r| audit::since(r, &base))
                .map_or(0, |x| x.1);
            audit::estimate(true, f, self.total)
        };
        let kind = if self.inc { "incremental" } else { "completa" };
        let ctx = format!(
            "Contexto incluído: CLAUDE.md, Dockerfile, .gitlab-ci.yml e k8s/ do repo, o mapa do ripwire, os work items deste repo{} (para não repetir e para reconhecer padrões), e as decisões do vault (para não sugerir o que já foi decidido contra).",
            if self.siblings.is_empty() {
                String::new()
            } else {
                format!(" e os de {}", self.siblings.join(" e "))
            }
        );
        let mut pil = div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(6.));
        for (i, (_, label)) in audit::PILLARS.iter().enumerate() {
            pil = pil.child(
                div()
                    .id(("au-pil", i))
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pillars[i] = !this.pillars[i];
                        cx.notify();
                    }))
                    .child(checkbox(&t, self.pillars[i]))
                    .child(label.to_string()),
            );
        }
        let mut why = div()
            .id("au-why")
            .cursor_pointer()
            .text_color(t.muted)
            .on_click(cx.listener(|this, _, _, cx| {
                this.why = !this.why;
                cx.notify();
            }))
            .child(format!(
                "{} O que mudou em relação ao prompt do snippet 5996330",
                if self.why { "▾" } else { "▸" }
            ));
        if self.why {
            let mut ul = div().flex().flex_col().gap(px(4.)).pt(px(8.)).pl(px(18.));
            for w in WHY {
                ul = ul.child(format!("• {w}"));
            }
            why = why.child(ul);
        }
        div()
            .id("au-setup")
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .px(px(18.))
            .py(px(16.))
            .overflow_y_scroll()
            .child(
                self.fieldset("Escopo")
                    .child(
                        div()
                            .id("au-inc")
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inc = true;
                                this.budget.update(cx, |b, cx| b.set("2".into(), cx));
                                cx.notify();
                            }))
                            .child(radio(self.inc))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Incremental"))
                            .child("(recomendado): só o que mudou desde")
                            .child(inc_base)
                            .child(div().text_color(t.muted).child(est_inc)),
                    )
                    .child(
                        div()
                            .id("au-full")
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inc = false;
                                this.budget.update(cx, |b, cx| b.set("12".into(), cx));
                                cx.notify();
                            }))
                            .child(radio(!self.inc))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Completa (holística)"),
                            )
                            .child(": o repo inteiro.")
                            .child(
                                div()
                                    .text_color(t.muted)
                                    .child(audit::estimate(false, 0, self.total)),
                            ),
                    ),
            )
            .child(self.fieldset("Pilares").child(pil))
            .child(
                self.fieldset("Limites e contexto")
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .items_center()
                            .child("Teto de gasto")
                            .child(input(&self.budget, 80.))
                            .child(div().text_color(t.muted).child("US$ ("))
                            .child(
                                div()
                                    .font_family(t.mono.clone())
                                    .text_color(t.muted)
                                    .child("--max-budget-usd"),
                            )
                            .child(
                                div()
                                    .text_color(t.muted)
                                    .child(": o processo para ao atingir)"),
                            ),
                    )
                    .child(
                        div()
                            .id("au-prod")
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .items_center()
                            .when(!self.gcloud, |d| d.opacity(0.45))
                            .when(self.gcloud, |d| {
                                d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                                    this.prod = !this.prod;
                                    cx.notify();
                                }))
                            })
                            .child(checkbox(&t, self.prod))
                            .child("Incluir dados de produção via gcloud e kubectl (Cloud Logging, Cloud SQL, Pub/Sub, pods)")
                            .child(div().text_color(t.muted).child(if self.gcloud {
                                "só leitura, para validar achados de custo e infra; +~US$ 0,50"
                            } else {
                                "gcloud não está no PATH"
                            })),
                    )
                    .child(div().text_color(t.muted).child(ctx)),
            )
            .child(why)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        button(&t, "au-cancel", "Cancelar", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cancel();
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button(&t, "au-go", &format!("Iniciar auditoria {kind}"), true)
                            .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                    ),
            )
    }

    fn progress(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let mut steps = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .font_family(t.mono.clone())
            .text_size(px(13.5));
        for (k, s) in &self.steps {
            let (g, c, pl) = match *k {
                "ok" => ("✓", t.ok, 0.),
                "run" => ("⟳", yellow(), 0.),
                _ => ("↳", t.muted, 18.),
            };
            steps = steps.child(div().pl(px(pl)).text_color(c).child(format!("{g} {s}")));
        }
        if let Some(tool) = &self.tool {
            steps = steps.child(
                div()
                    .pl(px(18.))
                    .text_color(t.muted)
                    .truncate()
                    .child(format!("↳ {tool}")),
            );
        }
        let run = self.running();
        div()
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(18.))
            .py(px(16.))
            .child(steps)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(div().font_family(t.mono.clone()).child(format!(
                        "US$ {} de {}",
                        audit::brl(self.spent),
                        audit::brl(self.cap).trim_end_matches(",00")
                    )))
                    .child(div().flex_1())
                    .when(run, |d| {
                        d.child(
                            button(&t, "au-stop", "Parar", false)
                                .text_color(red())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.stop("processo claude encerrado");
                                    cx.notify();
                                })),
                        )
                    })
                    .when(!run, |d| {
                        d.child(
                            div().text_color(t.muted).child(
                                "Parada. Nada foi criado; o gasto até aqui fica registrado.",
                            ),
                        )
                    }),
            )
    }

    fn st_pill(&self, f: &Finding) -> Div {
        let t = &self.theme;
        let (text, c) = match &f.st {
            St::New => ("novo".to_string(), t.accent),
            St::Cross(r) => (format!("novo · mesmo padrão de {r}"), t.accent),
            St::Dup(r) => (format!("já existe {r}"), t.muted),
            St::Created(i) => (format!("aberto #{i}"), t.ok),
            St::Wont(i) => (format!("won't do #{i}"), t.muted),
        };
        div()
            .px(px(7.))
            .py(px(1.))
            .rounded(px(10.))
            .border_1()
            .border_color(if matches!(f.st, St::Dup(_) | St::Wont(_)) {
                t.line
            } else {
                c
            })
            .text_color(c)
            .font_family(t.mono.clone())
            .text_size(px(12.))
            .whitespace_nowrap()
            .child(text)
    }

    fn finding(&self, i: usize, f: &Finding, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let o = self.open == Some(i);
        let row = div()
            .flex()
            .gap(px(10.))
            .items_center()
            .px(px(12.))
            .py(px(7.))
            .child(
                div()
                    .id(("au-on", i))
                    .when(f.open(), |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(i, cx)))
                    })
                    .when(!f.open(), |d| d.opacity(0.45))
                    .child(checkbox(&t, f.on)),
            )
            .child(
                div()
                    .id(("au-open", i))
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.expand(i, cx)))
                    .child(div().truncate().child(format!(
                        "{} {}",
                        if o { "▾" } else { "▸" },
                        f.item_title()
                    )))
                    .child(
                        div()
                            .flex()
                            .text_size(px(13.))
                            .text_color(t.muted)
                            .child(div().font_family(t.mono.clone()).child(f.location.clone()))
                            .child(format!(
                                " · {} · confiança {}",
                                f.labels.join(", "),
                                f.confidence
                            )),
                    ),
            )
            .child(self.st_pill(f))
            .when(matches!(f.st, St::Created(_)), |d| {
                d.child(
                    button(&t, ("au-fix", i), "Corrigir em feature", false)
                        .on_click(cx.listener(move |this, _, _, cx| this.ask_fix(i, cx))),
                )
            });
        let mut el = div().border_b_1().border_color(t.line).child(row);
        if o && let Some(iid) = f.iid() {
            return el.child(
                div()
                    .min_w_0()
                    .pl(px(42.))
                    .pr(px(12.))
                    .pb(px(12.))
                    .pt(px(4.))
                    .child(self.wi_view(i, iid, cx)),
            );
        }
        if o {
            let inner =
                match &self.editing {
                    Some((j, field)) if *j == i => div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .min_h(px(220.))
                                .text_size(px(13.5))
                                .child(field.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(button(&t, ("au-sv", i), "Salvar texto", true).on_click(
                                    cx.listener(move |this, _, _, cx| this.save_text(i, cx)),
                                ))
                                .child(button(&t, ("au-cn", i), "Cancelar", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.editing = None;
                                        cx.notify();
                                    }),
                                )),
                        ),
                    _ => div()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t.line)
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .items_center()
                                .px(px(10.))
                                .py(px(6.))
                                .border_b_1()
                                .border_color(t.line)
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Prévia do work item"),
                                )
                                .child(div().flex_1())
                                .when(f.open(), |d| {
                                    d.child(button(&t, ("au-ed", i), "Editar", false).on_click(
                                        cx.listener(move |this, _, w, cx| this.edit(i, w, cx)),
                                    ))
                                    .child(
                                        button(&t, ("au-ig", i), "Não é problema", false)
                                            .text_color(red())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.ignore(i, cx)
                                            })),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(8.))
                                .font_family(t.mono.clone())
                                .text_size(px(13.5))
                                .line_height(px(21.))
                                .whitespace_normal()
                                .child(md(&t, &f.body())),
                        ),
                };
            el = el.child(
                div()
                    .pl(px(42.))
                    .pr(px(12.))
                    .pb(px(12.))
                    .pt(px(4.))
                    .child(inner),
            );
        }
        el
    }

    /// O work item aberto: texto do GitLab, comentários, rascunho do Claude e a caixa de comentar.
    fn wi_view(&self, i: usize, iid: u64, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let wont = self
            .report
            .as_ref()
            .and_then(|r| r.findings.get(i))
            .is_some_and(|f| matches!(f.st, St::Wont(_)));
        let w = self.wis.get(&iid).cloned().flatten();
        let n = match &w {
            Some(Ok(w)) => w.notes.len(),
            _ => 0,
        };
        let state = match &w {
            None => "lendo do GitLab…".to_string(),
            Some(Err(_)) => "não deu para ler".into(),
            Some(Ok(w)) => format!(
                "{} · {n} comentário{}",
                if wont {
                    "fechado como won't do"
                } else if w.closed {
                    "fechado"
                } else {
                    "aberto"
                },
                s(n)
            ),
        };
        let busy = self.writing;
        let head = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .items_center()
            .px(px(10.))
            .py(px(6.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Work item #{iid}")),
            )
            .child(div().text_size(px(13.)).text_color(t.muted).child(state))
            .child(div().flex_1())
            .child(
                button(&t, ("au-gl", i), "Abrir no GitLab ↗", false)
                    .on_click(cx.listener(move |this, _, _, _| this.open_gitlab(iid))),
            )
            .child(if wont {
                button(&t, ("au-ro", i), "Reabrir", false)
                    .when(busy, |d| d.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| this.reopen(i, cx)))
            } else {
                button(&t, ("au-wd", i), "Won't do", false)
                    .text_color(red())
                    .when(busy, |d| d.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| this.ask_wont(i, cx)))
            });
        let mut body = div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.));
        match &w {
            None => {}
            Some(Err(e)) => {
                body = body.child(div().text_color(red()).child(e.clone()));
            }
            Some(Ok(w)) => {
                body = body
                    .child(
                        div()
                            .min_w_0()
                            .font_family(t.mono.clone())
                            .text_size(px(13.5))
                            .line_height(px(21.))
                            .whitespace_normal()
                            .child(md(&t, &w.body)),
                    )
                    .children(w.notes.iter().map(|(who, when, text)| {
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .pl(px(8.))
                            .border_l_2()
                            .border_color(t.line)
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(t.muted)
                                    .child(format!("{who} · {when}")),
                            )
                            .child(div().min_w_0().whitespace_normal().child(md(&t, text)))
                    }));
            }
        }
        let it = self.iters.get(&iid);
        if let Some((ask, res)) = it {
            let ready = matches!(res, Some(Ok(_)));
            let draft = div()
                .min_w_0()
                .rounded(px(6.))
                .border_1()
                .border_dashed()
                .border_color(yellow())
                .bg(Hsla {
                    a: 0.07,
                    ..yellow()
                })
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .items_center()
                        .px(px(10.))
                        .py(px(6.))
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(yellow())
                                .child(match res {
                                    None => "CLAUDE PENSANDO…",
                                    Some(Err(_)) => "O CLAUDE NÃO RESPONDEU",
                                    Some(Ok(_)) => "NOVA CORREÇÃO SUGERIDA",
                                }),
                        )
                        .child(div().flex_1())
                        .when(ready, |d| {
                            d.child(
                                button(&t, ("au-ap", i), "Atualizar o work item", true)
                                    .when(busy, |d| d.opacity(0.5))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.apply_iter(iid, cx)),
                                    ),
                            )
                        })
                        .when(res.is_some(), |d| {
                            d.child(button(&t, ("au-dc", i), "Descartar", false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.iters.remove(&iid);
                                    cx.notify();
                                }),
                            ))
                        }),
                );
            let mut inner = div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(px(6.))
                .px(px(10.))
                .pb(px(8.))
                .child(div().text_color(t.muted).child(format!("Você: {ask}")));
            match res {
                Some(Ok((fix, why))) => {
                    inner = inner
                        .child(
                            div()
                                .min_w_0()
                                .font_family(t.mono.clone())
                                .text_size(px(13.5))
                                .line_height(px(21.))
                                .whitespace_normal()
                                .child(md(&t, fix)),
                        )
                        .child(div().text_color(t.muted).child(why.clone()))
                        .child(div().text_size(px(13.)).text_color(t.muted).child(
                            "Troca só a seção Correção sugerida e comenta no work item o que mudou e por quê.",
                        ));
                }
                Some(Err(e)) => inner = inner.child(div().text_color(red()).child(e.clone())),
                None => {}
            }
            body = body.child(draft.child(inner));
        } else if !wont && matches!(w, Some(Ok(_))) {
            body = body
                .child(
                    div()
                        .min_w_0()
                        .min_h(px(64.))
                        .text_size(px(13.5))
                        .child(self.note.clone()),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .items_center()
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(t.muted)
                                .child("o comentário vai como você"),
                        )
                        .child(div().flex_1())
                        .child(
                            button(&t, ("au-cm", i), "Comentar", false)
                                .when(busy, |d| d.opacity(0.5))
                                .on_click(
                                    cx.listener(move |this, _, w, cx| this.comment(iid, w, cx)),
                                ),
                        )
                        .child(
                            button(&t, ("au-it", i), "Iterar com o Claude", true).on_click(
                                cx.listener(move |this, _, w, cx| this.iterate(iid, w, cx)),
                            ),
                        ),
                );
        }
        div()
            .min_w_0()
            .rounded(px(6.))
            .border_1()
            .border_color(t.line)
            .child(head)
            .child(body)
    }

    fn report_view(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let Some(r) = &self.report else {
            return div().p(px(24.)).text_color(t.muted).child(
                "Nenhuma auditoria salva para este repo. Abrir o projeto não roda auditoria; comece com uma incremental desde um sha ou uma completa em Nova auditoria…",
            );
        };
        let vis = self.visible();
        let mut list = div().flex().flex_col().pt(px(8.)).pb(px(30.));
        if self.phase == Phase::Cached && self.since.is_some_and(|x| x.0 > 0) {
            list = list.child(
                div()
                    .mx(px(12.))
                    .mb(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .bg(t.raise)
                    .border_1()
                    .border_color(t.line)
                    .child(format!(
                        "Este é o relatório salvo de {}. Abrir o projeto não roda auditoria; rode uma incremental quando quiser cobrir os {} commits novos.",
                        when(r),
                        self.since.map_or(0, |x| x.0)
                    )),
            );
        }
        if vis.is_empty() {
            list = list.child(div().px(px(14.)).py(px(10.)).text_color(t.muted).child(
                "Nenhum achado novo: o que sobrou da verificação já existia ou foi ignorado.",
            ));
        }
        for (sev, desc) in audit::SEVT {
            let g: Vec<&(usize, &Finding)> = vis.iter().filter(|(_, f)| f.sev == *sev).collect();
            if g.is_empty() {
                continue;
            }
            list = list.child(
                div()
                    .flex()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(5.))
                    .bg(t.panel)
                    .border_b_1()
                    .border_color(t.line)
                    .child(div().font_weight(FontWeight::BOLD).child(sev.to_string()))
                    .child(div().text_color(t.muted).child(desc.to_string())),
            );
            for (i, f) in g {
                list = list.child(self.finding(*i, f, cx));
            }
        }
        let ign: Vec<&Finding> = r.findings.iter().filter(|f| self.is_ignored(f)).collect();
        let mut summary = format!("{} descartados na verificação", r.dropped.len());
        if !ign.is_empty() {
            summary.push_str(&format!(" · {} marcados como “não é problema”", ign.len()));
        }
        let mut dropped = div().px(px(14.)).py(px(10.)).text_color(t.muted).child(
            div()
                .id("au-drop")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dropped_open = !this.dropped_open;
                    cx.notify();
                }))
                .child(format!(
                    "{} {summary}",
                    if self.dropped_open { "▾" } else { "▸" }
                )),
        );
        if self.dropped_open {
            let mut ul = div().flex().flex_col().gap(px(4.)).pt(px(6.)).pl(px(18.));
            for d in &r.dropped {
                ul = ul.child(format!("• {d}"));
            }
            for (k, f) in ign.iter().enumerate() {
                let ig = Ignored {
                    title: f.title.clone(),
                    location: f.location.clone(),
                };
                ul =
                    ul.child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .child(format!("• {}: ignorado por você", f.title))
                            .child(button(&t, ("au-un", k), "Desfazer", false).on_click(
                                cx.listener(move |this, _, _, cx| this.unignore(&ig, cx)),
                            )),
                    );
            }
            dropped = dropped.child(ul);
        }
        list.child(dropped)
    }

    fn pubbar(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let n = self.selected().len();
        div()
            .flex()
            .flex_wrap()
            .gap(px(10.))
            .items_center()
            .px(px(12.))
            .py(px(8.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .child(
                div()
                    .when(n > 0, |d| {
                        d.text_color(yellow()).font_weight(FontWeight::MEDIUM)
                    })
                    .when(n == 0, |d| d.text_color(t.muted))
                    .child(format!("{n} selecionado{}", s(n))),
            )
            .child(
                div()
                    .text_color(t.muted)
                    .child("nada é criado no GitLab sem este botão"),
            )
            .child(div().flex_1())
            .child(
                button(
                    &t,
                    "au-create",
                    &format!("Abrir {n} work item{}", s(n)),
                    true,
                )
                .when(n == 0 || self.creating, |d| d.opacity(0.45))
                .on_click(cx.listener(|this, _, _, cx| this.ask_create(cx))),
            )
    }
}

fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// O `md()` do protótipo: `código` com fundo e sem as crases.
fn md(t: &Theme, s: &str) -> StyledText {
    let (mut text, mut hl) = (String::new(), Vec::new());
    for (i, part) in s.split('`').enumerate() {
        let start = text.len();
        text.push_str(part);
        if i % 2 == 1 {
            hl.push((
                start..text.len(),
                HighlightStyle {
                    background_color: Some(t.panel),
                    ..Default::default()
                },
            ));
        }
    }
    StyledText::new(text).with_highlights(hl)
}

/// `3 out` (ou `hoje`).
fn when(r: &Report) -> String {
    if r.date == crate::vault::today() {
        return "hoje".into();
    }
    const M: [&str; 12] = [
        "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
    ];
    let mut p = r.date.split('-').skip(1);
    match (p.next().and_then(|m| m.parse::<usize>().ok()), p.next()) {
        (Some(m @ 1..=12), Some(d)) => format!("{} {}", d.trim_start_matches('0'), M[m - 1]),
        _ => r.date.clone(),
    }
}

impl Render for AuditView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        self.seen = true;
        let main = match self.phase {
            Phase::Setup => div().flex_1().min_h_0().flex().child(self.setup(cx)),
            Phase::Run | Phase::Stopped => div().flex_1().min_h_0().child(self.progress(cx)),
            Phase::Cached | Phase::Done => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("au-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(self.report_view(cx)),
                )
                .when(self.report.is_some(), |d| d.child(self.pubbar(cx))),
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(t.win)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(self.header(cx))
            .child(main)
    }
}
