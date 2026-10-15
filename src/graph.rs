use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Duration,
};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    form::{FieldEvent, TextField, checkbox},
    theme::Theme,
    vault::Note,
    vault_view::FeatureRef,
};

pub enum Event {
    Open(String),
    Focus(String),
    Toast(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    /// `repos`, `decisions`, `daily`, `.`, `feature`, `mr` ou `ghost` (link quebrado)
    pub dir: String,
    pub exists: bool,
    pub deg: usize,
    pub key: Option<String>,
    pub pane: Option<String>,
    pub x: f32,
    pub y: f32,
    vx: f32,
    vy: f32,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Opts {
    pub local: bool,
    pub ghosts: bool,
    pub feats: bool,
}

/// O `vaultGraph()` do protótipo: notas, `[[links]]`, links quebrados e as features/MRs ligados ao repo.
pub fn build(
    notes: &[Note],
    feats: &[FeatureRef],
    o: Opts,
    center: &str,
) -> (Vec<Node>, Vec<(usize, usize)>) {
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut edges: HashSet<(usize, usize)> = HashSet::new();
    let mut add = |nodes: &mut Vec<Node>, id: &str, dir: Option<&str>| -> usize {
        let i = *index.entry(id.to_string()).or_insert_with(|| {
            nodes.push(Node {
                id: id.to_string(),
                dir: "ghost".into(),
                exists: false,
                deg: 0,
                key: None,
                pane: None,
                x: 0.,
                y: 0.,
                vx: 0.,
                vy: 0.,
            });
            nodes.len() - 1
        });
        if let Some(d) = dir {
            nodes[i].dir = d.into();
            nodes[i].exists = true;
        }
        i
    };
    let link = |edges: &mut HashSet<(usize, usize)>, a: usize, b: usize| {
        if a != b {
            edges.insert((a.min(b), a.max(b)));
        }
    };
    for n in notes {
        let a = add(&mut nodes, &n.name, Some(&n.dir));
        nodes[a].key = Some(n.key.clone());
        for l in n.links() {
            let b = add(&mut nodes, &l, None);
            link(&mut edges, a, b);
        }
    }
    if o.feats {
        for f in feats {
            let id = format!("⎇ {}", f.branch);
            let a = add(&mut nodes, &id, Some("feature"));
            nodes[a].pane = f.pane.clone();
            let r = add(&mut nodes, &f.repo, None);
            link(&mut edges, a, r);
            if let Some(m) = f.mr {
                let b = add(&mut nodes, &format!("!{m}"), Some("mr"));
                nodes[b].pane = f.pane.clone();
                link(&mut edges, a, b);
            }
        }
    }
    let mut edges: Vec<(usize, usize)> = edges.into_iter().collect();
    edges.sort();
    let mut keep: Vec<bool> = nodes.iter().map(|n| o.ghosts || n.exists).collect();
    if o.local
        && let Some(&c) = index.get(center)
    {
        let mut near = HashSet::from([c]);
        for _ in 0..2 {
            for &(a, b) in &edges {
                if near.contains(&a) && keep[b] {
                    near.insert(b);
                } else if near.contains(&b) && keep[a] {
                    near.insert(a);
                }
            }
        }
        for (i, k) in keep.iter_mut().enumerate() {
            *k = *k && near.contains(&i);
        }
    }
    let map: Vec<Option<usize>> = keep
        .iter()
        .scan(0, |n, &k| {
            let r = k.then_some(*n);
            if k {
                *n += 1;
            }
            Some(r)
        })
        .collect();
    let mut kept: Vec<Node> = nodes
        .into_iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(n, _)| n)
        .collect();
    let edges: Vec<(usize, usize)> = edges
        .into_iter()
        .filter_map(|(a, b)| Some((map[a]?, map[b]?)))
        .collect();
    for &(a, b) in &edges {
        kept[a].deg += 1;
        kept[b].deg += 1;
    }
    for (i, n) in kept.iter_mut().enumerate() {
        let a = i as f32 * 2.4;
        n.x = a.cos() * (60. + i as f32 * 6.);
        n.y = a.sin() * (60. + i as f32 * 6.);
    }
    (kept, edges)
}

/// Um passo da simulação do protótipo (repulsão O(n²), molas nos links, atrito).
// ponytail: O(n²) por passo; Barnes-Hut quando o vault passar de ~500 notas
pub fn tick(nodes: &mut [Node], edges: &[(usize, usize)], alpha: f32, drag: Option<usize>) {
    let n = nodes.len();
    for a in 0..n {
        let (mut fx, mut fy) = (0., 0.);
        for b in 0..n {
            if a == b {
                continue;
            }
            let (dx, dy) = (nodes[a].x - nodes[b].x, nodes[a].y - nodes[b].y);
            let d2 = (dx * dx + dy * dy).max(1.);
            if d2 < 90000. {
                let f = 900. / d2;
                fx += dx * f * 0.02;
                fy += dy * f * 0.02;
            }
        }
        nodes[a].vx += fx - nodes[a].x * 0.004;
        nodes[a].vy += fy - nodes[a].y * 0.004;
    }
    for &(a, b) in edges {
        let (dx, dy) = (nodes[b].x - nodes[a].x, nodes[b].y - nodes[a].y);
        let d = (dx * dx + dy * dy).sqrt().max(1.);
        let f = (d - 70.) * 0.012;
        nodes[a].vx += dx / d * f;
        nodes[a].vy += dy / d * f;
        nodes[b].vx -= dx / d * f;
        nodes[b].vy -= dy / d * f;
    }
    for (i, nd) in nodes.iter_mut().enumerate() {
        if Some(i) == drag {
            continue;
        }
        nd.vx *= 0.82;
        nd.vy *= 0.82;
        nd.x += nd.vx * alpha * 4.;
        nd.y += nd.vy * alpha * 4.;
    }
}

fn radius(n: &Node) -> f32 {
    3. + (n.deg as f32).sqrt() * 2.4
}

#[derive(Clone, Copy)]
struct View {
    x: f32,
    y: f32,
    k: f32,
}

enum Press {
    Node(usize),
    Pan(Point<Pixels>, f32, f32),
}

/// O grafo do vault (estilo Obsidian): arrastar, zoom na roda, hover destaca vizinhos, clique abre.
pub struct Graph {
    nodes: Vec<Node>,
    edges: Vec<(usize, usize)>,
    view: View,
    alpha: f32,
    hover: Option<usize>,
    press: Option<Press>,
    moved: bool,
    fitted: bool,
    touched: bool,
    opts: Opts,
    query: Entity<TextField>,
    current: String,
    center: String,
    data: (Vec<Note>, Vec<FeatureRef>),
    bounds: Rc<Cell<Bounds<Pixels>>>,
    theme: Theme,
    _anim: Option<Task<()>>,
}

impl EventEmitter<Event> for Graph {}

impl Graph {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        let t = theme.clone();
        let query = cx.new(|cx| TextField::new(t, "", "buscar nota…", false, false, cx));
        cx.subscribe(&query, |_, _, e: &FieldEvent, cx| {
            if matches!(e, FieldEvent::Changed) {
                cx.notify();
            }
        })
        .detach();
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            view: View {
                x: 0.,
                y: 0.,
                k: 1.,
            },
            alpha: 1.,
            hover: None,
            press: None,
            moved: false,
            fitted: false,
            touched: false,
            opts: Opts {
                local: false,
                ghosts: true,
                feats: true,
            },
            query,
            current: String::new(),
            center: String::new(),
            data: (Vec::new(), Vec::new()),
            bounds: Rc::new(Cell::new(Bounds::default())),
            theme,
            _anim: None,
        }
    }

    /// O "só <repo> (2 saltos)" parte desta nota.
    pub fn set_center(&mut self, name: String) {
        if !name.is_empty() {
            self.center = name;
        }
    }

    pub fn highlight(&mut self, name: String, cx: &mut Context<Self>) {
        self.query.update(cx, |q, cx| q.set(name, cx));
    }

    pub fn set_data(
        &mut self,
        notes: &[Note],
        feats: &[FeatureRef],
        current: String,
        cx: &mut Context<Self>,
    ) {
        if !current.is_empty() {
            self.center = current.clone();
        }
        self.current = current;
        if self.data.0 == notes && self.data.1 == feats {
            return cx.notify();
        }
        self.data = (notes.to_vec(), feats.to_vec());
        self.rebuild(cx);
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let prev: HashMap<String, (f32, f32)> = self
            .nodes
            .iter()
            .map(|n| (n.id.clone(), (n.x, n.y)))
            .collect();
        let (mut nodes, edges) = build(&self.data.0, &self.data.1, self.opts, &self.center);
        for n in &mut nodes {
            if let Some(&(x, y)) = prev.get(&n.id) {
                n.x = x;
                n.y = y;
            }
        }
        self.alpha = if prev.is_empty() { 1. } else { 0.3 };
        self.nodes = nodes;
        self.edges = edges;
        // a primeira abertura já vem calculada e enquadrada, como no protótipo
        if prev.is_empty() {
            for _ in 0..240 {
                tick(&mut self.nodes, &self.edges, self.alpha, None);
                self.alpha = (self.alpha * 0.985).max(0.);
            }
            self.fitted = false;
        }
        self.animate(cx);
    }

    fn animate(&mut self, cx: &mut Context<Self>) {
        if self._anim.is_some() {
            return;
        }
        self._anim = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let go = this.update(cx, |g, cx| {
                    let dragging = matches!(g.press, Some(Press::Node(_)));
                    if g.alpha > 0.01 || dragging {
                        let drag = match g.press {
                            Some(Press::Node(i)) => Some(i),
                            _ => None,
                        };
                        tick(&mut g.nodes, &g.edges, g.alpha, drag);
                        g.alpha = (g.alpha * 0.985).max(0.);
                        cx.notify();
                        true
                    } else {
                        g._anim = None;
                        false
                    }
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn fit(&mut self) {
        let b = self.bounds.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        if self.nodes.is_empty() || w <= 0. {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for n in &self.nodes {
            x0 = x0.min(n.x);
            y0 = y0.min(n.y);
            x1 = x1.max(n.x);
            y1 = y1.max(n.y);
        }
        self.view.k = (w / (x1 - x0 + 120.))
            .min(h / (y1 - y0 + 120.))
            .clamp(0.3, 2.2);
        self.view.x = -(x0 + x1) / 2.;
        self.view.y = -(y0 + y1) / 2.;
        self.fitted = true;
    }

    fn to_world(&self, p: Point<Pixels>) -> (f32, f32) {
        let b = self.bounds.get();
        let sx = f32::from(p.x - b.origin.x) - f32::from(b.size.width) / 2.;
        let sy = f32::from(p.y - b.origin.y) - f32::from(b.size.height) / 2.;
        (
            sx / self.view.k - self.view.x,
            sy / self.view.k - self.view.y,
        )
    }

    fn pick(&self, p: Point<Pixels>) -> Option<usize> {
        let (wx, wy) = self.to_world(p);
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (i, ((n.x - wx).powi(2) + (n.y - wy).powi(2)).sqrt(), n))
            .filter(|(_, d, n)| *d < radius(n) + 6. / self.view.k)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _, _)| i)
    }

    /// O `openNode` do protótipo.
    fn open(&mut self, i: usize, cx: &mut Context<Self>) {
        let n = &self.nodes[i];
        match (&n.key, &n.pane) {
            (Some(k), _) => cx.emit(Event::Open(k.clone())),
            (None, Some(p)) => cx.emit(Event::Focus(p.clone())),
            _ if !n.exists => cx.emit(Event::Toast(format!(
                "[[{}]] ainda não existe: link quebrado é TODO, não erro.",
                n.id
            ))),
            _ => {}
        }
    }

    fn color(&self, dir: &str) -> Hsla {
        let t = &self.theme;
        match dir {
            "repos" => t.accent,
            "decisions" => t.ok,
            "daily" => rgb(0xf1fa8c).into(),
            "." => t.cyan,
            "feature" => t.orange,
            "mr" => rgb(0xff79c6).into(),
            _ => t.comment,
        }
    }
}

impl Render for Graph {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        if !self.fitted && !self.touched && self.alpha < 0.25 {
            self.fit();
        }
        let q = self.query.read(cx).value.trim().to_lowercase();
        let nodes = self.nodes.clone();
        let edges = self.edges.clone();
        let view = self.view;
        let hover = self.hover;
        let current = self.current.clone();
        let bounds = self.bounds.clone();
        let colors: Vec<Hsla> = nodes.iter().map(|n| self.color(&n.dir)).collect();
        let (line, fg, muted, comment, sans) = (t.line, t.fg, t.muted, t.comment, t.sans);
        let canvas_el = canvas(
            move |b, _, _| bounds.set(b),
            move |b, _, window, cx| {
                let center = b.center();
                let to_s = |x: f32, y: f32| {
                    point(
                        center.x + px((x + view.x) * view.k),
                        center.y + px((y + view.y) * view.k),
                    )
                };
                let mut nb: Vec<HashSet<usize>> = vec![HashSet::new(); nodes.len()];
                for &(a, c) in &edges {
                    nb[a].insert(c);
                    nb[c].insert(a);
                }
                let hit = |n: &Node| q.is_empty() || n.id.to_lowercase().contains(&q);
                let lit = |i: usize| match hover {
                    Some(h) => i == h || nb[h].contains(&i),
                    None => hit(&nodes[i]),
                };
                window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                    for &(a, c) in &edges {
                        let on = hover.is_some_and(|h| a == h || c == h);
                        let alpha = if hover.is_some() && !on {
                            0.15
                        } else if !q.is_empty() && !(hit(&nodes[a]) || hit(&nodes[c])) {
                            0.12
                        } else {
                            0.9
                        };
                        let color = if on { colors[hover.unwrap()] } else { line };
                        let mut p = PathBuilder::stroke(px(1.));
                        if !nodes[a].exists || !nodes[c].exists {
                            p = p.dash_array(&[px(3.), px(3.)]);
                        }
                        p.move_to(to_s(nodes[a].x, nodes[a].y));
                        p.line_to(to_s(nodes[c].x, nodes[c].y));
                        if let Ok(path) = p.build() {
                            window.paint_path(path, Hsla { a: alpha, ..color });
                        }
                    }
                    let scale = view.k.clamp(0.7, 1.6);
                    for (i, n) in nodes.iter().enumerate() {
                        let pos = to_s(n.x, n.y);
                        let r = radius(n) * scale;
                        let a = if lit(i) { 1. } else { 0.2 };
                        let rect = Bounds::new(
                            point(pos.x - px(r), pos.y - px(r)),
                            size(px(2. * r), px(2. * r)),
                        );
                        if n.exists {
                            window.paint_quad(
                                fill(rect, Hsla { a, ..colors[i] }).corner_radii(px(r)),
                            );
                        } else {
                            window.paint_quad(
                                outline(rect, Hsla { a, ..comment }, BorderStyle::Dashed)
                                    .corner_radii(px(r)),
                            );
                        }
                        if n.id == current {
                            let rr = r + 3.;
                            let ring = Bounds::new(
                                point(pos.x - px(rr), pos.y - px(rr)),
                                size(px(2. * rr), px(2. * rr)),
                            );
                            window.paint_quad(
                                outline(ring, fg, BorderStyle::Solid)
                                    .corner_radii(px(rr))
                                    .border_widths(px(2.)),
                            );
                        }
                        let show = Some(i) == hover
                            || hover.is_some_and(|h| nb[h].contains(&i))
                            || (!q.is_empty() && hit(n))
                            || view.k > 1.25
                            || n.deg >= 4
                            || n.id == current;
                        if show {
                            let la = if lit(i) { 1. } else { 0.3 };
                            let mut f = font(sans);
                            if Some(i) == hover {
                                f.weight = FontWeight::SEMIBOLD;
                            }
                            let run = TextRun {
                                len: n.id.len(),
                                font: f,
                                color: Hsla {
                                    a: la,
                                    ..if n.exists { fg } else { muted }
                                },
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let shaped = window.text_system().shape_line(
                                n.id.clone().into(),
                                px(11.5),
                                &[run],
                                None,
                            );
                            let origin = point(pos.x - shaped.width / 2., pos.y + px(r + 3.));
                            let _ =
                                shaped.paint(origin, px(14.), TextAlign::Left, None, window, cx);
                        }
                    }
                });
            },
        )
        .size_full();
        let legend = [
            ("repos", "repos"),
            ("decisions", "decisions"),
            ("daily", "daily"),
            ("feature", "feature"),
            ("mr", "MR"),
        ];
        let opt = |id: &'static str, on: bool, label: String| {
            div()
                .id(id)
                .flex()
                .gap(px(6.))
                .items_center()
                .cursor_pointer()
                .child(checkbox(&t, on))
                .child(label)
        };
        let center = self.center.clone();
        div()
            .size_full()
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .bg(t.raise)
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .font_weight(FontWeight::MEDIUM)
                    .child("GRAFO DO VAULT"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(12.))
                    .items_center()
                    .text_size(px(12.))
                    .child(div().w(px(160.)).child(self.query.clone()))
                    .child(
                        opt(
                            "g-local",
                            self.opts.local,
                            format!(
                                "só {} (2 saltos)",
                                if center.is_empty() {
                                    "esta nota".into()
                                } else {
                                    center
                                }
                            ),
                        )
                        .on_click(cx.listener(|g, _, _, cx| {
                            g.opts.local = !g.opts.local;
                            g.rebuild(cx);
                        })),
                    )
                    .child(
                        opt("g-ghost", self.opts.ghosts, "links quebrados".into()).on_click(
                            cx.listener(|g, _, _, cx| {
                                g.opts.ghosts = !g.opts.ghosts;
                                g.rebuild(cx);
                            }),
                        ),
                    )
                    .child(
                        opt("g-feats", self.opts.feats, "features e MRs".into()).on_click(
                            cx.listener(|g, _, _, cx| {
                                g.opts.feats = !g.opts.feats;
                                g.rebuild(cx);
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .id("g-box")
                    .flex_1()
                    .min_h(px(340.))
                    .bg(t.win)
                    .rounded(px(6.))
                    .overflow_hidden()
                    .cursor(if self.hover.is_some() {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::OpenHand
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|g, e: &MouseDownEvent, _, cx| {
                            g.touched = true;
                            g.moved = false;
                            g.press = Some(match g.pick(e.position) {
                                Some(i) => {
                                    g.alpha = g.alpha.max(0.3);
                                    g.animate(cx);
                                    Press::Node(i)
                                }
                                None => Press::Pan(e.position, g.view.x, g.view.y),
                            });
                        }),
                    )
                    .on_mouse_move(cx.listener(|g, e: &MouseMoveEvent, _, cx| {
                        match g.press {
                            Some(Press::Node(i)) => {
                                let (x, y) = g.to_world(e.position);
                                g.nodes[i].x = x;
                                g.nodes[i].y = y;
                                g.moved = true;
                            }
                            Some(Press::Pan(start, vx, vy)) => {
                                g.view.x = vx + f32::from(e.position.x - start.x) / g.view.k;
                                g.view.y = vy + f32::from(e.position.y - start.y) / g.view.k;
                                g.moved = true;
                            }
                            None => {
                                let h = g.pick(e.position);
                                if h == g.hover {
                                    return;
                                }
                                g.hover = h;
                            }
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|g, _, _, cx| {
                            if let Some(Press::Node(i)) = g.press.take()
                                && !g.moved
                            {
                                g.open(i, cx);
                            }
                            g.press = None;
                            cx.notify();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|g, e: &ScrollWheelEvent, _, cx| {
                        g.touched = true;
                        let dy = f32::from(e.delta.pixel_delta(px(20.)).y);
                        if dy == 0. {
                            return;
                        }
                        let (wx, wy) = g.to_world(e.position);
                        g.view.k =
                            (g.view.k * if dy > 0. { 1.12 } else { 1. / 1.12 }).clamp(0.3, 3.);
                        let (nx, ny) = g.to_world(e.position);
                        g.view.x += nx - wx;
                        g.view.y += ny - wy;
                        cx.notify();
                    }))
                    .child(canvas_el),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(12.))
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(t.muted)
                    .children(legend.iter().map(|(d, l)| {
                        div()
                            .flex()
                            .gap(px(4.))
                            .items_center()
                            .child(div().size(px(9.)).rounded_full().bg(self.color(d)))
                            .child(l.to_string())
                    }))
                    .child("◌ link quebrado")
                    .child(div().flex_1())
                    .when(!self.nodes.is_empty(), |d| {
                        d.child("arraste · roda do mouse = zoom · clique abre")
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Opts, build, tick};
    use crate::{vault::Note, vault_view::FeatureRef};

    fn note(dir: &str, name: &str, text: &str) -> Note {
        Note {
            key: format!("{dir}/{name}.md"),
            dir: dir.into(),
            name: name.into(),
            text: text.into(),
        }
    }

    #[test]
    fn grafo_do_vault() {
        let notes = vec![
            note("repos", "bots-api", "[[atendimento-zendesk]] [[gotcha]]"),
            note("decisions", "gotcha", "[[bots-api]]"),
            note("repos", "longe", "[[x]]"),
        ];
        let feats = vec![FeatureRef {
            branch: "feat/a".into(),
            repo: "bots-api".into(),
            pane: Some("w1:p1".into()),
            mr: Some(7),
            status: String::new(),
            merged: None,
            cost: 0.,
        }];
        let all = Opts {
            local: false,
            ghosts: true,
            feats: true,
        };
        let (n, e) = build(&notes, &feats, all, "bots-api");
        let ids: Vec<&str> = n.iter().map(|x| x.id.as_str()).collect();
        assert!(
            ids.contains(&"atendimento-zendesk")
                && ids.contains(&"⎇ feat/a")
                && ids.contains(&"!7")
        );
        let bots = n.iter().find(|x| x.id == "bots-api").unwrap();
        assert_eq!(bots.deg, 3);
        assert_eq!(e.len(), 5);
        let (n2, _) = build(
            &notes,
            &feats,
            Opts {
                ghosts: false,
                feats: false,
                ..all
            },
            "",
        );
        assert!(n2.iter().all(|x| x.exists) && n2.len() == 3);
        let (n3, _) = build(&notes, &[], Opts { local: true, ..all }, "bots-api");
        assert!(!n3.iter().any(|x| x.id == "longe"));
        let (mut n4, e4) = build(&notes, &feats, all, "");
        for _ in 0..50 {
            tick(&mut n4, &e4, 1., None);
        }
        assert!(n4.iter().all(|x| x.x.is_finite() && x.y.is_finite()));
    }
}
